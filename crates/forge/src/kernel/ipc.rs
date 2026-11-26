use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use http::{Method, StatusCode};
use serde_json::{Value, json};

use super::ForgeKernel;
use super::assets::percent_decode;
use super::http::{json_error_response, json_ok_response};
use crate::error::{ForgeError, codes};
use crate::handler::InvokeContext;
use crate::runtime::{AppHandle, ProtocolRequest, ProtocolResponse};

impl ForgeKernel {
    pub(super) async fn handle_ipc(
        &self,
        encoded_cmd: &str,
        req: ProtocolRequest,
    ) -> ProtocolResponse {
        let csp = self.active_csp();
        if req.method != Method::POST {
            return json_error_response(
                StatusCode::METHOD_NOT_ALLOWED,
                &ForgeError::bad_request(codes::METHOD_NOT_ALLOWED, "IPC endpoints require POST"),
                csp,
            );
        }

        if let Some(site) = req
            .headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            && site != "same-origin"
            && site != "none"
        {
            return json_error_response(
                StatusCode::FORBIDDEN,
                &ForgeError::forbidden(
                    codes::FORBIDDEN,
                    format!("rejected cross-origin Sec-Fetch-Site '{site}'"),
                ),
                csp,
            );
        }

        let provided_token = req
            .headers
            .get("x-forge-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !constant_time_eq(provided_token.as_bytes(), self.launch_token.as_bytes()) {
            return json_error_response(
                StatusCode::FORBIDDEN,
                &ForgeError::forbidden(
                    codes::FORBIDDEN,
                    "missing or invalid X-Forge-Token capability header",
                ),
                csp,
            );
        }

        let calling_window = self
            .registry
            .window_handle(&req.window)
            .or_else(|| self.registry.window_handle("main"));

        let cmd_name = percent_decode(encoded_cmd);
        if cmd_name == "__forge_drain_events" {
            let events = calling_window
                .as_ref()
                .map(|w| w.mark_js_ready_and_drain_early())
                .unwrap_or_default();
            return json_ok_response(&json!(events), csp);
        }
        if cmd_name == "__forge_ready" {
            let ua = serde_json::from_slice::<Value>(&req.body)
                .ok()
                .and_then(|v| v.get("ua").and_then(Value::as_str).map(String::from))
                .unwrap_or_default();
            println!(
                "[FORGE_SMOKE_TEST] ready runtime={} ua={ua}",
                self.config.runtime.default
            );
            self.registry.quit();
            return json_ok_response(&json!({ "ok": true }), csp);
        }

        let Some(window) = calling_window else {
            return json_error_response(
                StatusCode::NOT_FOUND,
                &ForgeError::not_found(
                    codes::WINDOW_NOT_FOUND,
                    format!("calling window '{}' is no longer open", req.window),
                ),
                csp,
            );
        };

        let Some(command) = self.commands.get(cmd_name.as_str()) else {
            return json_error_response(
                StatusCode::NOT_FOUND,
                &ForgeError::not_found(
                    codes::UNKNOWN_COMMAND,
                    format!("unknown command '{cmd_name}'"),
                ),
                csp,
            );
        };

        let payload: Value = if req.body.is_empty() {
            Value::Null
        } else {
            match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(err) => {
                    return json_error_response(
                        StatusCode::BAD_REQUEST,
                        &ForgeError::bad_request(
                            codes::INVALID_PAYLOAD,
                            format!("malformed JSON body: {err}"),
                        ),
                        csp,
                    );
                }
            }
        };

        let ctx = InvokeContext {
            states: Arc::clone(&self.states),
            config: Arc::clone(&self.config),
            app_handle: AppHandle::new(Arc::clone(&self.registry)),
            window,
        };

        let fut = command.call(ctx, payload);
        match CatchUnwindFuture(fut).await {
            Ok(Ok(reply_value)) => json_ok_response(&reply_value, csp),
            Ok(Err(forge_err)) => json_error_response(forge_err.status_code(), &forge_err, csp),
            Err(panic_payload) => {
                let panic_msg = extract_panic_message(panic_payload);
                let err = ForgeError::internal(
                    codes::HANDLER_PANIC,
                    format!("command '{cmd_name}' panicked: {panic_msg}"),
                );
                json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &err, csp)
            }
        }
    }
}

struct CatchUnwindFuture<F>(F);

impl<F: Future + Unpin> Future for CatchUnwindFuture<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = &mut self.0;
        match catch_unwind(AssertUnwindSafe(|| Pin::new(inner).poll(cx))) {
            Ok(Poll::Ready(val)) => Poll::Ready(Ok(val)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(panic_err) => Poll::Ready(Err(panic_err)),
        }
    }
}

fn extract_panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
