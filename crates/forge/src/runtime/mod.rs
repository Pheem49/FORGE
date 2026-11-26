use std::sync::Arc;

use http::{HeaderMap, Method, StatusCode, Uri};
use tokio::sync::mpsc;

use crate::config::RuntimeKind;
#[cfg(not(all(feature = "webview", feature = "cef")))]
use crate::error::Error;
use crate::error::Result;
use crate::kernel::ForgeKernel;

pub(crate) mod windows;

#[cfg(feature = "webview")]
pub(crate) mod webview;

#[cfg(feature = "cef")]
pub(crate) mod cef;

#[cfg(any(feature = "webview", feature = "cef"))]
pub(crate) use windows::WindowSpec;
pub use windows::{AppHandle, WindowHandle, WindowOptions};
pub(crate) use windows::{HostCommand, HostEvent, WindowOp, WindowRegistry};

/// Engine-neutral HTTP-like request passed across the internal runtime seam.
#[derive(Debug)]
pub(crate) struct ProtocolRequest {
    pub window: String,
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
        commands: mpsc::UnboundedReceiver<HostCommand>,
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
    commands: mpsc::UnboundedReceiver<HostCommand>,
) -> Result<()> {
    match kind {
        RuntimeKind::Webview => {
            #[cfg(feature = "webview")]
            {
                webview::WebViewHost::run_host(kernel, commands)
            }
            #[cfg(not(feature = "webview"))]
            {
                let _ = (kernel, commands);
                Err(Error::RuntimeUnavailable {
                    requested: RuntimeKind::Webview,
                    reason: "Cargo feature 'forge/webview' is not enabled".into(),
                })
            }
        }
        RuntimeKind::Cef => {
            #[cfg(feature = "cef")]
            {
                cef::CefHost::run_host(kernel, commands)
            }
            #[cfg(not(feature = "cef"))]
            {
                let _ = (kernel, commands);
                Err(Error::RuntimeUnavailable {
                    requested: RuntimeKind::Cef,
                    reason: "Cargo feature 'forge/cef' is not enabled".into(),
                })
            }
        }
    }
}
