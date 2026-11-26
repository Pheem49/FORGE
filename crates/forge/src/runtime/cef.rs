use std::os::raw::c_int;
use std::ptr;
use std::sync::{Arc, Mutex};

use cef::{
    App, Browser, BrowserSettings, Callback, CefString, CefStringMultimap, Client, CommandLine,
    Frame, ImplApp, ImplBrowser, ImplBrowserHost, ImplCallback, ImplClient, ImplCommandLine,
    ImplFrame, ImplLifeSpanHandler, ImplPostData, ImplPostDataElement, ImplRequest,
    ImplResourceHandler, ImplResponse, ImplSchemeHandlerFactory, ImplSchemeRegistrar, ImplTask,
    LifeSpanHandler, LogSeverity, PostDataElement, Rect, Request, ResourceHandler,
    ResourceReadCallback, Response, RuntimeStyle, SchemeHandlerFactory, SchemeOptions,
    SchemeRegistrar, Settings, Task, ThreadId, WindowInfo, WrapApp, WrapClient,
    WrapLifeSpanHandler, WrapResourceHandler, WrapSchemeHandlerFactory, WrapTask, args::Args,
    rc::Rc,
};
use http::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use http::{HeaderMap, Method, Uri};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::error::{Error, Result};
use crate::kernel::ForgeKernel;
use crate::runtime::{
    HostDirective, ProcessDisposition, ProtocolRequest, ProtocolResponse, RuntimeHost,
};

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
        mut directives: mpsc::UnboundedReceiver<HostDirective>,
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

        let mut scheme_factory = ForgeSchemeFactory::new(Arc::clone(&kernel));
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

        let browser_slot: Arc<Mutex<Option<Browser>>> = Arc::new(Mutex::new(None));
        let slot_for_directives = Arc::clone(&browser_slot);

        let forwarder = kernel.tokio_handle.spawn(async move {
            while let Some(directive) = directives.recv().await {
                let mut ui_task = DirectiveUiTask::new(Arc::clone(&slot_for_directives), directive);
                cef::post_task(ThreadId::UI, Some(&mut ui_task));
            }
        });

        let life_span = ForgeLifeSpanHandler::new(Arc::clone(&browser_slot));
        let mut client = ForgeClient::new(life_span);

        let win_cfg = &kernel.config.window;
        let window_info = WindowInfo {
            window_name: CefString::from(win_cfg.title.as_str()),
            bounds: Rect {
                x: 0,
                y: 0,
                width: win_cfg.width as c_int,
                height: win_cfg.height as c_int,
            },
            runtime_style: RuntimeStyle::ALLOY,
            ..Default::default()
        };
        let browser_settings = BrowserSettings::default();
        let entry_url = CefString::from("forge://localhost/index.html");

        if cef::browser_host_create_browser(
            Some(&window_info),
            Some(&mut client),
            Some(&entry_url),
            Some(&browser_settings),
            None,
            None,
        ) == 0
        {
            forwarder.abort();
            cef::shutdown();
            return Err(Error::Runtime(
                "failed to create Chromium browser window".into(),
            ));
        }

        cef::run_message_loop();

        forwarder.abort();
        cef::shutdown();
        Ok(())
    }
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
    let safe_id: String = identifier
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
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

cef::wrap_life_span_handler! {
    struct ForgeLifeSpanHandler {
        browser_slot: Arc<Mutex<Option<Browser>>>,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser
                && let Ok(mut guard) = self.browser_slot.lock()
            {
                *guard = Some(b.clone());
            }
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if let Ok(mut guard) = self.browser_slot.lock() {
                *guard = None;
            }
            cef::quit_message_loop();
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
    struct DirectiveUiTask {
        browser_slot: Arc<Mutex<Option<Browser>>>,
        directive: HostDirective,
    }

    impl Task {
        fn execute(&self) {
            let browser = self.browser_slot.lock().ok().and_then(|g| g.clone());
            match &self.directive {
                HostDirective::SetTitle(new_title) => {
                    if let Some(b) = browser
                        && let Some(frame) = b.main_frame()
                        && let Ok(escaped) = serde_json::to_string(new_title)
                    {
                        let script = CefString::from(format!("document.title = {escaped};").as_str());
                        frame.execute_java_script(Some(&script), None, 0);
                    }
                }
                HostDirective::SetSize { width, height } => {
                    if let Some(b) = browser
                        && let Some(frame) = b.main_frame()
                    {
                        let script = CefString::from(
                            format!("window.resizeTo({width}, {height});").as_str(),
                        );
                        frame.execute_java_script(Some(&script), None, 0);
                    }
                }
                HostDirective::SetMinimized(_)
                | HostDirective::SetMaximized(_)
                | HostDirective::SetFullscreen(_)
                | HostDirective::DragWindow => {}
                HostDirective::EmitEvent {
                    event,
                    payload_json,
                } => {
                    if let Some(b) = browser
                        && let Some(frame) = b.main_frame()
                        && let Ok(event_js) = serde_json::to_string(event)
                    {
                        let script = CefString::from(
                            format!(
                                "if (window.__forge_dispatch) {{ window.__forge_dispatch({event_js}, {payload_json}); }}"
                            )
                            .as_str(),
                        );
                        frame.execute_java_script(Some(&script), None, 0);
                    }
                }
                HostDirective::CloseWindow | HostDirective::QuitApp => {
                    if let Some(b) = browser
                        && let Some(host) = b.host()
                    {
                        host.close_browser(1);
                    } else {
                        cef::quit_message_loop();
                    }
                }
            }
        }
    }
}

cef::wrap_scheme_handler_factory! {
    struct ForgeSchemeFactory {
        kernel: Arc<ForgeKernel>,
    }

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            _request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            Some(ForgeResourceHandler::new(
                Arc::clone(&self.kernel),
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

            let proto_req = extract_protocol_request(req);
            // Defer response completion onto FORGE's async Tokio pool.
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

            // CEF's `set_mime_type` expects bare type/subtype without `; charset=utf-8`.
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

fn extract_protocol_request(req: &mut Request) -> ProtocolRequest {
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
        method,
        uri,
        headers,
        body,
    }
}
