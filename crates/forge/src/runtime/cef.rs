use std::collections::HashMap;
use std::os::raw::c_int;
use std::ptr;
use std::sync::{Arc, Mutex};

use cef::{
    App, Browser, BrowserSettings, BrowserView, Callback, CefString, CefStringMultimap, Client,
    CommandLine, Frame, ImplApp, ImplBrowser, ImplCallback, ImplClient, ImplCommandLine, ImplFrame,
    ImplLifeSpanHandler, ImplPanel, ImplPanelDelegate, ImplPostData, ImplPostDataElement,
    ImplRequest, ImplResourceHandler, ImplResponse, ImplSchemeHandlerFactory, ImplSchemeRegistrar,
    ImplTask, ImplView, ImplViewDelegate, ImplWindow, ImplWindowDelegate, LifeSpanHandler,
    LogSeverity, Point, PostDataElement, Rect, Request, ResourceHandler, ResourceReadCallback,
    Response, SchemeHandlerFactory, SchemeOptions, SchemeRegistrar, Settings, Size, Task, ThreadId,
    View, Window, WindowDelegate, WrapApp, WrapClient, WrapLifeSpanHandler, WrapResourceHandler,
    WrapSchemeHandlerFactory, WrapTask, WrapWindowDelegate, args::Args, rc::Rc,
};
use http::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use http::{HeaderMap, Method, Uri};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::error::{Error, Result};
use crate::kernel::ForgeKernel;
use crate::runtime::{
    HostCommand, HostEvent, ProcessDisposition, ProtocolRequest, ProtocolResponse, RuntimeHost,
    WindowOp, WindowSpec, emit_script,
};

struct CefWindowEntry {
    window: Window,
    #[allow(dead_code)]
    browser_view: BrowserView,
    browser: Option<Browser>,
}

#[derive(Default)]
struct CefHostState {
    windows: HashMap<String, CefWindowEntry>,
    browser_id_to_label: HashMap<c_int, String>,
}

pub(crate) struct CefHost;

impl RuntimeHost for CefHost {
    fn enter_process() -> Result<ProcessDisposition> {
        init_cef_api_version()?;

        let args = Args::new();
        let mut app = ForgeCefApp::new();
        let code = cef::execute_process(Some(args.as_main_args()), Some(&mut app), ptr::null_mut());
        if code >= 0 {
            Ok(ProcessDisposition::SubprocessExited(code))
        } else {
            Ok(ProcessDisposition::ContinueAsHost)
        }
    }

    fn run_host(
        kernel: Arc<ForgeKernel>,
        mut commands: mpsc::UnboundedReceiver<HostCommand>,
    ) -> Result<()> {
        init_cef_api_version()?;

        let args = Args::new();
        let mut app = ForgeCefApp::new();

        let cache_dir = resolve_profile_cache_dir(&kernel.config.app.identifier);
        let _ = std::fs::create_dir_all(&cache_dir);
        let cache_str = cache_dir.to_string_lossy().into_owned();

        let settings = Settings {
            no_sandbox: 1,
            root_cache_path: CefString::from(cache_str.as_str()),
            cache_path: CefString::from(cache_str.as_str()),
            log_severity: LogSeverity::WARNING,
            ..Default::default()
        };

        if cef::initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            ptr::null_mut(),
        ) == 0
        {
            return Err(Error::Runtime(
                "cef::initialize failed (verify CEF_PATH / runtime resources)".into(),
            ));
        }

        let host_state = Arc::new(Mutex::new(CefHostState::default()));

        let mut scheme_factory =
            ForgeSchemeFactory::new(Arc::clone(&kernel), Arc::clone(&host_state));
        if cef::register_scheme_handler_factory(
            Some(&CefString::from("forge")),
            Some(&CefString::from("localhost")),
            Some(&mut scheme_factory),
        ) == 0
        {
            cef::shutdown();
            return Err(Error::Runtime(
                "failed to register forge://localhost custom scheme factory in CEF".into(),
            ));
        }

        let state_for_cmds = Arc::clone(&host_state);
        let kernel_for_cmds = Arc::clone(&kernel);
        let forwarder = kernel.tokio_handle.spawn(async move {
            while let Some(cmd) = commands.recv().await {
                let mut ui_task = HostCommandUiTask::new(
                    Arc::clone(&kernel_for_cmds),
                    Arc::clone(&state_for_cmds),
                    cmd,
                );
                cef::post_task(ThreadId::UI, Some(&mut ui_task));
            }
        });

        for (label, spec) in kernel.registry.startup_windows() {
            create_views_window(&kernel, &host_state, &label, &spec)?;
        }

        cef::run_message_loop();

        if let Ok(mut guard) = host_state.lock() {
            guard.windows.clear();
            guard.browser_id_to_label.clear();
        }
        forwarder.abort();
        cef::shutdown();
        Ok(())
    }
}

fn create_views_window(
    kernel: &Arc<ForgeKernel>,
    host_state: &Arc<Mutex<CefHostState>>,
    label: &str,
    spec: &WindowSpec,
) -> Result<()> {
    let life_span = ForgeLifeSpanHandler::new(label.to_string(), Arc::clone(host_state));
    let mut client = ForgeClient::new(life_span);
    let browser_settings = BrowserSettings::default();
    let clean_path = spec.url.trim_start_matches('/');
    let rel = if clean_path.is_empty() {
        "index.html"
    } else {
        clean_path
    };
    let entry_url = CefString::from(format!("forge://localhost/{rel}").as_str());

    let browser_view = cef::browser_view_create(
        Some(&mut client),
        Some(&entry_url),
        Some(&browser_settings),
        None,
        None,
        None,
    )
    .ok_or_else(|| {
        Error::Runtime(format!(
            "failed to create CEF BrowserView for window '{label}'"
        ))
    })?;

    let mut win_delegate = ForgeWindowDelegate::new(
        label.to_string(),
        spec.clone(),
        browser_view.clone(),
        Arc::clone(kernel),
        Arc::clone(host_state),
    );
    let window = cef::window_create_top_level(Some(&mut win_delegate)).ok_or_else(|| {
        Error::Runtime(format!(
            "failed to create top-level CEF Views Window for '{label}'"
        ))
    })?;

    if let Ok(mut guard) = host_state.lock() {
        guard.windows.insert(
            label.to_string(),
            CefWindowEntry {
                window,
                browser_view,
                browser: None,
            },
        );
    }

    Ok(())
}

fn init_cef_api_version() -> Result<()> {
    #[cfg(target_os = "macos")]
    unsafe {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let cef_dir = cef::sys::get_cef_dir()
            .ok_or_else(|| Error::Runtime("macOS CEF framework directory not found".into()))?;
        let framework_dir = cef_dir
            .join(cef::sys::FRAMEWORK_PATH)
            .canonicalize()
            .map_err(|e| Error::Runtime(format!("failed to locate CEF framework: {e}")))?;
        let c_path = CString::new(framework_dir.as_os_str().as_bytes())
            .map_err(|e| Error::Runtime(format!("invalid framework path: {e}")))?;
        if cef::sys::cef_load_library(c_path.as_ptr().cast()) != 1 {
            return Err(Error::Runtime("failed to load macOS CEF framework".into()));
        }
    }

    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    Ok(())
}

fn resolve_profile_cache_dir(identifier: &str) -> std::path::PathBuf {
    let safe_id = crate::util::sanitize_identifier(identifier);
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("forge")
        .join("profiles")
        .join(safe_id)
}

cef::wrap_app! {
    struct ForgeCefApp {}

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            #[cfg(target_os = "linux")]
            if let Some(cmd) = command_line {
                cmd.append_switch(Some(&CefString::from("disable-gpu-sandbox")));
            }
            #[cfg(not(target_os = "linux"))]
            let _ = command_line;
        }

        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            if let Some(reg) = registrar {
                let opts = (SchemeOptions::STANDARD.get_raw()
                    | SchemeOptions::DISPLAY_ISOLATED.get_raw()
                    | SchemeOptions::SECURE.get_raw()
                    | SchemeOptions::CORS_ENABLED.get_raw()
                    | SchemeOptions::FETCH_ENABLED.get_raw()) as c_int;
                reg.add_custom_scheme(Some(&CefString::from("forge")), opts);
            }
        }
    }
}

cef::wrap_window_delegate! {
    struct ForgeWindowDelegate {
        label: String,
        spec: WindowSpec,
        browser_view: BrowserView,
        kernel: Arc<ForgeKernel>,
        host_state: Arc<Mutex<CefHostState>>,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size {
                width: self.spec.width as c_int,
                height: self.spec.height as c_int,
            }
        }

        fn minimum_size(&self, _view: Option<&mut View>) -> Size {
            Size {
                width: self.spec.min_width.unwrap_or(200) as c_int,
                height: self.spec.min_height.unwrap_or(150) as c_int,
            }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(win) = window else {
                return;
            };
            let mut child = View::from(&self.browser_view);
            win.add_child_view(Some(&mut child));
            win.set_title(Some(&CefString::from(self.spec.title.as_str())));

            let size = Size {
                width: self.spec.width as c_int,
                height: self.spec.height as c_int,
            };
            win.set_size(Some(&size));
            if let (Some(x), Some(y)) = (self.spec.x, self.spec.y) {
                let pos = Point { x, y };
                win.set_position(Some(&pos));
            } else {
                win.center_window(Some(&size));
            }

            if self.spec.visible {
                win.show();
            }
            if self.spec.maximized {
                win.maximize();
            }
        }

        fn on_window_bounds_changed(&self, _window: Option<&mut Window>, new_bounds: Option<&Rect>) {
            if let Some(b) = new_bounds {
                if b.width > 0 && b.height > 0 {
                    let _ = self.kernel.registry.handle_host_event(HostEvent::Resized {
                        label: self.label.clone(),
                        width: b.width as u32,
                        height: b.height as u32,
                    });
                }
                let _ = self.kernel.registry.handle_host_event(HostEvent::Moved {
                    label: self.label.clone(),
                    x: b.x,
                    y: b.y,
                });
            }
        }

        fn on_window_activation_changed(&self, _window: Option<&mut Window>, active: c_int) {
            let _ = self.kernel.registry.handle_host_event(HostEvent::Focused {
                label: self.label.clone(),
                focused: active != 0,
            });
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            if let Ok(mut guard) = self.host_state.lock()
                && let Some(removed) = guard.windows.remove(&self.label)
                && let Some(b) = removed.browser
            {
                guard.browser_id_to_label.remove(&b.identifier());
            }
            let should_exit = self.kernel.registry.handle_host_event(HostEvent::Closed {
                label: self.label.clone(),
            });
            if should_exit {
                cef::quit_message_loop();
            }
        }

        fn is_frameless(&self, _window: Option<&mut Window>) -> c_int {
            c_int::from(!self.spec.decorations)
        }

        fn can_resize(&self, _window: Option<&mut Window>) -> c_int {
            c_int::from(self.spec.resizable)
        }

        fn can_maximize(&self, _window: Option<&mut Window>) -> c_int {
            c_int::from(self.spec.resizable)
        }

        fn can_minimize(&self, _window: Option<&mut Window>) -> c_int {
            1
        }

        fn can_close(&self, _window: Option<&mut Window>) -> c_int {
            1
        }
    }
}

cef::wrap_life_span_handler! {
    struct ForgeLifeSpanHandler {
        label: String,
        host_state: Arc<Mutex<CefHostState>>,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser
                && let Ok(mut guard) = self.host_state.lock()
            {
                let bid = b.identifier();
                guard.browser_id_to_label.insert(bid, self.label.clone());
                if let Some(entry) = guard.windows.get_mut(&self.label) {
                    entry.browser = Some(b.clone());
                }
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser
                && let Ok(mut guard) = self.host_state.lock()
            {
                guard.browser_id_to_label.remove(&b.identifier());
            }
        }
    }
}

cef::wrap_client! {
    struct ForgeClient {
        life_span: LifeSpanHandler,
    }

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
    }
}

cef::wrap_task! {
    struct HostCommandUiTask {
        kernel: Arc<ForgeKernel>,
        host_state: Arc<Mutex<CefHostState>>,
        command: HostCommand,
    }

    impl Task {
        fn execute(&self) {
            match &self.command {
                HostCommand::CreateWindow { label, spec } => {
                    let exists = self
                        .host_state
                        .lock()
                        .map(|g| g.windows.contains_key(label))
                        .unwrap_or(false);
                    if !exists {
                        let _ = create_views_window(&self.kernel, &self.host_state, label, spec);
                    }
                }
                HostCommand::Window { label, op } => {
                    let entry_pair = self.host_state.lock().ok().and_then(|g| {
                        g.windows
                            .get(label)
                            .map(|e| (e.window.clone(), e.browser.clone()))
                    });
                    let Some((window, browser)) = entry_pair else {
                        return;
                    };
                    match op {
                        WindowOp::SetTitle(title) => {
                            window.set_title(Some(&CefString::from(title.as_str())));
                        }
                        WindowOp::SetSize { width, height } => {
                            let sz = Size {
                                width: *width as c_int,
                                height: *height as c_int,
                            };
                            window.set_size(Some(&sz));
                        }
                        WindowOp::SetPosition { x, y } => {
                            let pt = Point { x: *x, y: *y };
                            window.set_position(Some(&pt));
                        }
                        WindowOp::Minimize(minimized) => {
                            if *minimized {
                                window.minimize();
                            } else {
                                window.restore();
                            }
                        }
                        WindowOp::Maximize(maximized) => {
                            if *maximized {
                                window.maximize();
                            } else {
                                window.restore();
                            }
                        }
                        WindowOp::Fullscreen(fullscreen) => {
                            window.set_fullscreen(c_int::from(*fullscreen));
                        }
                        WindowOp::Show => {
                            window.show();
                        }
                        WindowOp::Hide => {
                            window.hide();
                        }
                        WindowOp::Focus => {
                            window.activate();
                        }
                        WindowOp::Drag => {}
                        WindowOp::Emit {
                            event,
                            payload_json,
                        } => {
                            if let Some(b) = browser
                                && let Some(frame) = b.main_frame()
                                && let Some(raw_script) = emit_script(event, payload_json)
                            {
                                let script = CefString::from(raw_script.as_str());
                                frame.execute_java_script(Some(&script), None, 0);
                            }
                        }
                        WindowOp::Close => {
                            window.close();
                        }
                    }
                }
                HostCommand::SetTray(_) | HostCommand::SetMenu(_) => {}
                HostCommand::Quit => {
                    let windows: Vec<Window> = self
                        .host_state
                        .lock()
                        .map(|g| g.windows.values().map(|e| e.window.clone()).collect())
                        .unwrap_or_default();
                    if windows.is_empty() {
                        cef::quit_message_loop();
                    } else {
                        for win in windows {
                            win.close();
                        }
                    }
                }
            }
        }
    }
}

cef::wrap_scheme_handler_factory! {
    struct ForgeSchemeFactory {
        kernel: Arc<ForgeKernel>,
        host_state: Arc<Mutex<CefHostState>>,
    }

    impl SchemeHandlerFactory {
        fn create(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            _request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let window_label = match browser.as_ref().map(|b| b.identifier()) {
                Some(bid) => self
                    .host_state
                    .lock()
                    .ok()
                    .and_then(|g| g.browser_id_to_label.get(&bid).cloned())
                    .unwrap_or_else(|| "__closed__".to_string()),
                None => "main".to_string(),
            };

            Some(ForgeResourceHandler::new(
                Arc::clone(&self.kernel),
                window_label,
                Arc::new(Mutex::new(ResourceState::default())),
            ))
        }
    }
}

#[derive(Default)]
struct ResourceState {
    response: Option<ProtocolResponse>,
    offset: usize,
    abort_handle: Option<AbortHandle>,
}

cef::wrap_resource_handler! {
    struct ForgeResourceHandler {
        kernel: Arc<ForgeKernel>,
        window_label: String,
        state: Arc<Mutex<ResourceState>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            request: Option<&mut Request>,
            handle_request: Option<&mut c_int>,
            callback: Option<&mut Callback>,
        ) -> c_int {
            let Some(req) = request else {
                return 0;
            };
            let Some(handle_request) = handle_request else {
                return 0;
            };
            let Some(callback) = callback else {
                return 0;
            };

            let proto_req = extract_protocol_request(req, self.window_label.clone());
            *handle_request = 0;
            let cb = callback.clone();
            let state_slot = Arc::clone(&self.state);

            let abort = self.kernel.dispatch_async(proto_req, move |proto_res| {
                if let Ok(mut guard) = state_slot.lock() {
                    guard.response = Some(proto_res);
                    guard.offset = 0;
                }
                cb.cont();
            });

            if let Ok(mut guard) = self.state.lock() {
                guard.abort_handle = Some(abort);
            }

            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let Some(response) = response else {
                return;
            };
            let Ok(guard) = self.state.lock() else {
                return;
            };
            let Some(proto_res) = &guard.response else {
                response.set_status(500);
                if let Some(len) = response_length {
                    *len = 0;
                }
                return;
            };

            response.set_status(proto_res.status.as_u16() as c_int);
            if let Some(reason) = proto_res.status.canonical_reason() {
                response.set_status_text(Some(&CefString::from(reason)));
            }

            let full_ct = proto_res
                .headers
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream");
            let bare_mime = full_ct.split(';').next().unwrap_or(full_ct).trim();
            response.set_mime_type(Some(&CefString::from(bare_mime)));

            let mut multimap = CefStringMultimap::new();
            for (name, val) in &proto_res.headers {
                if let Ok(v_str) = val.to_str() {
                    multimap.append(name.as_str(), v_str);
                }
            }
            response.set_header_map(Some(&mut multimap));

            if let Some(len) = response_length {
                *len = proto_res.body.len() as i64;
            }
        }

        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: c_int,
            bytes_read: Option<&mut c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> c_int {
            if bytes_to_read <= 0 || data_out.is_null() {
                return 0;
            }
            let Some(bytes_read) = bytes_read else {
                return 0;
            };
            let Ok(mut guard) = self.state.lock() else {
                *bytes_read = 0;
                return 0;
            };
            let offset = guard.offset;
            let Some(proto_res) = &guard.response else {
                *bytes_read = 0;
                return 0;
            };

            if offset >= proto_res.body.len() {
                *bytes_read = 0;
                return 0;
            }

            let remaining = proto_res.body.len() - offset;
            let to_copy = remaining.min(bytes_to_read as usize);
            unsafe {
                ptr::copy_nonoverlapping(proto_res.body.as_ptr().add(offset), data_out, to_copy);
            }
            guard.offset = offset + to_copy;
            *bytes_read = to_copy as c_int;
            1
        }

        fn cancel(&self) {
            if let Ok(mut guard) = self.state.lock()
                && let Some(handle) = guard.abort_handle.take()
            {
                handle.abort();
            }
        }
    }
}

fn extract_protocol_request(req: &mut Request, window_label: String) -> ProtocolRequest {
    let raw_url = CefString::from(&req.url()).to_string();
    let uri: Uri = raw_url
        .parse()
        .unwrap_or_else(|_| Uri::from_static("forge://localhost/"));

    let raw_method = CefString::from(&req.method()).to_string();
    let method = Method::from_bytes(raw_method.as_bytes()).unwrap_or(Method::GET);

    let mut headers = HeaderMap::new();
    for name in ["x-forge-token", "content-type", "sec-fetch-site"] {
        let userfree = req.header_by_name(Some(&CefString::from(name)));
        let val_str = CefString::from(&userfree).to_string();
        if !val_str.is_empty()
            && let (Ok(hn), Ok(hv)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(&val_str),
            )
        {
            headers.insert(hn, hv);
        }
    }

    let mut body = Vec::new();
    if let Some(post_data) = req.post_data() {
        let count = post_data.element_count();
        if count > 0 {
            let mut elements: Vec<Option<PostDataElement>> = vec![None; count];
            post_data.elements(Some(&mut elements));
            for elem in elements.into_iter().flatten() {
                let len = elem.bytes_count();
                if len > 0 {
                    let mut chunk = vec![0u8; len];
                    let actual = elem.bytes(len, chunk.as_mut_ptr());
                    chunk.truncate(actual);
                    body.extend_from_slice(&chunk);
                }
            }
        }
    }

    ProtocolRequest {
        window: window_label,
        method,
        uri,
        headers,
        body,
    }
}
