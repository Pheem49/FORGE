use std::sync::Arc;

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Fullscreen, WindowBuilder};
use tokio::sync::mpsc;
use wry::WebViewBuilder;

use crate::error::{Error, Result};
use crate::kernel::ForgeKernel;
use crate::runtime::{HostDirective, ProtocolRequest, RuntimeHost, emit_script};

fn entry_url(path: &str) -> String {
    let clean = path.trim_start_matches('/');
    #[cfg(target_os = "windows")]
    {
        format!("https://forge.localhost/{clean}")
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("forge://localhost/{clean}")
    }
}

pub(crate) struct WebViewHost;

impl RuntimeHost for WebViewHost {
    fn run_host(
        kernel: Arc<ForgeKernel>,
        mut directives: mpsc::UnboundedReceiver<HostDirective>,
    ) -> Result<()> {
        let mut event_loop = EventLoopBuilder::<HostDirective>::with_user_event().build();
        let proxy = event_loop.create_proxy();

        let forwarder = kernel.tokio_handle.spawn(async move {
            while let Some(directive) = directives.recv().await {
                if proxy.send_event(directive).is_err() {
                    break;
                }
            }
        });

        let win_cfg = &kernel.config.window;
        let mut window_builder = WindowBuilder::new()
            .with_title(&win_cfg.title)
            .with_inner_size(LogicalSize::new(win_cfg.width, win_cfg.height))
            .with_resizable(win_cfg.resizable)
            .with_decorations(win_cfg.decorations);

        if let (Some(min_w), Some(min_h)) = (win_cfg.min_width, win_cfg.min_height) {
            window_builder = window_builder.with_min_inner_size(LogicalSize::new(min_w, min_h));
        }

        let window = window_builder
            .build(&event_loop)
            .map_err(|e| Error::Runtime(format!("failed to create native OS window: {e}")))?;

        let kernel_for_proto = Arc::clone(&kernel);
        let builder = WebViewBuilder::new()
            .with_devtools(win_cfg.devtools)
            .with_asynchronous_custom_protocol(
                "forge".into(),
                move |_webview_id, request, responder| {
                    let (parts, body) = request.into_parts();
                    let proto_req = ProtocolRequest {
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

        let url = entry_url("/index.html");
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

        event_loop.run_return(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;

            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    *control_flow = ControlFlow::Exit;
                }
                Event::UserEvent(directive) => match directive {
                    HostDirective::SetTitle(new_title) => {
                        window.set_title(&new_title);
                    }
                    HostDirective::SetSize { width, height } => {
                        window.set_inner_size(LogicalSize::new(width, height));
                    }
                    HostDirective::SetMinimized(minimized) => {
                        window.set_minimized(minimized);
                    }
                    HostDirective::SetMaximized(maximized) => {
                        window.set_maximized(maximized);
                    }
                    HostDirective::SetFullscreen(fullscreen) => {
                        let mode = if fullscreen {
                            Some(Fullscreen::Borderless(None))
                        } else {
                            None
                        };
                        window.set_fullscreen(mode);
                    }
                    HostDirective::DragWindow => {
                        let _ = window.drag_window();
                    }
                    HostDirective::EmitEvent {
                        event,
                        payload_json,
                    } => {
                        if let Some(script) = emit_script(&event, &payload_json) {
                            let _ = webview.evaluate_script(&script);
                        }
                    }
                    HostDirective::CloseWindow | HostDirective::QuitApp => {
                        *control_flow = ControlFlow::Exit;
                    }
                },
                _ => {}
            }
        });

        forwarder.abort();
        Ok(())
    }
}
