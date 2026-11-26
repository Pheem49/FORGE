use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;

use crate::config::{Config, MenuConfig, RuntimeKind, TrayConfig, WindowConfig};
use crate::error::{ForgeError, codes};
use crate::window_state::{SavedWindowGeometry, WindowStateStore};

const MAX_EARLY_EVENT_BUFFER: usize = 64;

/// Builder for opening additional windows dynamically via [`AppHandle::create_window`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowOptions {
    pub url: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub resizable: bool,
    pub decorations: bool,
    pub visible: bool,
    pub devtools: bool,
    pub remember_state: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self::from_window_config(&WindowConfig::default())
    }
}

impl WindowOptions {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }

    pub fn from_window_config(cfg: &WindowConfig) -> Self {
        Self {
            url: cfg.url.clone(),
            title: cfg.title.clone(),
            width: cfg.width,
            height: cfg.height,
            min_width: cfg.min_width,
            min_height: cfg.min_height,
            x: cfg.x,
            y: cfg.y,
            resizable: cfg.resizable,
            decorations: cfg.decorations,
            visible: cfg.visible,
            devtools: cfg.devtools,
            remember_state: cfg.remember_state,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn min_size(mut self, min_width: u32, min_height: u32) -> Self {
        self.min_width = Some(min_width);
        self.min_height = Some(min_height);
        self
    }

    pub fn position(mut self, x: i32, y: i32) -> Self {
        self.x = Some(x);
        self.y = Some(y);
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn decorations(mut self, decorations: bool) -> Self {
        self.decorations = decorations;
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn devtools(mut self, devtools: bool) -> Self {
        self.devtools = devtools;
        self
    }

    pub fn remember_state(mut self, remember: bool) -> Self {
        self.remember_state = remember;
        self
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct WindowSpec {
    pub url: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub resizable: bool,
    pub decorations: bool,
    pub visible: bool,
    pub maximized: bool,
    pub devtools: bool,
    pub remember_state: bool,
}

impl WindowSpec {
    pub(crate) fn from_options(opts: WindowOptions, saved: Option<SavedWindowGeometry>) -> Self {
        let mut spec = Self {
            url: opts.url,
            title: opts.title,
            width: opts.width,
            height: opts.height,
            min_width: opts.min_width,
            min_height: opts.min_height,
            x: opts.x,
            y: opts.y,
            resizable: opts.resizable,
            decorations: opts.decorations,
            visible: opts.visible,
            maximized: false,
            devtools: opts.devtools,
            remember_state: opts.remember_state,
        };
        if let Some(geom) = saved {
            spec.width = geom.width;
            spec.height = geom.height;
            if geom.x.is_some() {
                spec.x = geom.x;
            }
            if geom.y.is_some() {
                spec.y = geom.y;
            }
            spec.maximized = geom.maximized;
        }
        spec
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WindowOp {
    SetTitle(String),
    SetSize { width: u32, height: u32 },
    SetPosition { x: i32, y: i32 },
    Minimize(bool),
    Maximize(bool),
    Fullscreen(bool),
    Show,
    Hide,
    Focus,
    Drag,
    Emit { event: String, payload_json: String },
    Close,
}

/// Commands dispatched from [`WindowRegistry`] to the active [`super::RuntimeHost`] adapter.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum HostCommand {
    Window { label: String, op: WindowOp },
    CreateWindow { label: String, spec: WindowSpec },
    SetTray(TrayConfig),
    SetMenu(MenuConfig),
    Quit,
}

/// Events reported back from the [`super::RuntimeHost`] adapter into [`WindowRegistry`].
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum HostEvent {
    Resized {
        label: String,
        width: u32,
        height: u32,
    },
    Moved {
        label: String,
        x: i32,
        y: i32,
    },
    Focused {
        label: String,
        focused: bool,
    },
    Maximized {
        label: String,
        maximized: bool,
    },
    Closed {
        label: String,
    },
    MenuClicked {
        id: String,
    },
    TrayClicked {
        id: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ObservedWindowState {
    pub spec: WindowSpec,
    pub title: String,
    pub size: (u32, u32),
    pub position: Option<(i32, i32)>,
    pub visible: bool,
    pub focused: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub early_events: VecDeque<(String, String)>,
    pub js_ready: bool,
}

impl ObservedWindowState {
    fn from_spec(spec: WindowSpec) -> Self {
        let title = spec.title.clone();
        let size = (spec.width, spec.height);
        let position = spec.x.zip(spec.y);
        let visible = spec.visible;
        let maximized = spec.maximized;
        Self {
            spec,
            title,
            size,
            position,
            visible,
            focused: false,
            minimized: false,
            maximized,
            fullscreen: false,
            early_events: VecDeque::new(),
            js_ready: false,
        }
    }

    fn to_saved_geometry(&self) -> SavedWindowGeometry {
        SavedWindowGeometry {
            x: self.position.map(|(x, _)| x),
            y: self.position.map(|(_, y)| y),
            width: self.size.0,
            height: self.size.1,
            maximized: self.maximized,
        }
    }
}

struct RegistryInner {
    windows: HashMap<String, Arc<Mutex<ObservedWindowState>>>,
    configured_templates: HashMap<String, WindowConfig>,
    tray: Option<TrayConfig>,
    menu: Option<MenuConfig>,
    runtime_kind: RuntimeKind,
    is_headless: bool,
    exit_on_last_close: bool,
    state_store: Option<WindowStateStore>,
}

/// Central in-process window and desktop UI coordinator shared by [`ForgeKernel`], [`AppHandle`],
/// and [`WindowHandle`].
pub(crate) struct WindowRegistry {
    inner: Mutex<RegistryInner>,
    tx: mpsc::UnboundedSender<HostCommand>,
}

impl WindowRegistry {
    pub(crate) fn new(
        config: &Config,
        is_headless: bool,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<HostCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let state_store = WindowStateStore::for_identifier(
            &config.app.identifier,
            config.explicit_state_dir.as_deref(),
        );

        let mut windows = HashMap::new();
        let mut startup_specs = Vec::new();

        if config.window.create_on_startup {
            let saved = if config.window.remember_state {
                state_store.as_ref().and_then(|s| s.get("main"))
            } else {
                None
            };
            let spec =
                WindowSpec::from_options(WindowOptions::from_window_config(&config.window), saved);
            startup_specs.push(("main".to_string(), spec.clone()));
            windows.insert(
                "main".to_string(),
                Arc::new(Mutex::new(ObservedWindowState::from_spec(spec))),
            );
        }

        let mut configured_templates = HashMap::new();
        for (label, wcfg) in &config.windows {
            configured_templates.insert(label.clone(), wcfg.clone());
            if wcfg.create_on_startup && label != "main" {
                let saved = if wcfg.remember_state {
                    state_store.as_ref().and_then(|s| s.get(label))
                } else {
                    None
                };
                let spec = WindowSpec::from_options(WindowOptions::from_window_config(wcfg), saved);
                startup_specs.push((label.clone(), spec.clone()));
                windows.insert(
                    label.clone(),
                    Arc::new(Mutex::new(ObservedWindowState::from_spec(spec))),
                );
            }
        }

        let tray = config.tray.clone();
        let menu = config.menu.clone();

        let registry = Arc::new(Self {
            inner: Mutex::new(RegistryInner {
                windows,
                configured_templates,
                tray,
                menu,
                runtime_kind: config.runtime.default,
                is_headless,
                exit_on_last_close: config.app.exit_on_last_window_close,
                state_store,
            }),
            tx,
        });

        let _ = startup_specs;
        (registry, rx)
    }

    #[allow(dead_code)]
    pub(crate) fn startup_windows(&self) -> Vec<(String, WindowSpec)> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        let mut list: Vec<(String, WindowSpec)> = guard
            .windows
            .iter()
            .filter_map(|(label, st)| st.lock().ok().map(|s| (label.clone(), s.spec.clone())))
            .collect();
        // Ensure `"main"` initializes first when present.
        list.sort_by(|(a, _), (b, _)| match (a.as_str(), b.as_str()) {
            ("main", "main") => std::cmp::Ordering::Equal,
            ("main", _) => std::cmp::Ordering::Less,
            (_, "main") => std::cmp::Ordering::Greater,
            _ => a.cmp(b),
        });
        list
    }

    pub(crate) fn configured_template(&self, label: &str) -> Option<WindowConfig> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.configured_templates.get(label).cloned())
    }

    pub(crate) fn window_handle(self: &Arc<Self>, label: &str) -> Option<WindowHandle> {
        let state = self
            .inner
            .lock()
            .ok()
            .and_then(|g| g.windows.get(label).cloned())?;
        Some(WindowHandle {
            label: Arc::from(label),
            registry: Arc::clone(self),
            state,
        })
    }

    pub(crate) fn window_labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self
            .inner
            .lock()
            .map(|g| g.windows.keys().cloned().collect())
            .unwrap_or_default();
        labels.sort();
        labels
    }

    pub(crate) fn create_window(
        self: &Arc<Self>,
        label: String,
        opts: WindowOptions,
    ) -> Result<WindowHandle, ForgeError> {
        let trimmed = label.trim();
        if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('\0') {
            return Err(ForgeError::bad_request(
                codes::INVALID_PAYLOAD,
                "window label must be a non-empty identifier without '/'",
            ));
        }

        let (state_arc, spec) = {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| ForgeError::internal(codes::APP_ERROR, "window registry poisoned"))?;
            if guard.windows.contains_key(trimmed) {
                return Err(ForgeError::bad_request(
                    codes::DUPLICATE_WINDOW,
                    format!("window '{trimmed}' already exists"),
                ));
            }
            let saved = if opts.remember_state {
                guard.state_store.as_ref().and_then(|s| s.get(trimmed))
            } else {
                None
            };
            let spec = WindowSpec::from_options(opts, saved);
            let state_arc = Arc::new(Mutex::new(ObservedWindowState::from_spec(spec.clone())));
            guard
                .windows
                .insert(trimmed.to_string(), Arc::clone(&state_arc));
            (state_arc, spec)
        };

        let _ = self.tx.send(HostCommand::CreateWindow {
            label: trimmed.to_string(),
            spec,
        });

        Ok(WindowHandle {
            label: Arc::from(trimmed),
            registry: Arc::clone(self),
            state: state_arc,
        })
    }

    pub(crate) fn send_window_op(&self, label: &str, op: WindowOp) {
        let _ = self.tx.send(HostCommand::Window {
            label: label.to_string(),
            op,
        });
    }

    pub(crate) fn quit(&self) {
        let _ = self.tx.send(HostCommand::Quit);
    }

    pub(crate) fn tray(&self) -> Option<TrayConfig> {
        self.inner.lock().ok().and_then(|g| g.tray.clone())
    }

    pub(crate) fn menu(&self) -> Option<MenuConfig> {
        self.inner.lock().ok().and_then(|g| g.menu.clone())
    }

    pub(crate) fn set_tray(&self, tray: TrayConfig) -> Result<(), ForgeError> {
        {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| ForgeError::internal(codes::APP_ERROR, "window registry poisoned"))?;
            if guard.runtime_kind == RuntimeKind::Cef {
                return Err(ForgeError::unsupported(
                    "system tray is not supported on the CEF runtime (use runtime = \"webview\")",
                ));
            }
            if !cfg!(feature = "desktop-ui") && !guard.is_headless {
                return Err(ForgeError::unsupported(
                    "Cargo feature 'forge/desktop-ui' must be enabled to use system tray",
                ));
            }
            guard.tray = Some(tray.clone());
        }
        let _ = self.tx.send(HostCommand::SetTray(tray));
        Ok(())
    }

    pub(crate) fn set_menu(&self, menu: MenuConfig) -> Result<(), ForgeError> {
        {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| ForgeError::internal(codes::APP_ERROR, "window registry poisoned"))?;
            if guard.runtime_kind == RuntimeKind::Cef {
                return Err(ForgeError::unsupported(
                    "native application menu is not supported on the CEF runtime (use runtime = \"webview\")",
                ));
            }
            if !cfg!(feature = "desktop-ui") && !guard.is_headless {
                return Err(ForgeError::unsupported(
                    "Cargo feature 'forge/desktop-ui' must be enabled to use native menus",
                ));
            }
            guard.menu = Some(menu.clone());
        }
        let _ = self.tx.send(HostCommand::SetMenu(menu));
        Ok(())
    }

    /// Processes a host-originated window, menu, or tray event, returning `true` if the application
    /// loop should now terminate (for example, `forge.quit` or closing the last open window).
    pub(crate) fn handle_host_event(self: &Arc<Self>, event: HostEvent) -> bool {
        match event {
            HostEvent::Resized {
                label,
                width,
                height,
            } => {
                self.update_and_persist(&label, |st| {
                    st.size = (width, height);
                });
                false
            }
            HostEvent::Moved { label, x, y } => {
                self.update_and_persist(&label, |st| {
                    st.position = Some((x, y));
                });
                false
            }
            HostEvent::Focused { label, focused } => {
                if let Ok(guard) = self.inner.lock()
                    && let Some(st_arc) = guard.windows.get(&label)
                    && let Ok(mut st) = st_arc.lock()
                {
                    st.focused = focused;
                }
                false
            }
            HostEvent::Maximized { label, maximized } => {
                self.update_and_persist(&label, |st| {
                    st.maximized = maximized;
                });
                false
            }
            HostEvent::Closed { label } => {
                let Ok(mut guard) = self.inner.lock() else {
                    return true;
                };
                if let Some(removed) = guard.windows.remove(&label)
                    && let Ok(st) = removed.lock()
                    && st.spec.remember_state
                    && let Some(store) = &guard.state_store
                {
                    let _ = store.save(&label, st.to_saved_geometry());
                }
                guard.exit_on_last_close && guard.windows.is_empty()
            }
            HostEvent::MenuClicked { id } => self.dispatch_action_id(&id, "menu:click"),
            HostEvent::TrayClicked { id } => self.dispatch_action_id(&id, "tray:click"),
        }
    }

    fn update_and_persist(&self, label: &str, f: impl FnOnce(&mut ObservedWindowState)) {
        let Ok(guard) = self.inner.lock() else {
            return;
        };
        let Some(st_arc) = guard.windows.get(label) else {
            return;
        };
        let Ok(mut st) = st_arc.lock() else {
            return;
        };
        f(&mut st);
        if st.spec.remember_state
            && let Some(store) = &guard.state_store
        {
            let _ = store.save(label, st.to_saved_geometry());
        }
    }

    fn dispatch_action_id(self: &Arc<Self>, id: &str, event_topic: &str) -> bool {
        match id {
            "forge.quit" => {
                self.quit();
                true
            }
            "forge.show-main" => {
                if let Some(win) = self.window_handle("main") {
                    win.show();
                    win.focus();
                }
                false
            }
            "forge.hide-main" => {
                if let Some(win) = self.window_handle("main") {
                    win.hide();
                }
                false
            }
            "forge.fullscreen" => {
                if let Some(win) = self.window_handle("main") {
                    let next = !win.snapshot_state().fullscreen;
                    win.set_fullscreen(next);
                }
                false
            }
            custom => {
                let labels = self.window_labels();
                for lbl in labels {
                    if let Some(win) = self.window_handle(&lbl) {
                        let _ = win.emit(event_topic, json!({ "id": custom }));
                    }
                }
                false
            }
        }
    }
}

/// Application-wide capability handle (`Clone + Send + Sync`) for creating windows,
/// broadcasting events across windows, and updating tray/menu configurations.
#[derive(Clone)]
pub struct AppHandle {
    pub(crate) registry: Arc<WindowRegistry>,
}

impl AppHandle {
    pub(crate) fn new(registry: Arc<WindowRegistry>) -> Self {
        Self { registry }
    }

    /// Opens a new application window identified by `label`.
    pub fn create_window(
        &self,
        label: impl Into<String>,
        opts: WindowOptions,
    ) -> Result<WindowHandle, ForgeError> {
        self.registry.create_window(label.into(), opts)
    }

    /// Looks up an active window by `label`.
    pub fn window(&self, label: &str) -> Option<WindowHandle> {
        self.registry.window_handle(label)
    }

    /// Returns the sorted list of currently open window labels.
    pub fn windows(&self) -> Vec<String> {
        self.registry.window_labels()
    }

    /// Broadcasts an asynchronous push event to every open window.
    pub fn emit_all<T: Serialize>(
        &self,
        event: impl Into<String>,
        payload: T,
    ) -> Result<(), ForgeError> {
        let event_str = event.into();
        for lbl in self.registry.window_labels() {
            if let Some(win) = self.registry.window_handle(&lbl) {
                win.emit(event_str.clone(), &payload)?;
            }
        }
        Ok(())
    }

    /// Emits an asynchronous push event to a specific target window (`FORGE_WINDOW_NOT_FOUND` if absent).
    pub fn emit_to<T: Serialize>(
        &self,
        label: &str,
        event: impl Into<String>,
        payload: T,
    ) -> Result<(), ForgeError> {
        let win = self.registry.window_handle(label).ok_or_else(|| {
            ForgeError::not_found(
                codes::WINDOW_NOT_FOUND,
                format!("target window '{label}' does not exist"),
            )
        })?;
        win.emit(event, payload)
    }

    /// Installs or updates the system tray icon and context menu (`FORGE_UNSUPPORTED` on CEF).
    pub fn set_tray(&self, tray: TrayConfig) -> Result<(), ForgeError> {
        self.registry.set_tray(tray)
    }

    /// Installs or updates the native application menu bar (`FORGE_UNSUPPORTED` on CEF).
    pub fn set_menu(&self, menu: MenuConfig) -> Result<(), ForgeError> {
        self.registry.set_menu(menu)
    }

    /// Terminates the application event loop cleanly.
    pub fn quit(&self) {
        self.registry.quit();
    }
}

/// Thread-safe handle (`Clone + Send + Sync`) representing a specific native window (by default `"main"`),
/// injected automatically into command handlers as the calling window.
#[derive(Clone)]
pub struct WindowHandle {
    label: Arc<str>,
    registry: Arc<WindowRegistry>,
    state: Arc<Mutex<ObservedWindowState>>,
}

impl WindowHandle {
    /// Unique identifier of this window (`"main"` for the primary window).
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Pushes an asynchronous event to JavaScript listeners in *this* window.
    pub fn emit<T: Serialize>(
        &self,
        event: impl Into<String>,
        payload: T,
    ) -> Result<(), ForgeError> {
        let event = event.into();
        let payload_json = serde_json::to_string(&payload).map_err(|e| {
            ForgeError::new(
                codes::SERIALIZE_ERROR,
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

        self.registry.send_window_op(
            &self.label,
            WindowOp::Emit {
                event,
                payload_json,
            },
        );
        Ok(())
    }

    pub fn set_title(&self, title: impl Into<String>) {
        let title = title.into();
        if let Ok(mut guard) = self.state.lock() {
            guard.title = title.clone();
        }
        self.registry
            .send_window_op(&self.label, WindowOp::SetTitle(title));
    }

    pub fn set_size(&self, width: u32, height: u32) {
        if let Ok(mut guard) = self.state.lock() {
            guard.size = (width, height);
        }
        let _ = self.registry.handle_host_event(HostEvent::Resized {
            label: self.label.to_string(),
            width,
            height,
        });
        self.registry
            .send_window_op(&self.label, WindowOp::SetSize { width, height });
    }

    pub fn set_position(&self, x: i32, y: i32) {
        if let Ok(mut guard) = self.state.lock() {
            guard.position = Some((x, y));
        }
        let _ = self.registry.handle_host_event(HostEvent::Moved {
            label: self.label.to_string(),
            x,
            y,
        });
        self.registry
            .send_window_op(&self.label, WindowOp::SetPosition { x, y });
    }

    pub fn set_minimized(&self, minimized: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.minimized = minimized;
        }
        self.registry
            .send_window_op(&self.label, WindowOp::Minimize(minimized));
    }

    pub fn set_maximized(&self, maximized: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.maximized = maximized;
        }
        let _ = self.registry.handle_host_event(HostEvent::Maximized {
            label: self.label.to_string(),
            maximized,
        });
        self.registry
            .send_window_op(&self.label, WindowOp::Maximize(maximized));
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.fullscreen = fullscreen;
        }
        self.registry
            .send_window_op(&self.label, WindowOp::Fullscreen(fullscreen));
    }

    pub fn show(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.visible = true;
        }
        self.registry.send_window_op(&self.label, WindowOp::Show);
    }

    pub fn hide(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.visible = false;
        }
        self.registry.send_window_op(&self.label, WindowOp::Hide);
    }

    pub fn focus(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.focused = true;
        }
        self.registry.send_window_op(&self.label, WindowOp::Focus);
    }

    pub fn drag_window(&self) {
        self.registry.send_window_op(&self.label, WindowOp::Drag);
    }

    pub fn close_window(&self) {
        self.registry.send_window_op(&self.label, WindowOp::Close);
    }

    pub fn quit_app(&self) {
        self.registry.quit();
    }

    pub(crate) fn snapshot_state(&self) -> ObservedWindowState {
        self.state.lock().map(|g| g.clone()).unwrap_or_else(|_| {
            ObservedWindowState::from_spec(WindowSpec::from_options(WindowOptions::default(), None))
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
