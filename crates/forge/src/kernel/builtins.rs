use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::config::{AppConfig, CloseBehavior, RuntimeKind};
use crate::error::ForgeError;
use crate::handler::{CommandEntry, CommandHandler, ErasedCommand};
use crate::runtime::{AppHandle, WindowHandle, WindowOptions};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MinimizeReq {
    minimized: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MaximizeReq {
    maximized: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FullscreenReq {
    fullscreen: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct AlwaysOnTopReq {
    always_on_top: bool,
}

#[derive(Debug, Deserialize)]
struct SetOnCloseReq {
    on_close: CloseBehavior,
}

#[derive(Debug, Deserialize)]
struct SetTitleReq {
    title: String,
}

#[derive(Debug, Deserialize)]
struct CreateWindowReq {
    label: String,
    url: Option<String>,
    title: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    x: Option<i32>,
    y: Option<i32>,
    resizable: Option<bool>,
    decorations: Option<bool>,
    visible: Option<bool>,
    always_on_top: Option<bool>,
    on_close: Option<CloseBehavior>,
}

#[derive(Debug, Deserialize)]
struct EmitAllReq {
    event: String,
    #[serde(default)]
    payload: Value,
}

#[derive(Debug, Deserialize)]
struct EmitToReq {
    target: String,
    event: String,
    #[serde(default)]
    payload: Value,
}

pub(crate) fn insert_builtin_commands(map: &mut HashMap<&'static str, Arc<dyn ErasedCommand>>) {
    register_default(map, "window.close", builtin_window_close);
    register_default(map, "window.destroy", builtin_window_destroy);
    register_default(map, "window.minimize", builtin_window_minimize);
    register_default(map, "window.maximize", builtin_window_maximize);
    register_default(map, "window.toggle_fullscreen", builtin_window_fullscreen);
    register_default(
        map,
        "window.set_always_on_top",
        builtin_window_set_always_on_top,
    );
    register_default(map, "window.set_on_close", builtin_window_set_on_close);
    register_default(map, "window.set_title", builtin_window_set_title);
    register_default(map, "window.show", builtin_window_show);
    register_default(map, "window.hide", builtin_window_hide);
    register_default(map, "window.focus", builtin_window_focus);
    register_default(map, "window.start_dragging", builtin_window_drag);
    register_default(map, "window.create", builtin_window_create);
    register_default(map, "window.list", builtin_window_list);
    register_default(map, "event.emit_all", builtin_event_emit_all);
    register_default(map, "event.emit_to", builtin_event_emit_to);
}

fn register_default<H, Marker>(
    map: &mut HashMap<&'static str, Arc<dyn ErasedCommand>>,
    name: &'static str,
    handler: H,
) where
    H: CommandHandler<Marker>,
    Marker: 'static,
{
    map.entry(name)
        .or_insert_with(|| Arc::new(CommandEntry::new(handler)));
}

async fn builtin_window_close(win: WindowHandle) -> Result<(), Infallible> {
    win.close_window();
    Ok(())
}

async fn builtin_window_destroy(win: WindowHandle) -> Result<(), Infallible> {
    win.destroy();
    Ok(())
}

async fn builtin_window_set_always_on_top(
    win: WindowHandle,
    req: AlwaysOnTopReq,
) -> Result<(), Infallible> {
    win.set_always_on_top(req.always_on_top);
    Ok(())
}

async fn builtin_window_set_on_close(
    win: WindowHandle,
    req: SetOnCloseReq,
) -> Result<(), Infallible> {
    win.set_on_close(req.on_close);
    Ok(())
}

async fn builtin_window_minimize(
    win: WindowHandle,
    req: Option<MinimizeReq>,
) -> Result<(), Infallible> {
    let next = req.and_then(|r| r.minimized).unwrap_or(true);
    win.set_minimized(next);
    Ok(())
}

async fn builtin_window_maximize(
    win: WindowHandle,
    req: Option<MaximizeReq>,
) -> Result<(), Infallible> {
    let next = req
        .and_then(|r| r.maximized)
        .unwrap_or_else(|| !win.snapshot_state().maximized);
    win.set_maximized(next);
    Ok(())
}

async fn builtin_window_fullscreen(
    win: WindowHandle,
    req: Option<FullscreenReq>,
) -> Result<(), Infallible> {
    let next = req
        .and_then(|r| r.fullscreen)
        .unwrap_or_else(|| !win.snapshot_state().fullscreen);
    win.set_fullscreen(next);
    Ok(())
}

async fn builtin_window_set_title(win: WindowHandle, req: SetTitleReq) -> Result<(), Infallible> {
    win.set_title(req.title);
    Ok(())
}

async fn builtin_window_show(win: WindowHandle) -> Result<(), Infallible> {
    win.show();
    Ok(())
}

async fn builtin_window_hide(win: WindowHandle) -> Result<(), Infallible> {
    win.hide();
    Ok(())
}

async fn builtin_window_focus(win: WindowHandle) -> Result<(), Infallible> {
    win.focus();
    Ok(())
}

async fn builtin_window_drag(cfg: AppConfig, win: WindowHandle) -> Result<(), ForgeError> {
    if cfg.runtime.default == RuntimeKind::Cef {
        return Err(ForgeError::unsupported(
            "window.start_dragging is not supported on the CEF runtime",
        ));
    }
    win.drag_window();
    Ok(())
}

async fn builtin_window_create(app: AppHandle, req: CreateWindowReq) -> Result<String, ForgeError> {
    let mut opts = app
        .registry
        .configured_template(&req.label)
        .map(|cfg| WindowOptions::from_window_config(&cfg))
        .unwrap_or_default();

    if let Some(url) = req.url {
        opts.url = url;
    }
    if let Some(title) = req.title {
        opts.title = title;
    }
    if let (Some(w), Some(h)) = (req.width, req.height) {
        opts = opts.size(w, h);
    }
    if let (Some(x), Some(y)) = (req.x, req.y) {
        opts = opts.position(x, y);
    }
    if let Some(resizable) = req.resizable {
        opts = opts.resizable(resizable);
    }
    if let Some(decorations) = req.decorations {
        opts = opts.decorations(decorations);
    }
    if let Some(visible) = req.visible {
        opts = opts.visible(visible);
    }
    if let Some(always_on_top) = req.always_on_top {
        opts = opts.always_on_top(always_on_top);
    }
    if let Some(on_close) = req.on_close {
        opts = opts.on_close(on_close);
    }

    let handle = app.create_window(&req.label, opts)?;
    Ok(handle.label().to_string())
}

async fn builtin_window_list(app: AppHandle) -> Result<Vec<String>, Infallible> {
    Ok(app.windows())
}

async fn builtin_event_emit_all(app: AppHandle, req: EmitAllReq) -> Result<(), ForgeError> {
    app.emit_all(req.event, req.payload)
}

async fn builtin_event_emit_to(app: AppHandle, req: EmitToReq) -> Result<(), ForgeError> {
    app.emit_to(&req.target, req.event, req.payload)
}
