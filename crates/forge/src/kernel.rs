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
use serde_json::{Value, json};
use tokio::task::AbortHandle;

use crate::config::{Config, unpack_tar_gz_assets};
use crate::error::{Error, ForgeError, Result};
use crate::handler::{ErasedCommand, InvokeContext, StateMap};
use crate::runtime::{ProtocolRequest, ProtocolResponse, WindowHandle};

include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));

const STRICT_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'";
const DEV_PROXY_CSP: &str = "default-src 'self' http://localhost:* http://127.0.0.1:*; script-src 'self' 'unsafe-inline' 'unsafe-eval' http://localhost:* http://127.0.0.1:*; style-src 'self' 'unsafe-inline' http://localhost:* http://127.0.0.1:*; img-src 'self' data: blob: http://localhost:* http://127.0.0.1:*; font-src 'self' data: http://localhost:* http://127.0.0.1:*; connect-src 'self' http://localhost:* http://127.0.0.1:* ws://localhost:* ws://127.0.0.1:*";
const BOOTSTRAP_TAG: &str = r#"<script src="/__forge/bootstrap.js"></script>"#;

pub(crate) struct ForgeKernel {
    pub(crate) config: Arc<Config>,
    pub(crate) canonical_assets_root: Option<PathBuf>,
    pub(crate) states: Arc<StateMap>,
    pub(crate) commands: HashMap<&'static str, Arc<dyn ErasedCommand>>,
    pub(crate) launch_token: String,
    pub(crate) window_handle: WindowHandle,
    pub(crate) tokio_handle: tokio::runtime::Handle,
    #[cfg(feature = "dev-tools")]
    pub(crate) dev_proxy: Option<(String, ureq::Agent)>,
    #[cfg(feature = "dev-tools")]
    _watcher_guard: Option<Box<dyn Send + Sync>>,
}

impl ForgeKernel {
    pub(crate) fn new(
        root_dir: &Path,
        mut config: Config,
        states: StateMap,
        commands: HashMap<&'static str, Arc<dyn ErasedCommand>>,
        window_handle: WindowHandle,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<Self> {
        let dev_watch_enabled = std::env::var("FORGE_DEV_WATCH").ok().as_deref() == Some("1");
        let smoke_test_enabled = std::env::var("FORGE_SMOKE_TEST").ok().as_deref() == Some("1");
        if smoke_test_enabled {
            tokio_handle.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                eprintln!("[FORGE_SMOKE_TEST] timed out after 30s waiting for __forge_ready");
                std::process::exit(124);
            });
        }
        let active_dev_url = config
            .dev
            .url
            .clone()
            .filter(|_| dev_watch_enabled || std::env::var("FORGE_DEV_URL").is_ok());

        // Hydrate single-binary embedded UI archive when running standalone release builds.
        if config.in_memory_assets.is_none()
            && !dev_watch_enabled
            && active_dev_url.is_none()
            && let Some(archive_bytes) = EMBEDDED_UI_ARCHIVE
        {
            config.in_memory_assets = Some(unpack_tar_gz_assets(archive_bytes)?);
        }

        let canonical_assets_root = if config.in_memory_assets.is_some() || active_dev_url.is_some()
        {
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

        #[cfg(feature = "dev-tools")]
        let dev_proxy = active_dev_url.as_ref().map(|url| {
            let agent_cfg = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(std::time::Duration::from_secs(10)))
                .build();
            let agent: ureq::Agent = agent_cfg.into();
            (url.trim_end_matches('/').to_string(), agent)
        });

        #[cfg(feature = "dev-tools")]
        let watcher_guard = if dev_watch_enabled
            && active_dev_url.is_none()
            && let Some(watch_root) = canonical_assets_root.as_ref()
        {
            start_live_reload_watcher(watch_root, window_handle.clone(), &tokio_handle).ok()
        } else {
            None
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
            #[cfg(feature = "dev-tools")]
            dev_proxy,
            #[cfg(feature = "dev-tools")]
            _watcher_guard: watcher_guard,
        })
    }

    /// Non-blocking entry point invoked by GUI / IO threads (`WebViewHost` and `CefHost`).
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

    pub(crate) async fn handle_request(&self, req: ProtocolRequest) -> ProtocolResponse {
        let raw_path = req.uri.path();

        if raw_path == "/__forge/bootstrap.js" {
            return self.serve_bootstrap(&req.method);
        }

        if let Some(encoded_cmd) = raw_path.strip_prefix("/__forge_ipc/") {
            let encoded_cmd = encoded_cmd.to_owned();
            return self.handle_ipc(&encoded_cmd, req).await;
        }

        #[cfg(feature = "dev-tools")]
        if let Some((base_url, agent)) = &self.dev_proxy {
            let path_and_query = req
                .uri
                .path_and_query()
                .map(|pq| pq.as_str().to_string())
                .unwrap_or_else(|| "/".to_string());
            let base = base_url.clone();
            let agent = agent.clone();
            let method = req.method.clone();
            return match tokio::task::spawn_blocking(move || {
                proxy_dev_request(&agent, &base, &method, &path_and_query)
            })
            .await
            {
                Ok(res) => res,
                Err(e) => plain_response(
                    StatusCode::BAD_GATEWAY,
                    format!("Dev server proxy task failed: {e}").into_bytes(),
                    DEV_PROXY_CSP,
                ),
            };
        }

        self.serve_static_asset(&req.method, raw_path)
    }

    fn active_csp(&self) -> &'static str {
        #[cfg(feature = "dev-tools")]
        if self.dev_proxy.is_some() {
            return DEV_PROXY_CSP;
        }
        STRICT_CSP
    }

    fn serve_bootstrap(&self, method: &Method) -> ProtocolResponse {
        if *method != Method::GET && *method != Method::HEAD {
            return plain_response(
                StatusCode::METHOD_NOT_ALLOWED,
                b"Method Not Allowed".to_vec(),
                self.active_csp(),
            );
        }

        let runtime_name = self.config.runtime.default.as_str();
        let token = &self.launch_token;
        let smoke_flag = if std::env::var("FORGE_SMOKE_TEST").ok().as_deref() == Some("1") {
            "true"
        } else {
            "false"
        };
        let script = format!(
            r#"(() => {{
  "use strict";
  const TOKEN = "{token}";
  const RUNTIME = "{runtime_name}";
  const SMOKE = {smoke_flag};
  const listeners = new Map();

  function dispatch(event, payload) {{
    if (event === "__forge:css-reload") {{
      const stamp = Date.now();
      document.querySelectorAll('link[rel="stylesheet"]').forEach((link) => {{
        const href = link.getAttribute("href");
        if (href) {{
          const clean = href.split("?")[0];
          link.setAttribute("href", clean + "?__forge_t=" + stamp);
        }}
      }});
      return;
    }}
    if (event === "__forge:reload") {{
      window.location.reload();
      return;
    }}
    const set = listeners.get(event);
    if (!set) return;
    for (const fn of Array.from(set)) {{
      try {{
        fn(payload);
      }} catch (err) {{
        console.error("[FORGE] listener error on", event, err);
      }}
    }}
  }}

  function listen(event, handler) {{
    if (typeof event !== "string" || typeof handler !== "function") {{
      throw new TypeError("forge.listen(event, handler) requires a string and a function");
    }}
    let set = listeners.get(event);
    if (!set) {{
      set = new Set();
      listeners.set(event, set);
    }}
    set.add(handler);
    return () => {{
      set.delete(handler);
      if (set.size === 0) listeners.delete(event);
    }};
  }}

  function once(event, handler) {{
    const unlisten = listen(event, (payload) => {{
      unlisten();
      handler(payload);
    }});
    return unlisten;
  }}

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

  Object.defineProperty(window, "__forge_dispatch", {{
    value: dispatch,
    writable: false,
    configurable: false,
    enumerable: false
  }});

  Object.defineProperty(window, "forge", {{
    value: Object.freeze({{ runtime: RUNTIME, invoke, listen, once }}),
    writable: false,
    configurable: false,
    enumerable: true
  }});

  window.addEventListener("DOMContentLoaded", () => {{
    invoke("__forge_drain_events").then((queued) => {{
      if (Array.isArray(queued)) {{
        for (const [ev, payload] of queued) {{
          dispatch(ev, payload);
        }}
      }}
      if (SMOKE) {{
        return invoke("__forge_ready", {{ ua: navigator.userAgent }});
      }}
    }}).catch((err) => {{
      console.error("[FORGE] bootstrap drain/smoke error:", err);
    }});
  }}, {{ once: true }});
}})();
"#
        );

        let body = if *method == Method::HEAD {
            Vec::new()
        } else {
            script.into_bytes()
        };

        let mut headers = security_headers(self.active_csp());
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
        let csp = self.active_csp();
        if req.method != Method::POST {
            return json_error_response(
                StatusCode::METHOD_NOT_ALLOWED,
                &ForgeError::new("FORGE_METHOD_NOT_ALLOWED", "IPC endpoints require POST"),
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
                &ForgeError::new("FORGE_FORBIDDEN", "cross-origin IPC invocation denied"),
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
                &ForgeError::new(
                    "FORGE_FORBIDDEN",
                    "missing or invalid X-Forge-Token capability header",
                ),
                csp,
            );
        }

        let cmd_name = percent_decode(encoded_cmd);
        if cmd_name == "__forge_drain_events" {
            let events = self.window_handle.mark_js_ready_and_drain_early();
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
            self.window_handle.quit_app();
            return json_ok_response(&json!({ "ok": true }), csp);
        }

        let Some(command) = self.commands.get(cmd_name.as_str()) else {
            return json_error_response(
                StatusCode::NOT_FOUND,
                &ForgeError::new(
                    "FORGE_UNKNOWN_COMMAND",
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
                            "FORGE_INVALID_PAYLOAD",
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
            window: self.window_handle.clone(),
        };

        let fut = command.call(ctx, payload);
        match CatchUnwindFuture(fut).await {
            Ok(Ok(reply_value)) => json_ok_response(&reply_value, csp),
            Ok(Err(forge_err)) => json_error_response(forge_err.status_code(), &forge_err, csp),
            Err(panic_payload) => {
                let panic_msg = extract_panic_message(panic_payload);
                let err = ForgeError::new(
                    "FORGE_HANDLER_PANIC",
                    format!("command '{cmd_name}' panicked: {panic_msg}"),
                );
                json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &err, csp)
            }
        }
    }

    fn serve_static_asset(&self, method: &Method, raw_path: &str) -> ProtocolResponse {
        let csp = self.active_csp();
        if *method != Method::GET && *method != Method::HEAD {
            return plain_response(
                StatusCode::METHOD_NOT_ALLOWED,
                b"Method Not Allowed".to_vec(),
                csp,
            );
        }

        let Some(clean_rel) = sanitize_asset_path(raw_path) else {
            return plain_response(StatusCode::FORBIDDEN, b"Forbidden".to_vec(), csp);
        };

        let raw_bytes = if let Some(mem_map) = &self.config.in_memory_assets {
            let key = format!("/{clean_rel}");
            match mem_map.get(&key) {
                Some(bytes) => bytes.clone(),
                None => return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp),
            }
        } else if let Some(canon_root) = &self.canonical_assets_root {
            let candidate = canon_root.join(&clean_rel);
            let Ok(canon_target) = candidate.canonicalize() else {
                return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp);
            };
            if !canon_target.starts_with(canon_root) {
                return plain_response(StatusCode::FORBIDDEN, b"Forbidden".to_vec(), csp);
            }
            if !canon_target.is_file() {
                return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp);
            }
            match std::fs::read(&canon_target) {
                Ok(b) => b,
                Err(_) => return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp),
            }
        } else {
            return plain_response(StatusCode::NOT_FOUND, b"Not Found".to_vec(), csp);
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

        let mut headers = security_headers(csp);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
        headers.insert(CONTENT_LENGTH, HeaderValue::from(content_len));

        ProtocolResponse {
            status: StatusCode::OK,
            headers,
            body,
        }
    }
}

#[cfg(feature = "dev-tools")]
fn start_live_reload_watcher(
    watch_root: &Path,
    window: WindowHandle,
    tokio_handle: &tokio::runtime::Handle,
) -> std::result::Result<Box<dyn Send + Sync>, notify::Error> {
    use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                        if !event.paths.is_empty() =>
                    {
                        let _ = tx.send(event.paths);
                    }
                    _ => {}
                }
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(watch_root, RecursiveMode::Recursive)?;

    tokio_handle.spawn(async move {
        while let Some(mut changed_paths) = rx.recv().await {
            // Debounce bursty save events across 65ms.
            tokio::time::sleep(std::time::Duration::from_millis(65)).await;
            while let Ok(more) = rx.try_recv() {
                changed_paths.extend(more);
            }
            let meaningful: Vec<&Path> = changed_paths
                .iter()
                .map(PathBuf::as_path)
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| !n.starts_with('.') && !n.ends_with('~'))
                })
                .collect();
            if meaningful.is_empty() {
                continue;
            }
            let css_only = meaningful.iter().all(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("css"))
            });
            let topic = if css_only {
                "__forge:css-reload"
            } else {
                "__forge:reload"
            };
            let _ = window.emit(topic, json!({ "count": meaningful.len() }));
        }
    });

    Ok(Box::new(watcher))
}

#[cfg(feature = "dev-tools")]
fn proxy_dev_request(
    agent: &ureq::Agent,
    base_url: &str,
    method: &Method,
    path_and_query: &str,
) -> ProtocolResponse {
    if *method != Method::GET && *method != Method::HEAD {
        return plain_response(
            StatusCode::METHOD_NOT_ALLOWED,
            b"Method Not Allowed".to_vec(),
            DEV_PROXY_CSP,
        );
    }

    let target_url = format!("{base_url}{path_and_query}");
    let upstream = match agent.get(&target_url).call() {
        Ok(res) => res,
        Err(err) => {
            return plain_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to reach frontend dev server at {target_url}: {err}").into_bytes(),
                DEV_PROXY_CSP,
            );
        }
    };

    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let raw_bytes = match upstream.into_body().read_to_vec() {
        Ok(b) => b,
        Err(err) => {
            return plain_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to read upstream dev server body: {err}").into_bytes(),
                DEV_PROXY_CSP,
            );
        }
    };

    let final_bytes = if content_type.starts_with("text/html") {
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

    let mut headers = security_headers(DEV_PROXY_CSP);
    if let Ok(hv) = HeaderValue::from_str(&content_type) {
        headers.insert(CONTENT_TYPE, hv);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(content_len));

    ProtocolResponse {
        status,
        headers,
        body,
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

fn security_headers(csp: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(csp));
    headers
}

fn plain_response(status: StatusCode, body: Vec<u8>, csp: &'static str) -> ProtocolResponse {
    let mut headers = security_headers(csp);
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

fn json_ok_response(value: &Value, csp: &'static str) -> ProtocolResponse {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"null".to_vec());
    let mut headers = security_headers(csp);
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

fn json_error_response(
    status: StatusCode,
    err: &ForgeError,
    csp: &'static str,
) -> ProtocolResponse {
    let body = serde_json::to_vec(err)
        .unwrap_or_else(|_| br#"{"code":"APP_ERROR","message":"serialization failed"}"#.to_vec());
    let mut headers = security_headers(csp);
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
