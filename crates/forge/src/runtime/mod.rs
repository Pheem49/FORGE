use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use http::{HeaderMap, Method, StatusCode, Uri};
use serde::Serialize;
use tokio::sync::mpsc;

use crate::config::RuntimeKind;
#[cfg(not(all(feature = "webview", feature = "cef")))]
use crate::error::Error;
use crate::error::{ForgeError, Result};
use crate::kernel::ForgeKernel;

#[cfg(feature = "webview")]
pub(crate) mod webview;

#[cfg(feature = "cef")]
pub(crate) mod cef;

const MAX_EARLY_EVENT_BUFFER: usize = 64;

/// Directives sent from async Rust handlers or background tasks to the host windowing thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostDirective {
    SetTitle(String),
    SetSize { width: u32, height: u32 },
    SetMinimized(bool),
    SetMaximized(bool),
    SetFullscreen(bool),
    DragWindow,
    EmitEvent { event: String, payload_json: String },
    CloseWindow,
    QuitApp,
}

#[derive(Debug, Clone)]
pub(crate) struct ObservedWindowState {
    pub title: String,
    pub size: (u32, u32),
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub early_events: VecDeque<(String, String)>,
    pub js_ready: bool,
}

/// Thread-safe handle (`Clone + Send + Sync`) allowing async command handlers and background
/// tasks to push events to JavaScript and control the native application window.
#[derive(Clone)]
pub struct WindowHandle {
    tx: mpsc::UnboundedSender<HostDirective>,
    state: Arc<Mutex<ObservedWindowState>>,
}

impl WindowHandle {
    pub(crate) fn new(
        initial_title: String,
        initial_size: (u32, u32),
    ) -> (Self, mpsc::UnboundedReceiver<HostDirective>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = Self {
            tx,
            state: Arc::new(Mutex::new(ObservedWindowState {
                title: initial_title,
                size: initial_size,
                minimized: false,
                maximized: false,
                fullscreen: false,
                early_events: VecDeque::new(),
                js_ready: false,
            })),
        };
        (handle, rx)
    }

    /// Pushes an asynchronous event to JavaScript listeners registered via
    /// `window.forge.listen(event, callback)`.
    pub fn emit<T: Serialize>(
        &self,
        event: impl Into<String>,
        payload: T,
    ) -> std::result::Result<(), ForgeError> {
        let event = event.into();
        let payload_json = serde_json::to_string(&payload).map_err(|e| {
            ForgeError::new(
                "FORGE_SERIALIZE_ERROR",
                format!("failed to serialize event payload for '{event}': {e}"),
            )
        })?;

        if let Ok(mut guard) = self.state.lock()
            && !guard.js_ready
        {
            if guard.early_events.len() >= MAX_EARLY_EVENT_BUFFER {
                guard.early_events.pop_front();
            }
            guard
                .early_events
                .push_back((event.clone(), payload_json.clone()));
        }

        let _ = self.tx.send(HostDirective::EmitEvent {
            event,
            payload_json,
        });
        Ok(())
    }

    /// Updates the native window title asynchronously.
    pub fn set_title(&self, title: impl Into<String>) {
        let title = title.into();
        if let Ok(mut guard) = self.state.lock() {
            guard.title = title.clone();
        }
        let _ = self.tx.send(HostDirective::SetTitle(title));
    }

    /// Resizes the window's logical inner dimensions.
    pub fn set_size(&self, width: u32, height: u32) {
        if let Ok(mut guard) = self.state.lock() {
            guard.size = (width, height);
        }
        let _ = self.tx.send(HostDirective::SetSize { width, height });
    }

    /// Minimizes or unminimizes the window.
    pub fn set_minimized(&self, minimized: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.minimized = minimized;
        }
        let _ = self.tx.send(HostDirective::SetMinimized(minimized));
    }

    /// Maximizes or restores the window.
    pub fn set_maximized(&self, maximized: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.maximized = maximized;
        }
        let _ = self.tx.send(HostDirective::SetMaximized(maximized));
    }

    /// Enters or exits fullscreen mode.
    pub fn set_fullscreen(&self, fullscreen: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.fullscreen = fullscreen;
        }
        let _ = self.tx.send(HostDirective::SetFullscreen(fullscreen));
    }

    /// Initiates an interactive OS window drag (useful for custom frameless titlebars).
    pub fn drag_window(&self) {
        let _ = self.tx.send(HostDirective::DragWindow);
    }

    /// Requests closing the main window cleanly.
    pub fn close_window(&self) {
        let _ = self.tx.send(HostDirective::CloseWindow);
    }

    /// Requests terminating the desktop application loop cleanly.
    pub fn quit_app(&self) {
        let _ = self.tx.send(HostDirective::QuitApp);
    }

    pub(crate) fn snapshot_state(&self) -> ObservedWindowState {
        self.state
            .lock()
            .map(|g| g.clone())
            .unwrap_or(ObservedWindowState {
                title: String::new(),
                size: (0, 0),
                minimized: false,
                maximized: false,
                fullscreen: false,
                early_events: VecDeque::new(),
                js_ready: false,
            })
    }

    pub(crate) fn mark_js_ready_and_drain_early(&self) -> Vec<(String, serde_json::Value)> {
        let Ok(mut guard) = self.state.lock() else {
            return Vec::new();
        };
        guard.js_ready = true;
        guard
            .early_events
            .drain(..)
            .filter_map(|(ev, raw)| serde_json::from_str(&raw).ok().map(|v| (ev, v)))
            .collect()
    }
}

/// Engine-neutral HTTP-like request passed across the internal runtime seam.
#[derive(Debug)]
pub(crate) struct ProtocolRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Engine-neutral HTTP-like response produced by [`ForgeKernel`].
#[derive(Debug)]
pub(crate) struct ProtocolResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Outcome of the Phase 1 subprocess pre-initialization gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessDisposition {
    ContinueAsHost,
    #[allow(dead_code)]
    SubprocessExited(i32),
}

/// Internal seam satisfied by concrete desktop engine adapters (`WebViewHost`, `CefHost`).
#[allow(dead_code)]
pub(crate) trait RuntimeHost {
    fn enter_process() -> Result<ProcessDisposition> {
        Ok(ProcessDisposition::ContinueAsHost)
    }

    fn run_host(
        kernel: Arc<ForgeKernel>,
        directives: mpsc::UnboundedReceiver<HostDirective>,
    ) -> Result<()>;
}

#[cfg(any(feature = "webview", feature = "cef"))]
pub(crate) fn emit_script(event: &str, payload_json: &str) -> Option<String> {
    serde_json::to_string(event).ok().map(|event_js| {
        format!(
            "if (window.__forge_dispatch) {{ window.__forge_dispatch({event_js}, {payload_json}); }}"
        )
    })
}

pub(crate) fn enter_process(kind: RuntimeKind) -> Result<ProcessDisposition> {
    match kind {
        RuntimeKind::Webview => {
            #[cfg(feature = "webview")]
            {
                webview::WebViewHost::enter_process()
            }
            #[cfg(not(feature = "webview"))]
            {
                Err(Error::RuntimeUnavailable {
                    requested: RuntimeKind::Webview,
                    reason: "Cargo feature 'forge/webview' is not enabled".into(),
                })
            }
        }
        RuntimeKind::Cef => {
            #[cfg(feature = "cef")]
            {
                cef::CefHost::enter_process()
            }
            #[cfg(not(feature = "cef"))]
            {
                Err(Error::RuntimeUnavailable {
                    requested: RuntimeKind::Cef,
                    reason: "Cargo feature 'forge/cef' is not enabled (run via `forge dev --runtime cef` or pass `--features forge/cef`)".into(),
                })
            }
        }
    }
}

pub(crate) fn run_host(
    kind: RuntimeKind,
    kernel: Arc<ForgeKernel>,
    directives: mpsc::UnboundedReceiver<HostDirective>,
) -> Result<()> {
    match kind {
        RuntimeKind::Webview => {
            #[cfg(feature = "webview")]
            {
                webview::WebViewHost::run_host(kernel, directives)
            }
            #[cfg(not(feature = "webview"))]
            {
                let _ = (kernel, directives);
                Err(Error::RuntimeUnavailable {
                    requested: RuntimeKind::Webview,
                    reason: "Cargo feature 'forge/webview' is not enabled".into(),
                })
            }
        }
        RuntimeKind::Cef => {
            #[cfg(feature = "cef")]
            {
                cef::CefHost::run_host(kernel, directives)
            }
            #[cfg(not(feature = "cef"))]
            {
                let _ = (kernel, directives);
                Err(Error::RuntimeUnavailable {
                    requested: RuntimeKind::Cef,
                    reason: "Cargo feature 'forge/cef' is not enabled".into(),
                })
            }
        }
    }
}
