use std::sync::{Arc, Mutex};

use http::header::HeaderValue;
use http::{HeaderMap, Method, Request, StatusCode, Uri};
use serde::Serialize;
use serde::de::DeserializeOwned;
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
/// Exercises the exact same configuration, setup hooks, `State<T>` map, static asset security,
/// bootstrap injection, and async command router as desktop runtimes—without requiring X11,
/// Wayland, Cocoa, WebView2, or Chromium binaries.
pub struct HeadlessApp {
    kernel: Arc<ForgeKernel>,
    directives: Mutex<mpsc::UnboundedReceiver<HostDirective>>,
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
            closed: Mutex::new(false),
            _owned_runtime: owned_runtime,
        }
    }

    /// Returns a reference to the resolved [`Config`].
    pub fn config(&self) -> &Config {
        &self.kernel.config
    }

    /// Invokes a registered async command with automatic launch-token authentication,
    /// mirroring `await window.forge.invoke(cmd, payload)` in JavaScript.
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

    /// Invokes a zero-argument command (`await window.forge.invoke(cmd)`).
    pub async fn invoke_unit<R>(&self, cmd: &str) -> Result<R, ForgeError>
    where
        R: DeserializeOwned,
    {
        self.invoke(cmd, serde_json::Value::Null).await
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

    /// Performs a `GET` request against the static asset / bootstrap exchange (e.g. `"/"` or `"/index.html"`).
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

    /// Sends an arbitrary raw [`http::Request`] into the exchange—useful for security tests
    /// verifying CSRF rejection when `X-Forge-Token` or `Sec-Fetch-Site` is forged.
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

    /// Returns the most recent window title (reflecting any [`crate::WindowHandle::set_title`] calls).
    pub fn window_title(&self) -> String {
        self.drain_directives();
        self.kernel.window_handle.current_title()
    }

    /// Returns `true` if a handler called [`crate::WindowHandle::close_window`] or [`crate::WindowHandle::quit_app`].
    pub fn is_closed(&self) -> bool {
        self.drain_directives();
        self.closed.lock().map(|g| *g).unwrap_or(false)
    }

    fn drain_directives(&self) {
        if let Ok(mut rx) = self.directives.lock() {
            while let Ok(dir) = rx.try_recv() {
                match dir {
                    HostDirective::SetTitle(_) => {}
                    HostDirective::CloseWindow | HostDirective::QuitApp => {
                        if let Ok(mut closed) = self.closed.lock() {
                            *closed = true;
                        }
                    }
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
