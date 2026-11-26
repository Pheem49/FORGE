use std::sync::{Arc, Mutex};

use http::header::HeaderValue;
use http::{HeaderMap, Method, Request, StatusCode, Uri};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::error::ForgeError;
use crate::kernel::ForgeKernel;
use crate::runtime::{HostDirective, ProtocolRequest};

/// Observable HTTP-like response returned by [`HeadlessApp::get_asset`] and [`HeadlessApp::raw_exchange`].
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl RawResponse {
    pub fn text(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.body)
    }
}

/// Deterministic zero-GUI application harness created via [`crate::App::headless`].
pub struct HeadlessApp {
    kernel: Arc<ForgeKernel>,
    directives: Mutex<mpsc::UnboundedReceiver<HostDirective>>,
    emitted_events: Mutex<Vec<(String, Value)>>,
    closed: Mutex<bool>,
    _owned_runtime: Option<tokio::runtime::Runtime>,
}

impl HeadlessApp {
    pub(crate) fn new(
        kernel: Arc<ForgeKernel>,
        directives: mpsc::UnboundedReceiver<HostDirective>,
        owned_runtime: Option<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            kernel,
            directives: Mutex::new(directives),
            emitted_events: Mutex::new(Vec::new()),
            closed: Mutex::new(false),
            _owned_runtime: owned_runtime,
        }
    }

    pub fn config(&self) -> &Config {
        &self.kernel.config
    }

    pub async fn invoke<P, R>(&self, cmd: &str, payload: P) -> Result<R, ForgeError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let body = serde_json::to_vec(&payload).map_err(|e| {
            ForgeError::bad_request(
                "FORGE_INVALID_PAYLOAD",
                format!("failed to serialize payload: {e}"),
            )
        })?;
        let raw = self.invoke_bytes(cmd, body).await?;
        serde_json::from_slice::<R>(&raw).map_err(|e| {
            ForgeError::new(
                "FORGE_DESERIALIZE_ERROR",
                format!("failed to deserialize command response: {e}"),
            )
        })
    }

    pub async fn invoke_unit<R>(&self, cmd: &str) -> Result<R, ForgeError>
    where
        R: DeserializeOwned,
    {
        self.invoke(cmd, Value::Null).await
    }

    async fn invoke_bytes(&self, cmd: &str, body: Vec<u8>) -> Result<Vec<u8>, ForgeError> {
        let uri: Uri = format!("forge://app/__forge_ipc/{}", percent_encode_segment(cmd))
            .parse()
            .map_err(|e| ForgeError::bad_request("FORGE_INVALID_COMMAND", format!("{e}")))?;

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert(
            "x-forge-token",
            HeaderValue::from_str(&self.kernel.launch_token)
                .map_err(|e| ForgeError::new("APP_ERROR", e.to_string()))?,
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));

        let res = self
            .kernel
            .handle_request(ProtocolRequest {
                method: Method::POST,
                uri,
                headers,
                body,
            })
            .await;

        self.drain_directives();

        if res.status.is_success() {
            Ok(res.body)
        } else {
            let err = serde_json::from_slice::<ForgeError>(&res.body).unwrap_or_else(|_| {
                ForgeError::new(
                    "APP_ERROR",
                    format!("IPC failed with HTTP status {}", res.status),
                )
            });
            Err(err)
        }
    }

    pub async fn get_asset(&self, path: &str) -> RawResponse {
        let normalized = if path.starts_with('/') {
            format!("forge://app{path}")
        } else {
            format!("forge://app/{path}")
        };
        let uri: Uri = normalized
            .parse()
            .unwrap_or_else(|_| Uri::from_static("forge://app/"));
        let res = self
            .kernel
            .handle_request(ProtocolRequest {
                method: Method::GET,
                uri,
                headers: HeaderMap::new(),
                body: Vec::new(),
            })
            .await;
        self.drain_directives();
        RawResponse {
            status: res.status,
            headers: res.headers,
            body: res.body,
        }
    }

    pub async fn raw_exchange(&self, req: Request<Vec<u8>>) -> RawResponse {
        let (parts, body) = req.into_parts();
        let res = self
            .kernel
            .handle_request(ProtocolRequest {
                method: parts.method,
                uri: parts.uri,
                headers: parts.headers,
                body,
            })
            .await;
        self.drain_directives();
        RawResponse {
            status: res.status,
            headers: res.headers,
            body: res.body,
        }
    }

    /// Drains and returns all Rust-to-JS push events emitted via [`crate::WindowHandle::emit`].
    pub fn take_emitted_events(&self) -> Vec<(String, Value)> {
        self.drain_directives();
        self.emitted_events
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }

    pub fn window_title(&self) -> String {
        self.drain_directives();
        self.kernel.window_handle.snapshot_state().title
    }

    pub fn window_size(&self) -> (u32, u32) {
        self.drain_directives();
        self.kernel.window_handle.snapshot_state().size
    }

    pub fn is_minimized(&self) -> bool {
        self.drain_directives();
        self.kernel.window_handle.snapshot_state().minimized
    }

    pub fn is_maximized(&self) -> bool {
        self.drain_directives();
        self.kernel.window_handle.snapshot_state().maximized
    }

    pub fn is_fullscreen(&self) -> bool {
        self.drain_directives();
        self.kernel.window_handle.snapshot_state().fullscreen
    }

    pub fn is_closed(&self) -> bool {
        self.drain_directives();
        self.closed.lock().map(|g| *g).unwrap_or(false)
    }

    fn drain_directives(&self) {
        if let Ok(mut rx) = self.directives.lock() {
            while let Ok(dir) = rx.try_recv() {
                match dir {
                    HostDirective::EmitEvent {
                        event,
                        payload_json,
                    } => {
                        if let Ok(val) = serde_json::from_str::<Value>(&payload_json)
                            && let Ok(mut events) = self.emitted_events.lock()
                        {
                            events.push((event, val));
                        }
                    }
                    HostDirective::CloseWindow | HostDirective::QuitApp => {
                        if let Ok(mut closed) = self.closed.lock() {
                            *closed = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn percent_encode_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(&mut out, "%{b:02X}");
            }
        }
    }
    out
}
