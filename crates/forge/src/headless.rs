use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use http::header::HeaderValue;
use http::{HeaderMap, Method, Request, StatusCode, Uri};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::config::{Config, MenuConfig, TrayConfig};
use crate::error::{ErrorKind, ForgeError, codes};
use crate::kernel::ForgeKernel;
use crate::runtime::{
    AppHandle, CloseDisposition, HostCommand, HostEvent, ProtocolRequest, WindowOp,
};
use crate::single_instance::SecondInstancePayload;

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
    commands_rx: Mutex<mpsc::UnboundedReceiver<HostCommand>>,
    emitted_events: Mutex<Vec<(String, Value)>>,
    window_events: Mutex<HashMap<String, Vec<(String, Value)>>>,
    closed: Mutex<bool>,
    _owned_runtime: Option<tokio::runtime::Runtime>,
}

impl HeadlessApp {
    pub(crate) fn new(
        kernel: Arc<ForgeKernel>,
        commands_rx: mpsc::UnboundedReceiver<HostCommand>,
        owned_runtime: Option<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            kernel,
            commands_rx: Mutex::new(commands_rx),
            emitted_events: Mutex::new(Vec::new()),
            window_events: Mutex::new(HashMap::new()),
            closed: Mutex::new(false),
            _owned_runtime: owned_runtime,
        }
    }

    pub fn config(&self) -> &Config {
        &self.kernel.config
    }

    /// Returns any non-fatal permission diagnostics emitted during startup validation.
    pub fn permission_warnings(&self) -> &[String] {
        &self.kernel.permission_warnings
    }

    /// Returns an [`AppHandle`] bound to this headless instance.
    pub fn app_handle(&self) -> AppHandle {
        AppHandle::new(Arc::clone(&self.kernel.registry))
    }

    pub async fn invoke<P, R>(&self, cmd: &str, payload: P) -> Result<R, ForgeError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.invoke_as("main", cmd, payload).await
    }

    /// Invokes an async command as originating from a specific window `window_label`.
    pub async fn invoke_as<P, R>(
        &self,
        window_label: &str,
        cmd: &str,
        payload: P,
    ) -> Result<R, ForgeError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let body = serde_json::to_vec(&payload).map_err(|e| {
            ForgeError::bad_request(
                codes::INVALID_PAYLOAD,
                format!("failed to serialize payload: {e}"),
            )
        })?;
        let raw = self
            .invoke_ipc_raw(window_label, cmd, "application/json", None, body)
            .await?;
        serde_json::from_slice::<R>(&raw).map_err(|e| {
            ForgeError::new(
                codes::DESERIALIZE_ERROR,
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

    /// Invokes a binary command (`application/octet-stream` in -> `application/octet-stream` out) from `"main"`.
    pub async fn invoke_raw(
        &self,
        cmd: &str,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Vec<u8>, ForgeError> {
        self.invoke_raw_as("main", cmd, bytes).await
    }

    /// Invokes a binary command originating from `window_label`.
    pub async fn invoke_raw_as(
        &self,
        window_label: &str,
        cmd: &str,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Vec<u8>, ForgeError> {
        self.invoke_ipc_raw(
            window_label,
            cmd,
            "application/octet-stream",
            None,
            bytes.into(),
        )
        .await
    }

    /// Invokes a binary command with a structured `X-Forge-Meta` JSON header (`Meta<M>`) and decodes a JSON reply.
    pub async fn invoke_raw_with_meta<M, R>(
        &self,
        cmd: &str,
        meta: M,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<R, ForgeError>
    where
        M: Serialize,
        R: DeserializeOwned,
    {
        let meta_json = serde_json::to_string(&meta).map_err(|e| {
            ForgeError::bad_request(
                codes::INVALID_PAYLOAD,
                format!("failed to serialize meta header: {e}"),
            )
        })?;
        let encoded_meta = format!("uri:{}", percent_encode_segment(&meta_json));
        let raw = self
            .invoke_ipc_raw(
                "main",
                cmd,
                "application/octet-stream",
                Some(&encoded_meta),
                bytes.into(),
            )
            .await?;
        serde_json::from_slice::<R>(&raw).map_err(|e| {
            ForgeError::new(
                codes::DESERIALIZE_ERROR,
                format!("failed to deserialize command response: {e}"),
            )
        })
    }

    /// Invokes a command accepting a JSON payload (`P`) and returning raw binary bytes (`RawBytes`).
    pub async fn invoke_json_to_raw<P>(&self, cmd: &str, payload: P) -> Result<Vec<u8>, ForgeError>
    where
        P: Serialize,
    {
        let body = serde_json::to_vec(&payload).map_err(|e| {
            ForgeError::bad_request(
                codes::INVALID_PAYLOAD,
                format!("failed to serialize payload: {e}"),
            )
        })?;
        self.invoke_ipc_raw("main", cmd, "application/json", None, body)
            .await
    }

    async fn invoke_ipc_raw(
        &self,
        window_label: &str,
        cmd: &str,
        content_type: &'static str,
        x_forge_meta: Option<&str>,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, ForgeError> {
        let uri: Uri = format!(
            "forge://localhost/__forge_ipc/{}",
            percent_encode_segment(cmd)
        )
        .parse()
        .map_err(|e| ForgeError::bad_request(codes::INVALID_COMMAND, format!("{e}")))?;

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static(content_type));
        headers.insert(
            "x-forge-token",
            HeaderValue::from_str(&self.kernel.launch_token)
                .map_err(|e| ForgeError::internal(codes::APP_ERROR, e.to_string()))?,
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        if let Some(meta_val) = x_forge_meta {
            headers.insert(
                "x-forge-meta",
                HeaderValue::from_str(meta_val).map_err(|e| {
                    ForgeError::bad_request(codes::INVALID_PAYLOAD, format!("invalid meta: {e}"))
                })?,
            );
        }

        let res = self
            .kernel
            .handle_request(ProtocolRequest {
                window: window_label.to_string(),
                method: Method::POST,
                uri,
                headers,
                body,
            })
            .await;

        self.drain_commands();

        if res.status.is_success() {
            Ok(res.body)
        } else {
            let kind = ErrorKind::from_status(res.status);
            let err = serde_json::from_slice::<ForgeError>(&res.body)
                .map(|e| e.set_kind(kind))
                .unwrap_or_else(|_| {
                    ForgeError::with_kind(
                        kind,
                        codes::APP_ERROR,
                        format!("IPC failed with HTTP status {}", res.status),
                    )
                });
            Err(err)
        }
    }

    pub async fn get_asset(&self, path: &str) -> RawResponse {
        self.get_asset_for("main", path).await
    }

    pub async fn get_asset_for(&self, window_label: &str, path: &str) -> RawResponse {
        let normalized = if path.starts_with('/') {
            format!("forge://localhost{path}")
        } else {
            format!("forge://localhost/{path}")
        };
        let uri: Uri = normalized
            .parse()
            .unwrap_or_else(|_| Uri::from_static("forge://localhost/"));
        let res = self
            .kernel
            .handle_request(ProtocolRequest {
                window: window_label.to_string(),
                method: Method::GET,
                uri,
                headers: HeaderMap::new(),
                body: Vec::new(),
            })
            .await;
        self.drain_commands();
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
                window: "main".to_string(),
                method: parts.method,
                uri: parts.uri,
                headers: parts.headers,
                body,
            })
            .await;
        self.drain_commands();
        RawResponse {
            status: res.status,
            headers: res.headers,
            body: res.body,
        }
    }

    /// Drains and returns all Rust-to-JS push events emitted across windows.
    pub fn take_emitted_events(&self) -> Vec<(String, Value)> {
        self.drain_commands();
        self.emitted_events
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }

    /// Drains and returns Rust-to-JS push events delivered specifically to `window_label`.
    pub fn take_emitted_events_for(&self, window_label: &str) -> Vec<(String, Value)> {
        self.drain_commands();
        self.window_events
            .lock()
            .ok()
            .and_then(|mut map| map.remove(window_label))
            .unwrap_or_default()
    }

    pub fn windows(&self) -> Vec<String> {
        self.drain_commands();
        self.kernel.registry.window_labels()
    }

    pub fn tray(&self) -> Option<TrayConfig> {
        self.drain_commands();
        self.kernel.registry.tray()
    }

    pub fn menu(&self) -> Option<MenuConfig> {
        self.drain_commands();
        self.kernel.registry.menu()
    }

    /// Simulates a user clicking a menu item `id`, routing through the exact same registry
    /// dispatch path (`forge.*` native actions + `menu:click` broadcast) as a real OS menu click.
    pub fn click_menu(&self, id: &str) {
        let should_exit = self
            .kernel
            .registry
            .handle_host_event(HostEvent::MenuClicked { id: id.to_string() });
        if should_exit && let Ok(mut closed) = self.closed.lock() {
            *closed = true;
        }
        self.drain_commands();
    }

    /// Simulates a user clicking a system tray menu item `id`.
    pub fn click_tray(&self, id: &str) {
        let should_exit = self
            .kernel
            .registry
            .handle_host_event(HostEvent::TrayClicked { id: id.to_string() });
        if should_exit && let Ok(mut closed) = self.closed.lock() {
            *closed = true;
        }
        self.drain_commands();
    }

    pub fn window_title(&self) -> String {
        self.window_title_for("main").unwrap_or_default()
    }

    pub fn window_title_for(&self, label: &str) -> Option<String> {
        self.drain_commands();
        self.kernel
            .registry
            .window_handle(label)
            .map(|w| w.snapshot_state().title)
    }

    pub fn window_size(&self) -> (u32, u32) {
        self.drain_commands();
        self.kernel
            .registry
            .window_handle("main")
            .map(|w| w.snapshot_state().size)
            .unwrap_or((0, 0))
    }

    pub fn window_position(&self) -> Option<(i32, i32)> {
        self.drain_commands();
        self.kernel
            .registry
            .window_handle("main")
            .and_then(|w| w.snapshot_state().position)
    }

    pub fn is_minimized(&self) -> bool {
        self.drain_commands();
        self.kernel
            .registry
            .window_handle("main")
            .is_some_and(|w| w.snapshot_state().minimized)
    }

    pub fn is_maximized(&self) -> bool {
        self.drain_commands();
        self.kernel
            .registry
            .window_handle("main")
            .is_some_and(|w| w.snapshot_state().maximized)
    }

    /// Simulates a user clicking the native OS close button (`[X]`) on `window_label`.
    pub fn request_close(&self, window_label: &str) {
        if self.kernel.registry.request_close(window_label) == CloseDisposition::DestroyNow {
            let should_exit = self.kernel.registry.handle_host_event(HostEvent::Closed {
                label: window_label.to_string(),
            });
            if should_exit && let Ok(mut closed) = self.closed.lock() {
                *closed = true;
            }
        }
        self.drain_commands();
    }

    /// Simulates pressing a registered global shortcut `accelerator`.
    pub fn trigger_shortcut(&self, accelerator: &str) {
        let _ = self
            .kernel
            .registry
            .handle_host_event(HostEvent::ShortcutTriggered {
                accelerator: accelerator.to_string(),
            });
        self.drain_commands();
    }

    /// Simulates a macOS/OS URL open event (`tao::event::Event::Opened { urls }`).
    pub fn simulate_urls_opened(&self, urls: Vec<String>) {
        let _ = self
            .kernel
            .registry
            .handle_host_event(HostEvent::UrlsOpened { urls });
        self.drain_commands();
    }

    /// Simulates a secondary process launch forwarding CLI `args` and `cwd` over `SingleInstanceGuard`.
    pub fn simulate_second_instance(&self, args: Vec<String>, cwd: std::path::PathBuf) {
        let payload = SecondInstancePayload { args, cwd };
        let app_handle = self.app_handle();
        if let Some(main_win) = app_handle.window("main") {
            main_win.show();
            main_win.focus();
        }
        self.kernel.registry.dispatch_second_instance(&payload);
        let _ = app_handle.emit_all("app:second-instance", payload);
        self.drain_commands();
    }

    pub fn is_visible_for(&self, label: &str) -> bool {
        self.drain_commands();
        self.kernel
            .registry
            .window_handle(label)
            .is_some_and(|w| w.snapshot_state().visible)
    }

    pub fn is_always_on_top(&self) -> bool {
        self.is_always_on_top_for("main")
    }

    pub fn is_always_on_top_for(&self, label: &str) -> bool {
        self.drain_commands();
        self.kernel
            .registry
            .window_handle(label)
            .is_some_and(|w| w.snapshot_state().always_on_top)
    }

    pub fn is_fullscreen(&self) -> bool {
        self.drain_commands();
        self.kernel
            .registry
            .window_handle("main")
            .is_some_and(|w| w.snapshot_state().fullscreen)
    }

    pub fn is_closed(&self) -> bool {
        self.drain_commands();
        self.closed.lock().map(|g| *g).unwrap_or(false)
    }

    fn drain_commands(&self) {
        let mut pending = Vec::new();
        if let Ok(mut rx) = self.commands_rx.lock() {
            while let Ok(cmd) = rx.try_recv() {
                pending.push(cmd);
            }
        }
        for cmd in pending {
            match cmd {
                HostCommand::Window {
                    label,
                    op:
                        WindowOp::Emit {
                            event,
                            payload_json,
                        },
                } => {
                    if let Ok(val) = serde_json::from_str::<Value>(&payload_json) {
                        if let Ok(mut global) = self.emitted_events.lock() {
                            global.push((event.clone(), val.clone()));
                        }
                        if let Ok(mut per_win) = self.window_events.lock() {
                            per_win.entry(label).or_default().push((event, val));
                        }
                    }
                }
                HostCommand::Window {
                    label,
                    op: WindowOp::Close,
                } => {
                    if self.kernel.registry.request_close(&label) == CloseDisposition::DestroyNow {
                        let should_exit = self
                            .kernel
                            .registry
                            .handle_host_event(HostEvent::Closed { label });
                        if should_exit && let Ok(mut closed) = self.closed.lock() {
                            *closed = true;
                        }
                    }
                }
                HostCommand::Window {
                    label,
                    op: WindowOp::Destroy,
                } => {
                    let should_exit = self
                        .kernel
                        .registry
                        .handle_host_event(HostEvent::Closed { label });
                    if should_exit && let Ok(mut closed) = self.closed.lock() {
                        *closed = true;
                    }
                }
                HostCommand::Quit => {
                    if let Ok(mut closed) = self.closed.lock() {
                        *closed = true;
                    }
                }
                _ => {}
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
