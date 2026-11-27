use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use http::{Method, StatusCode};
use serde_json::{Value, json};

use super::ForgeKernel;
use super::assets::percent_decode;
use super::http::{binary_ok_response, json_error_response, json_ok_response};
use crate::error::{ForgeError, codes};
use crate::handler::{InvokeContext, IpcBody};
use crate::runtime::{AppHandle, ProtocolRequest, ProtocolResponse};
use crate::util::constant_time_eq;

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

        // Strict window resolution: never fall back to `"main"` if a secondary window closed or
        // an unmapped frame issued a request, preventing per-window permission escalation.
        let Some(window) = self.registry.window_handle(&req.window) else {
            return json_error_response(
                StatusCode::NOT_FOUND,
                &ForgeError::not_found(
                    codes::WINDOW_NOT_FOUND,
                    format!("calling window '{}' is not open", req.window),
                ),
                csp,
            );
        };

        let cmd_name = percent_decode(encoded_cmd);
        if cmd_name == "__forge_drain_events" {
            let events = window.mark_js_ready_and_drain_early();
            return json_ok_response(&json!(events), csp);
        }
        if self.smoke_test_enabled && cmd_name == "__forge_ready" {
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

        // Single permission choke point: evaluated for every command (user, plugin, and built-in).
        if let Err(perm_err) = self.permissions.check(window.label(), &cmd_name) {
            return json_error_response(perm_err.status_code(), &perm_err, csp);
        }

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

        let is_octet_stream = req
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| {
                ct.split(';')
                    .next()
                    .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/octet-stream"))
            });

        let ipc_body = if is_octet_stream {
            IpcBody::Bytes(req.body)
        } else if req.body.is_empty() {
            IpcBody::Json(Value::Null)
        } else {
            match serde_json::from_slice(&req.body) {
                Ok(v) => IpcBody::Json(v),
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
            headers: req.headers,
        };

        let fut = command.call(ctx, ipc_body);
        match CatchUnwindFuture(fut).await {
            Ok(Ok(IpcBody::Json(reply_value))) => json_ok_response(&reply_value, csp),
            Ok(Ok(IpcBody::Bytes(reply_bytes))) => binary_ok_response(reply_bytes, csp),
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
