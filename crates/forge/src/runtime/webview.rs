use std::collections::HashMap;
use std::sync::Arc;

use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Fullscreen, Window, WindowBuilder, WindowId};
use tokio::sync::mpsc;
use wry::{WebView, WebViewBuilder};

#[cfg(feature = "desktop-ui")]
use crate::config::{MenuConfig, MenuItemConfig, TrayConfig};
use crate::error::{Error, Result};
use crate::kernel::ForgeKernel;
use crate::runtime::{
    HostCommand, HostEvent, ProtocolRequest, RuntimeHost, WindowOp, WindowSpec, emit_script,
};
use crate::window_state::{MonitorBounds, SavedWindowGeometry};

enum LoopMessage {
    Host(HostCommand),
    #[cfg(feature = "desktop-ui")]
    MenuActivated(String),
}

pub(crate) struct WebViewHost;

impl RuntimeHost for WebViewHost {
    fn run_host(
        kernel: Arc<ForgeKernel>,
        mut commands: mpsc::UnboundedReceiver<HostCommand>,
    ) -> Result<()> {
        let mut event_loop = EventLoopBuilder::<LoopMessage>::with_user_event().build();
        let proxy = event_loop.create_proxy();

        #[cfg(feature = "desktop-ui")]
        {
            let menu_proxy = event_loop.create_proxy();
            muda::MenuEvent::set_event_handler(Some(Arc::new(move |event: muda::MenuEvent| {
                let _ = menu_proxy.send_event(LoopMessage::MenuActivated(event.id().0.clone()));
            })));
        }

        let forwarder = kernel.tokio_handle.spawn(async move {
            while let Some(cmd) = commands.recv().await {
                if proxy.send_event(LoopMessage::Host(cmd)).is_err() {
                    break;
                }
            }
        });

        let mut windows: HashMap<String, (Window, WebView)> = HashMap::new();
        let mut id_to_label: HashMap<WindowId, String> = HashMap::new();
        #[cfg(feature = "desktop-ui")]
        let mut active_tray: Option<tray_icon::TrayIcon> = None;
        #[cfg(feature = "desktop-ui")]
        let mut active_menus: Vec<muda::Menu> = Vec::new();
        let mut boot_error: Option<Error> = None;

        event_loop.run_return(|event, window_target, control_flow| {
            *control_flow = ControlFlow::Wait;

            match event {
                Event::NewEvents(StartCause::Init) => {
                    #[cfg(feature = "desktop-ui")]
                    if let Some(tray_cfg) = kernel.registry.tray()
                        && let Ok(t) = build_native_tray(&tray_cfg)
                    {
                        active_tray = Some(t);
                    }

                    let startup = kernel.registry.startup_windows();
                    let initial_menu = kernel.registry.menu();
                    #[cfg(not(feature = "desktop-ui"))]
                    let _ = &initial_menu;

                    for (label, spec) in startup {
                        match spawn_window(
                            window_target,
                            &kernel,
                            &label,
                            &spec,
                            #[cfg(feature = "desktop-ui")]
                            if label == "main" {
                                initial_menu.as_ref()
                            } else {
                                None
                            },
                            #[cfg(feature = "desktop-ui")]
                            &mut active_menus,
                        ) {
                            Ok((win, wv)) => {
                                id_to_label.insert(win.id(), label.clone());
                                windows.insert(label, (win, wv));
                            }
                            Err(err) => {
                                boot_error = Some(err);
                                *control_flow = ControlFlow::Exit;
                                return;
                            }
                        }
                    }
                }
                Event::WindowEvent {
                    window_id, event, ..
                } => {
                    let Some(label) = id_to_label.get(&window_id).cloned() else {
                        return;
                    };
                    match event {
                        WindowEvent::CloseRequested => {
                            id_to_label.remove(&window_id);
                            windows.remove(&label);
                            if kernel
                                .registry
                                .handle_host_event(HostEvent::Closed { label })
                            {
                                *control_flow = ControlFlow::Exit;
                            }
                        }
                        WindowEvent::Resized(phys) => {
                            let scale = windows
                                .get(&label)
                                .map(|(w, _)| w.scale_factor())
                                .unwrap_or(1.0);
                            let logical = phys.to_logical::<u32>(scale);
                            let _ = kernel.registry.handle_host_event(HostEvent::Resized {
                                label,
                                width: logical.width,
                                height: logical.height,
                            });
                        }
                        WindowEvent::Moved(phys) => {
                            let scale = windows
                                .get(&label)
                                .map(|(w, _)| w.scale_factor())
                                .unwrap_or(1.0);
                            let logical = phys.to_logical::<i32>(scale);
                            let _ = kernel.registry.handle_host_event(HostEvent::Moved {
                                label,
                                x: logical.x,
                                y: logical.y,
                            });
                        }
                        WindowEvent::Focused(focused) => {
                            let _ = kernel
                                .registry
                                .handle_host_event(HostEvent::Focused { label, focused });
                        }
                        _ => {}
                    }
                }
                #[cfg(feature = "desktop-ui")]
                Event::UserEvent(LoopMessage::MenuActivated(raw_id)) => {
                    let should_exit = if let Some(tray_id) = raw_id.strip_prefix("__tray__:") {
                        kernel.registry.handle_host_event(HostEvent::TrayClicked {
                            id: tray_id.to_string(),
                        })
                    } else {
                        kernel
                            .registry
                            .handle_host_event(HostEvent::MenuClicked { id: raw_id })
                    };
                    if should_exit {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                Event::UserEvent(LoopMessage::Host(cmd)) => match cmd {
                    HostCommand::CreateWindow { label, spec } => {
                        if !windows.contains_key(&label)
                            && let Ok((win, wv)) = spawn_window(
                                window_target,
                                &kernel,
                                &label,
                                &spec,
                                #[cfg(feature = "desktop-ui")]
                                None,
                                #[cfg(feature = "desktop-ui")]
                                &mut active_menus,
                            )
                        {
                            id_to_label.insert(win.id(), label.clone());
                            windows.insert(label, (win, wv));
                        }
                    }
                    HostCommand::Window { label, op } => {
                        if let WindowOp::Close = op {
                            if let Some((win, _)) = windows.remove(&label) {
                                id_to_label.remove(&win.id());
                            }
                            if kernel
                                .registry
                                .handle_host_event(HostEvent::Closed { label })
                            {
                                *control_flow = ControlFlow::Exit;
                            }
                            return;
                        }
                        if let Some((window, webview)) = windows.get(&label) {
                            apply_window_op(window, webview, op);
                        }
                    }
                    HostCommand::SetTray(tray_cfg) => {
                        #[cfg(feature = "desktop-ui")]
                        if let Ok(t) = build_native_tray(&tray_cfg) {
                            active_tray = Some(t);
                        }
                        #[cfg(not(feature = "desktop-ui"))]
                        let _ = tray_cfg;
                    }
                    HostCommand::SetMenu(menu_cfg) => {
                        #[cfg(feature = "desktop-ui")]
                        if let Some((main_win, _)) = windows.get("main")
                            && let Ok(m) = attach_native_menu(main_win, &menu_cfg)
                        {
                            active_menus.push(m);
                        }
                        #[cfg(not(feature = "desktop-ui"))]
                        let _ = menu_cfg;
                    }
                    HostCommand::Quit => {
                        *control_flow = ControlFlow::Exit;
                    }
                },
                _ => {}
            }
        });

        #[cfg(feature = "desktop-ui")]
        {
            drop(active_tray);
            drop(active_menus);
            muda::MenuEvent::set_event_handler(None);
        }

        forwarder.abort();
        if let Some(err) = boot_error {
            return Err(err);
        }
        Ok(())
    }
}

fn entry_url(path: &str) -> String {
    let clean = path.trim_start_matches('/');
    let rel = if clean.is_empty() {
        "index.html"
    } else {
        clean
    };
    #[cfg(target_os = "windows")]
    {
        format!("https://forge.localhost/{rel}")
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("forge://localhost/{rel}")
    }
}

fn spawn_window(
    window_target: &EventLoopWindowTarget<LoopMessage>,
    kernel: &Arc<ForgeKernel>,
    label: &str,
    spec: &WindowSpec,
    #[cfg(feature = "desktop-ui")] menu_cfg: Option<&MenuConfig>,
    #[cfg(feature = "desktop-ui")] active_menus: &mut Vec<muda::Menu>,
) -> Result<(Window, WebView)> {
    let mut geom = SavedWindowGeometry {
        x: spec.x,
        y: spec.y,
        width: spec.width,
        height: spec.height,
        maximized: spec.maximized,
    };

    if let Some(mon) = window_target
        .primary_monitor()
        .or_else(|| window_target.available_monitors().next())
    {
        let scale = mon.scale_factor();
        let pos = mon.position().to_logical::<i32>(scale);
        let size = mon.size().to_logical::<u32>(scale);
        geom = geom.clamp_to_monitor(MonitorBounds {
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
        });
    }

    let mut window_builder = WindowBuilder::new()
        .with_title(&spec.title)
        .with_inner_size(LogicalSize::new(geom.width, geom.height))
        .with_resizable(spec.resizable)
        .with_decorations(spec.decorations)
        .with_maximized(geom.maximized)
        .with_visible(spec.visible);

    if let (Some(x), Some(y)) = (geom.x, geom.y) {
        window_builder = window_builder.with_position(LogicalPosition::new(x, y));
    }
    if let (Some(min_w), Some(min_h)) = (spec.min_width, spec.min_height) {
        window_builder = window_builder.with_min_inner_size(LogicalSize::new(min_w, min_h));
    }

    let window = window_builder
        .build(window_target)
        .map_err(|e| Error::Runtime(format!("failed to create native OS window '{label}': {e}")))?;

    #[cfg(feature = "desktop-ui")]
    if let Some(mcfg) = menu_cfg
        && let Ok(menu) = attach_native_menu(&window, mcfg)
    {
        active_menus.push(menu);
    }

    let kernel_for_proto = Arc::clone(kernel);
    let label_owned = label.to_string();
    let builder = WebViewBuilder::new()
        .with_devtools(spec.devtools)
        .with_asynchronous_custom_protocol(
            "forge".into(),
            move |_webview_id, request, responder| {
                let (parts, body) = request.into_parts();
                let proto_req = ProtocolRequest {
                    window: label_owned.clone(),
                    method: parts.method,
                    uri: parts.uri,
                    headers: parts.headers,
                    body,
                };
                kernel_for_proto.dispatch_async(proto_req, move |proto_res| {
                    let mut res_builder = http::Response::builder().status(proto_res.status);
                    if let Some(headers_mut) = res_builder.headers_mut() {
                        *headers_mut = proto_res.headers;
                    }
                    let http_res = res_builder
                        .body(proto_res.body)
                        .unwrap_or_else(|_| http::Response::new(Vec::new()));
                    responder.respond(http_res);
                });
            },
        );

    #[cfg(target_os = "windows")]
    let builder = {
        use wry::WebViewBuilderExtWindows;
        builder.with_https_scheme(true)
    };

    let url = entry_url(&spec.url);
    let builder = builder.with_url(&url);

    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    ))]
    let webview = builder
        .build(&window)
        .map_err(|e| Error::Runtime(format!("failed to initialize system WebView: {e}")))?;

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window.default_vbox().ok_or_else(|| {
            Error::Runtime("GTK window missing default vertical container".into())
        })?;
        builder
            .build_gtk(vbox)
            .map_err(|e| Error::Runtime(format!("failed to initialize GTK WebView: {e}")))?
    };

    Ok((window, webview))
}

fn apply_window_op(window: &Window, webview: &WebView, op: WindowOp) {
    match op {
        WindowOp::SetTitle(new_title) => {
            window.set_title(&new_title);
        }
        WindowOp::SetSize { width, height } => {
            window.set_inner_size(LogicalSize::new(width, height));
        }
        WindowOp::SetPosition { x, y } => {
            window.set_outer_position(LogicalPosition::new(x, y));
        }
        WindowOp::Minimize(minimized) => {
            window.set_minimized(minimized);
        }
        WindowOp::Maximize(maximized) => {
            window.set_maximized(maximized);
        }
        WindowOp::Fullscreen(fullscreen) => {
            let mode = if fullscreen {
                Some(Fullscreen::Borderless(None))
            } else {
                None
            };
            window.set_fullscreen(mode);
        }
        WindowOp::Show => {
            window.set_visible(true);
        }
        WindowOp::Hide => {
            window.set_visible(false);
        }
        WindowOp::Focus => {
            window.set_focus();
        }
        WindowOp::Drag => {
            let _ = window.drag_window();
        }
        WindowOp::Emit {
            event,
            payload_json,
        } => {
            if let Some(script) = emit_script(&event, &payload_json) {
                let _ = webview.evaluate_script(&script);
            }
        }
        WindowOp::Close => {}
    }
}

#[cfg(feature = "desktop-ui")]
fn attach_native_menu(window: &Window, cfg: &MenuConfig) -> Result<muda::Menu> {
    let menu = muda::Menu::new();
    for sub_cfg in &cfg.submenu {
        let submenu = muda::Submenu::new(&sub_cfg.label, true);
        append_menu_items(&submenu, &sub_cfg.items, "");
        let _ = menu.append(&submenu);
    }

    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix;
        let _ = menu.init_for_gtk_window(window.gtk_window(), window.default_vbox());
    }
    #[cfg(target_os = "windows")]
    unsafe {
        use tao::platform::windows::WindowExtWindows;
        let _ = menu.init_for_hwnd(window.hwnd() as isize);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        menu.init_for_nsapp();
    }

    Ok(menu)
}

#[cfg(feature = "desktop-ui")]
fn build_native_tray(cfg: &TrayConfig) -> Result<tray_icon::TrayIcon> {
    let tray_menu = muda::Menu::new();
    for item in &cfg.menu {
        if item.separator {
            let _ = tray_menu.append(&muda::PredefinedMenuItem::separator());
        } else {
            let label = item.label.as_deref().unwrap_or("Item");
            let raw_id = item.id.as_deref().unwrap_or(label);
            let prefixed_id = format!("__tray__:{raw_id}");
            let accel = item
                .accelerator
                .as_deref()
                .and_then(|s| s.parse::<muda::accelerator::Accelerator>().ok());
            let mi = muda::MenuItem::with_id(prefixed_id, label, item.enabled, accel);
            let _ = tray_menu.append(&mi);
        }
    }

    let icon = load_or_default_tray_icon(cfg.icon.as_deref())?;
    let mut builder = tray_icon::TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(tray_menu));
    if let Some(tip) = &cfg.tooltip {
        builder = builder.with_tooltip(tip);
    }
    builder
        .build()
        .map_err(|e| Error::Runtime(format!("failed to initialize system tray: {e}")))
}

#[cfg(feature = "desktop-ui")]
fn append_menu_items(submenu: &muda::Submenu, items: &[MenuItemConfig], prefix: &str) {
    for item in items {
        if item.separator {
            let _ = submenu.append(&muda::PredefinedMenuItem::separator());
        } else {
            let label = item.label.as_deref().unwrap_or("Item");
            let raw_id = item.id.as_deref().unwrap_or(label);
            let full_id = format!("{prefix}{raw_id}");
            let accel = item
                .accelerator
                .as_deref()
                .and_then(|s| s.parse::<muda::accelerator::Accelerator>().ok());
            let mi = muda::MenuItem::with_id(full_id, label, item.enabled, accel);
            let _ = submenu.append(&mi);
        }
    }
}

#[cfg(feature = "desktop-ui")]
fn load_or_default_tray_icon(icon_path: Option<&std::path::Path>) -> Result<tray_icon::Icon> {
    if let Some(p) = icon_path
        && p.is_file()
        && let Ok(img) = image::open(p)
    {
        let rgba = img.into_rgba8();
        let (w, h) = rgba.dimensions();
        if let Ok(ic) = tray_icon::Icon::from_rgba(rgba.into_raw(), w, h) {
            return Ok(ic);
        }
    }

    // Fallback 32x32 crisp FORGE amber-on-charcoal badge icon.
    let (w, h) = (32u32, 32u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let border = !(3..29).contains(&x) || !(3..29).contains(&y);
            let (r, g, b, a) = if border {
                (24, 28, 32, 255)
            } else if (8..24).contains(&x) && (9..14).contains(&y)
                || (8..13).contains(&x) && (9..24).contains(&y)
                || (8..20).contains(&x) && (16..20).contains(&y)
            {
                (229, 133, 41, 255)
            } else {
                (24, 28, 32, 255)
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    tray_icon::Icon::from_rgba(rgba, w, h)
        .map_err(|e| Error::Runtime(format!("failed to construct fallback tray icon: {e}")))
}
