use std::sync::{Arc, Mutex};

use http::{HeaderMap, Method, StatusCode, Uri};
use tokio::sync::mpsc;

use crate::config::RuntimeKind;
#[cfg(not(all(feature = "webview", feature = "cef")))]
use crate::error::Error;
use crate::error::Result;
use crate::kernel::ForgeKernel;

#[cfg(feature = "webview")]
pub(crate) mod webview;

#[cfg(feature = "cef")]
pub(crate) mod cef;

/// Directives sent from async Rust handlers to the host windowing thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostDirective {
    SetTitle(String),
    CloseWindow,
    QuitApp,
}

/// Thread-safe handle (`Clone + Send + Sync`) allowing async command handlers to mutate
/// or close the native application window without touching GUI pointers off-thread.
#[derive(Clone)]
pub struct WindowHandle {
    tx: mpsc::UnboundedSender<HostDirective>,
    observed_title: Arc<Mutex<String>>,
}

impl WindowHandle {
    pub(crate) fn new(initial_title: String) -> (Self, mpsc::UnboundedReceiver<HostDirective>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = Self {
            tx,
            observed_title: Arc::new(Mutex::new(initial_title)),
        };
        (handle, rx)
    }

    /// Updates the native window title asynchronously.
    pub fn set_title(&self, title: impl Into<String>) {
        let title = title.into();
        if let Ok(mut guard) = self.observed_title.lock() {
            *guard = title.clone();
        }
        let _ = self.tx.send(HostDirective::SetTitle(title));
    }

    /// Requests closing the main window cleanly.
    pub fn close_window(&self) {
        let _ = self.tx.send(HostDirective::CloseWindow);
    }

    /// Requests terminating the desktop application loop cleanly.
    pub fn quit_app(&self) {
        let _ = self.tx.send(HostDirective::QuitApp);
    }

    pub(crate) fn current_title(&self) -> String {
        self.observed_title
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
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
/// Deliberately `pub(crate)` so internal runtime mechanics never leak into caller `main.rs`.
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

/// Phase 1 gate executed at the very start of [`crate::App::run`] BEFORE starting Tokio
/// or executing `.setup(...)` hooks, ensuring CEF child processes exit cleanly.
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

/// Phase 2 host execution blocking the OS main thread until window close.
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
