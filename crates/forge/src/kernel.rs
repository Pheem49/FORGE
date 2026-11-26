use std::collections::HashMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderValue,
    X_CONTENT_TYPE_OPTIONS,
};
use http::{HeaderMap, Method, StatusCode};
use serde_json::Value;
use tokio::task::AbortHandle;

use crate::config::Config;
use crate::error::{Error, ForgeError, Result};
use crate::handler::{ErasedCommand, InvokeContext, StateMap};
use crate::runtime::{ProtocolRequest, ProtocolResponse, WindowHandle};

const DEFAULT_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'";
const BOOTSTRAP_TAG: &str = r#"<script src="/__forge/bootstrap.js"></script>"#;

/// Deep in-process application engine shared identically across `WebViewHost`, `CefHost`,
/// and `HeadlessApp`. Owns configuration, frozen state, static asset resolution, capability
/// token verification, and async command routing.
pub(crate) struct ForgeKernel {
    pub(crate) config: Arc<Config>,
    pub(crate) canonical_assets_root: Option<PathBuf>,
    pub(crate) states: Arc<StateMap>,
    pub(crate) commands: HashMap<&'static str, Arc<dyn ErasedCommand>>,
    pub(crate) launch_token: String,
    pub(crate) window_handle: WindowHandle,
    pub(crate) tokio_handle: tokio::runtime::Handle,
}

impl ForgeKernel {
    pub(crate) fn new(
        root_dir: &Path,
        config: Config,
        states: StateMap,
        commands: HashMap<&'static str, Arc<dyn ErasedCommand>>,
        window_handle: WindowHandle,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<Self> {
        let canonical_assets_root = if config.in_memory_assets.is_some() {
            None
        } else {
            let candidate = if config.app.assets.is_absolute() {
                config.app.assets.clone()
            } else {
                root_dir.join(&config.app.assets)
            };
            if !candidate.is_dir() {
                return Err(Error::AssetsNotFound { dir: candidate });
            }
            Some(candidate.canonicalize().map_err(|e| Error::Config {
                path: Some(candidate),
                message: format!("failed to canonicalize assets directory: {e}"),
            })?)
        };

        let launch_token = generate_launch_token()?;

        Ok(Self {
            config: Arc::new(config),
            canonical_assets_root,
            states: Arc::new(states),
            commands,
            launch_token,
            window_handle,
            tokio_handle,
        })
    }

    /// Non-blocking entry point invoked by GUI / IO threads (`WebViewHost` and `CefHost`).
    /// Spawns request execution onto the Tokio worker pool and invokes `respond` once complete.
    #[allow(dead_code)]
    pub(crate) fn dispatch_async<F>(
        self: &Arc<Self>,
        req: ProtocolRequest,
        respond: F,
    ) -> AbortHandle
    where
        F: FnOnce(ProtocolResponse) + Send + 'static,
    {
        let kernel = Arc::clone(self);
        let join = self.tokio_handle.spawn(async move {
            let res = kernel.handle_request(req).await;
            respond(res);
        });
        join.abort_handle()
    }

    /// Core unified exchange handling static assets, bootstrap SDK injection, and `/__forge_ipc/*`.
    pub(crate) async fn handle_request(&self, req: ProtocolRequest) -> ProtocolResponse {
        let raw_path = req.uri.path();

        if raw_path == "/__forge/bootstrap.js" {
            return self.serve_bootstrap(&req.method);
        }

        if let Some(encoded_cmd) = raw_path.strip_prefix("/__forge_ipc/") {
            let encoded_cmd = encoded_cmd.to_owned();
            return self.handle_ipc(&encoded_cmd, req).await;
        }

        self.serve_static_asset(&req.method, raw_path)
    }

    fn serve_bootstrap(&self, method: &Method) -> ProtocolResponse {
        if *method != Method::GET && *method != Method::HEAD {
            return plain_response(
                StatusCode::METHOD_NOT_ALLOWED,
                b"Method Not Allowed".to_vec(),
            );
        }

        let runtime_name = self.config.runtime.default.as_str();
        let token = &self.launch_token;
        let script = format!(
            r#"(() => {{
  "use strict";
  const TOKEN = "{token}";
  const RUNTIME = "{runtime_name}";
  async function invoke(command, payload) {{
    if (typeof command !== "string" || command.length === 0) {{
      const err = new Error("Command name must be a non-empty string");
      err.code = "FORGE_INVALID_COMMAND";
      throw err;
    }}
    const res = await fetch("/__forge_ipc/" + encodeURIComponent(command), {{
      method: "POST",
      headers: {{
        "Content-Type": "application/json",
        "X-Forge-Token": TOKEN
      }},
      body: JSON.stringify(payload === undefined ? null : payload)
    }});
    const data = await res.json().catch(() => null);
    if (!res.ok) {{
      const message = (data && data.message) || ("IPC failed with status " + res.status);
      const err = new Error(message);
      err.code = (data && data.code) || "APP_ERROR";
      if (data && data.details !== undefined) {{
        err.details = data.details;
      }}
      throw err;
    }}
    return data;
  }}
  Object.defineProperty(window, "forge", {{
    value: Object.freeze({{ runtime: RUNTIME, invoke }}),
    writable: false,
    configurable: false,
    enumerable: true
  }});
}})();
"#
        );

        let body = if *method == Method::HEAD {
            Vec::new()
        } else {
            script.into_bytes()
        };

        let mut headers = security_headers();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/javascript; charset=utf-8"),
        );
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));

        ProtocolResponse {
            status: StatusCode::OK,
            headers,
            body,
        }
    }

    async fn handle_ipc(&self, encoded_cmd: &str, req: ProtocolRequest) -> ProtocolResponse {
        if req.method != Method::POST {
            return json_error_response(
                StatusCode::METHOD_NOT_ALLOWED,
                &ForgeError::new("FORGE_METHOD_NOT_ALLOWED", "IPC endpoints require POST"),
            );
        }

        // Reject cross-site fetches when the browser supplies Sec-Fetch-Site metadata.
        if let Some(site) = req
            .headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            && site != "same-origin"
            && site != "none"
        {
            return json_error_response(
                StatusCode::FORBIDDEN,
                &ForgeError::new("FORGE_FORBIDDEN", "cross-origin IPC invocation denied"),
            );
        }

        // Verify per-launch 256-bit capability token in constant time.
        let provided_token = req
            .headers
            .get("x-forge-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !constant_time_eq(provided_token.as_bytes(), self.launch_token.as_bytes()) {
            return json_error_response(
                StatusCode::FORBIDDEN,
                &ForgeError::new(
                    "FORGE_FORBIDDEN",
                    "missing or invalid X-Forge-Token capability header",
                ),
            );
        }

        let cmd_name = percent_decode(encoded_cmd);
        let Some(command) = self.commands.get(cmd_name.as_str()) else {
            return json_error_response(
                StatusCode::NOT_FOUND,
                &ForgeError::new(
                    "FORGE_UNKNOWN_COMMAND",
                    format!("unknown command '{cmd_name}'"),
                ),
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
                            "FORGE_INVALID_PAYLOAD",
                            format!("malformed JSON body: {err}"),
                        ),
                    );
                }
            }
        };

        let ctx = InvokeContext {
            states: Arc::clone(&self.states),
            config: Arc::clone(&self.config),
            window: self.window_handle.clone(),
        };

        let fut = command.call(ctx, payload);
        match CatchUnwindFuture(fut).await {
            Ok(Ok(reply_value)) => json_ok_response(&reply_value),
            Ok(Err(forge_err)) => json_error_response(forge_err.status_code(), &forge_err),
            Err(panic_payload) => {
                let panic_msg = extract_panic_message(panic_payload);
                let err = ForgeError::new(
                    "FORGE_HANDLER_PANIC",
                    format!("command '{cmd_name}' panicked: {panic_msg}"),
                );
                json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &err)
            }
        }
    }

    fn serve_static_asset(&self, method: &Method, raw_path: &str) -> ProtocolResponse {
        if *method != Method::GET && *method != Method::HEAD {
            return plain_response(
                StatusCode::METHOD_NOT_ALLOWED,
                b"Method Not Allowed".to_vec(),
            );
        }

        let Some(clean_rel) = sanitize_asset_path(raw_path) else {
            return plain_response(StatusCode::FORBIDDEN, b"Forbidden".to_vec());
        };

        let raw_bytes = if let Some(mem_map) = &self.config.in_memory_assets {
            let key = format!("/{clean_rel}");
            match mem_map.get(&key) {
                Some(bytes) => bytes.clone(),
                None => return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec()),
            }
        } else if let Some(canon_root) = &self.canonical_assets_root {
            let candidate = canon_root.join(&clean_rel);
            let Ok(canon_target) = candidate.canonicalize() else {
                return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec());
            };
            if !canon_target.starts_with(canon_root) {
                return plain_response(StatusCode::FORBIDDEN, b"Forbidden".to_vec());
            }
            if !canon_target.is_file() {
                return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec());
            }
            match std::fs::read(&canon_target) {
                Ok(b) => b,
                Err(_) => return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec()),
            }
        } else {
            return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec());
        };

        let mime = detect_mime(&clean_rel);
        let final_bytes = if mime.starts_with("text/html") {
            inject_bootstrap_script(&raw_bytes)
        } else {
            raw_bytes
        };

        let content_len = final_bytes.len();
        let body = if *method == Method::HEAD {
            Vec::new()
        } else {
            final_bytes
        };

        let mut headers = security_headers();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
        headers.insert(CONTENT_LENGTH, HeaderValue::from(content_len));

        ProtocolResponse {
            status: StatusCode::OK,
            headers,
            body,
        }
    }
}

/// Polls an inner future inside `std::panic::catch_unwind` so handler panics are converted
/// into structured `FORGE_HANDLER_PANIC` responses rather than unwinding across C++/GTK FFI.
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

fn generate_launch_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        Error::Runtime(format!(
            "failed to generate cryptographic launch token: {e}"
        ))
    })?;
    let mut out = String::with_capacity(64);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    Ok(out)
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

fn sanitize_asset_path(raw_path: &str) -> Option<String> {
    let decoded = percent_decode(raw_path);
    if decoded.contains('\0') || decoded.contains('\\') {
        return None;
    }
    let trimmed = decoded.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some("index.html".to_string());
    }

    let path = Path::new(trimmed);
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_str()?;
                parts.push(s);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    if parts.is_empty() {
        Some("index.html".to_string())
    } else {
        Some(parts.join("/"))
    }
}

fn inject_bootstrap_script(html_bytes: &[u8]) -> Vec<u8> {
    let Ok(html) = std::str::from_utf8(html_bytes) else {
        return html_bytes.to_vec();
    };
    if html.contains("/__forge/bootstrap.js") {
        return html_bytes.to_vec();
    }

    let lower = html.to_ascii_lowercase();
    if let Some(head_idx) = lower.find("<head")
        && let Some(rel_close) = lower[head_idx..].find('>')
    {
        let insert_pos = head_idx + rel_close + 1;
        let mut out = String::with_capacity(html.len() + BOOTSTRAP_TAG.len() + 1);
        out.push_str(&html[..insert_pos]);
        out.push_str(BOOTSTRAP_TAG);
        out.push_str(&html[insert_pos..]);
        return out.into_bytes();
    }

    let mut out = String::with_capacity(html.len() + BOOTSTRAP_TAG.len());
    out.push_str(BOOTSTRAP_TAG);
    out.push_str(html);
    out.into_bytes()
}

fn detect_mime(rel_path: &str) -> &'static str {
    match Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn security_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(DEFAULT_CSP),
    );
    headers
}

fn plain_response(status: StatusCode, body: Vec<u8>) -> ProtocolResponse {
    let mut headers = security_headers();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
    ProtocolResponse {
        status,
        headers,
        body,
    }
}

fn json_ok_response(value: &Value) -> ProtocolResponse {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"null".to_vec());
    let mut headers = security_headers();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
    ProtocolResponse {
        status: StatusCode::OK,
        headers,
        body,
    }
}

fn json_error_response(status: StatusCode, err: &ForgeError) -> ProtocolResponse {
    let body = serde_json::to_vec(err)
        .unwrap_or_else(|_| br#"{"code":"APP_ERROR","message":"serialization failed"}"#.to_vec());
    let mut headers = security_headers();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
    ProtocolResponse {
        status,
        headers,
        body,
    }
}
