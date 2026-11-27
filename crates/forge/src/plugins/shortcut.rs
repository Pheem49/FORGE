use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use global_hotkey::hotkey::HotKey;
use serde::{Deserialize, Serialize};

use crate::config::RuntimeKind;
use crate::error::{ForgeError, codes};
use crate::runtime::{AppHandle, HostCommand};
use crate::{App, Plugin, State};

type ShortcutCallback = Arc<dyn Fn(AppHandle, &str) + Send + Sync + 'static>;

#[derive(Debug, Deserialize, Serialize)]
pub struct ShortcutReq {
    pub accelerator: String,
}

/// Seam for registering and unregistering OS global keyboard shortcuts.
pub trait ShortcutBackend: Send + Sync + 'static {
    fn register(&self, app: &AppHandle, accelerator: &str) -> Result<(), ForgeError>;
    fn unregister(&self, app: &AppHandle, accelerator: &str) -> Result<(), ForgeError>;
    fn unregister_all(&self, app: &AppHandle) -> Result<(), ForgeError>;
}

/// Default desktop shortcut backend dispatching to the main-thread `WebViewHost` hotkey manager.
#[derive(Default)]
pub struct HostShortcutBackend;

impl ShortcutBackend for HostShortcutBackend {
    fn register(&self, app: &AppHandle, accelerator: &str) -> Result<(), ForgeError> {
        app.registry
            .send_host_command(HostCommand::RegisterShortcut(accelerator.to_string()));
        Ok(())
    }

    fn unregister(&self, app: &AppHandle, accelerator: &str) -> Result<(), ForgeError> {
        app.registry
            .send_host_command(HostCommand::UnregisterShortcut(accelerator.to_string()));
        Ok(())
    }

    fn unregister_all(&self, app: &AppHandle) -> Result<(), ForgeError> {
        app.registry
            .send_host_command(HostCommand::UnregisterAllShortcuts);
        Ok(())
    }
}

struct ShortcutState {
    backend: Arc<dyn ShortcutBackend>,
    /// Keyed by deterministic `HotKey::id()` (`u32`) so `"ctrl+k"` and `"Ctrl+K"` normalize to the same entry.
    registered: Mutex<BTreeMap<u32, String>>,
}

/// Global OS keyboard shortcut plugin (`shortcut.register`, `shortcut.unregister`,
/// `shortcut.unregister_all`, `shortcut.list`), broadcasting `shortcut:triggered` (`{ accelerator }`).
pub struct GlobalShortcutPlugin {
    backend: Arc<dyn ShortcutBackend>,
    initial_shortcuts: Vec<String>,
    callback: Option<ShortcutCallback>,
}

impl Default for GlobalShortcutPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalShortcutPlugin {
    pub fn new() -> Self {
        Self {
            backend: Arc::new(HostShortcutBackend),
            initial_shortcuts: Vec::new(),
            callback: None,
        }
    }

    pub fn with_backend(mut self, backend: impl ShortcutBackend) -> Self {
        self.backend = Arc::new(backend);
        self
    }

    /// Registers an initial shortcut at startup (validated with [`HotKey`]).
    pub fn with_shortcut(mut self, accelerator: impl Into<String>) -> Self {
        self.initial_shortcuts.push(accelerator.into());
        self
    }

    /// Attaches an optional Rust handler invoked whenever any registered shortcut fires.
    pub fn on_shortcut<F>(mut self, handler: F) -> Self
    where
        F: Fn(AppHandle, &str) + Send + Sync + 'static,
    {
        self.callback = Some(Arc::new(handler));
        self
    }
}

impl Plugin for GlobalShortcutPlugin {
    fn apply(self, app: App) -> App {
        let backend = self.backend;
        let initial = self.initial_shortcuts;
        let cb = self.callback;

        app.setup(move |ctx| {
            let app_handle = ctx.app_handle();
            if !initial.is_empty()
                && app_handle.registry.runtime_kind() == RuntimeKind::Cef
                && !app_handle.registry.is_headless()
            {
                return Err(crate::Error::SetupFailed(
                    "global shortcuts are not supported on the CEF runtime (use runtime = \"webview\")"
                        .into(),
                ));
            }

            let state = ShortcutState {
                backend,
                registered: Mutex::new(BTreeMap::new()),
            };

            if let Some(callback) = cb {
                let handle_for_hook = app_handle.clone();
                app_handle
                    .registry
                    .register_shortcut_hook(Arc::new(move |accel| {
                        callback(handle_for_hook.clone(), accel);
                    }));
            }

            for raw in initial {
                let trimmed = raw.trim().to_string();
                let hk: HotKey = trimmed.parse().map_err(|e| {
                    crate::Error::SetupFailed(format!("invalid global shortcut '{trimmed}': {e}"))
                })?;
                state
                    .backend
                    .register(&app_handle, &trimmed)
                    .map_err(|e| crate::Error::SetupFailed(e.message))?;
                if let Ok(mut map) = state.registered.lock() {
                    map.insert(hk.id(), trimmed);
                }
            }

            ctx.insert_state(state);
            Ok(())
        })
        .command("shortcut.register", shortcut_register)
        .command("shortcut.unregister", shortcut_unregister)
        .command("shortcut.unregister_all", shortcut_unregister_all)
        .command("shortcut.list", shortcut_list)
    }
}

fn ensure_shortcut_supported(app: &AppHandle) -> Result<(), ForgeError> {
    if app.registry.runtime_kind() == RuntimeKind::Cef && !app.registry.is_headless() {
        return Err(ForgeError::unsupported(
            "global shortcuts are not supported on the CEF runtime (use runtime = \"webview\")",
        ));
    }
    Ok(())
}

async fn shortcut_register(
    app: AppHandle,
    state: State<ShortcutState>,
    req: ShortcutReq,
) -> Result<(), ForgeError> {
    ensure_shortcut_supported(&app)?;
    let trimmed = req.accelerator.trim().to_string();
    let hk: HotKey = trimmed.parse().map_err(|e| {
        ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            format!("invalid shortcut accelerator '{trimmed}': {e}"),
        )
    })?;

    state.backend.register(&app, &trimmed)?;
    if let Ok(mut map) = state.registered.lock() {
        map.insert(hk.id(), trimmed);
    }
    Ok(())
}

async fn shortcut_unregister(
    app: AppHandle,
    state: State<ShortcutState>,
    req: ShortcutReq,
) -> Result<bool, ForgeError> {
    ensure_shortcut_supported(&app)?;
    let trimmed = req.accelerator.trim().to_string();
    let hk: HotKey = trimmed.parse().map_err(|e| {
        ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            format!("invalid shortcut accelerator '{trimmed}': {e}"),
        )
    })?;

    let removed = state
        .registered
        .lock()
        .ok()
        .and_then(|mut map| map.remove(&hk.id()));
    if let Some(canonical) = removed {
        state.backend.unregister(&app, &canonical)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn shortcut_unregister_all(
    app: AppHandle,
    state: State<ShortcutState>,
) -> Result<(), ForgeError> {
    ensure_shortcut_supported(&app)?;
    state.backend.unregister_all(&app)?;
    if let Ok(mut map) = state.registered.lock() {
        map.clear();
    }
    Ok(())
}

async fn shortcut_list(
    app: AppHandle,
    state: State<ShortcutState>,
) -> Result<Vec<String>, ForgeError> {
    ensure_shortcut_supported(&app)?;
    let mut list: Vec<String> = state
        .registered
        .lock()
        .map(|map| map.values().cloned().collect())
        .unwrap_or_default();
    list.sort();
    Ok(list)
}
