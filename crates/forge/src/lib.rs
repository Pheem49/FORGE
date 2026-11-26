pub mod config;
mod error;
mod handler;
mod headless;
mod kernel;
mod runtime;

pub use http;

pub use config::{AppConfig, AppMetaConfig, Config, RuntimeConfig, RuntimeKind, WindowConfig};
pub use error::{Error, ForgeError, Result};
pub use handler::{CommandHandler, FromContext, State};
pub use headless::{HeadlessApp, RawResponse};
pub use runtime::WindowHandle;

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use handler::{CommandEntry, ErasedCommand, StateMap};
use kernel::ForgeKernel;
use runtime::ProcessDisposition;

type SetupHook = Box<dyn FnOnce(&mut SetupContext) -> Result<()> + Send + 'static>;
type ConfigHook = Box<dyn FnOnce(&mut Config) + Send + 'static>;

/// Extension trait for grouping state initialization and commands into reusable modules
/// (providing a clean growth path toward v0.2 plugins without changing [`App`]).
pub trait Plugin {
    fn apply(self, app: App) -> App;
}

/// Context passed exclusively to [`App::setup`] hooks inside the primary host process.
/// Guaranteed never to run inside CEF helper subprocesses (`--type=renderer`, `--type=gpu-process`, etc.).
pub struct SetupContext<'a> {
    states: &'a mut StateMap,
    config: &'a Config,
    tokio_handle: &'a tokio::runtime::Handle,
}

impl<'a> SetupContext<'a> {
    /// Registers shared application state `T` accessible to handlers via [`State<T>`].
    pub fn insert_state<T: Send + Sync + 'static>(&mut self, state: T) {
        self.states.insert(state);
    }

    /// Returns a read-only reference to the resolved [`Config`].
    pub fn config(&self) -> &Config {
        self.config
    }

    /// Blocks on an async initialization future (e.g., opening an async database pool)
    /// using FORGE's internal Tokio runtime.
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.tokio_handle.block_on(fut))
    }

    /// Spawns a background task tied to FORGE's Tokio runtime.
    pub fn spawn<F>(&self, task: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.tokio_handle.spawn(task)
    }
}

/// Primary deep module for building and running a FORGE desktop application.
pub struct App {
    root_dir: PathBuf,
    config_hooks: Vec<ConfigHook>,
    setup_hooks: Vec<SetupHook>,
    commands: Vec<(&'static str, Arc<dyn ErasedCommand>)>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Creates an application anchored at `FORGE_MANIFEST_DIR` (when invoked via `forge dev`)
    /// or the current working directory.
    pub fn new() -> Self {
        let root = std::env::var("FORGE_MANIFEST_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::from_root(root)
    }

    /// Creates an application anchored at an explicit project root directory
    /// (ideal for isolated parallel integration tests).
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root.into(),
            config_hooks: Vec::new(),
            setup_hooks: Vec::new(),
            commands: Vec::new(),
        }
    }

    /// Programmatically overrides values loaded from `forge.toml`.
    pub fn configure<F>(mut self, override_fn: F) -> Self
    where
        F: FnOnce(&mut Config) + Send + 'static,
    {
        self.config_hooks.push(Box::new(override_fn));
        self
    }

    /// Registers shared in-memory state `T` accessible to handlers via [`State<T>`].
    pub fn state<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.setup(move |ctx| {
            ctx.insert_state(value);
            Ok(())
        })
    }

    /// Registers a lazy initialization hook executed once inside the primary host process
    /// after Tokio starts and before any window opens or command executes.
    pub fn setup<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(&mut SetupContext) -> Result<()> + Send + 'static,
    {
        self.setup_hooks.push(Box::new(hook));
        self
    }

    /// Applies a composable [`Plugin`] module to this application.
    pub fn plugin(self, plugin: impl Plugin) -> Self {
        plugin.apply(self)
    }

    /// Registers a typed async IPC command reachable from JavaScript via `window.forge.invoke(name, payload)`.
    pub fn command<H, Marker>(mut self, name: &'static str, handler: H) -> Self
    where
        H: CommandHandler<Marker>,
        Marker: 'static,
    {
        self.commands
            .push((name, Arc::new(CommandEntry::new(handler))));
        self
    }

    /// Boots the selected desktop runtime (`webview` or `cef`), opens the application window,
    /// and blocks the OS main thread until the window closes.
    pub fn run(self) -> Result<()> {
        // Phase 1: Intercept CEF helper subprocesses BEFORE checking main-thread/Tokio
        // or running any user setup hooks.
        let early_kind = Config::peek_runtime_kind(&self.root_dir);
        if let ProcessDisposition::SubprocessExited(_code) = runtime::enter_process(early_kind)? {
            return Ok(());
        }

        // Enforce main-thread & non-Tokio entry invariant for native GUI loops.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(Error::Config {
                path: None,
                message: "do not annotate `fn main()` with `#[tokio::main]`; FORGE manages its own Tokio runtime".into(),
            });
        }
        if let Some(thread_name) = std::thread::current().name()
            && thread_name != "main"
        {
            return Err(Error::NotMainThread);
        }

        let tokio_rt = build_tokio_runtime()?;
        let (kernel, directives, _guard) = self.build_kernel(tokio_rt.handle().clone())?;
        let active_kind = kernel.config.runtime.default;

        // Re-verify runtime availability if a `.configure(...)` hook changed `runtime.default`.
        if active_kind != early_kind {
            runtime::enter_process(active_kind)?;
        }

        let res = runtime::run_host(active_kind, kernel, directives);
        drop(tokio_rt);
        res
    }

    /// Initializes configuration, setup hooks, startup `State<T>` validation, and the command
    /// router in memory without opening an OS window—returning a [`HeadlessApp`] test harness.
    pub fn headless(self) -> Result<HeadlessApp> {
        let (handle, owned_rt) = match tokio::runtime::Handle::try_current() {
            Ok(h) => (h, None),
            Err(_) => {
                let rt = build_tokio_runtime()?;
                (rt.handle().clone(), Some(rt))
            }
        };
        let (kernel, directives, ()) = self.build_kernel(handle)?;
        Ok(HeadlessApp::new(kernel, directives, owned_rt))
    }

    fn build_kernel(
        self,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<(
        Arc<ForgeKernel>,
        tokio::sync::mpsc::UnboundedReceiver<runtime::HostDirective>,
        (),
    )> {
        let mut config = Config::load_from_root(&self.root_dir)?;
        for hook in self.config_hooks {
            hook(&mut config);
        }

        let mut command_map: HashMap<&'static str, Arc<dyn ErasedCommand>> =
            HashMap::with_capacity(self.commands.len());
        for (name, cmd) in self.commands {
            if name.trim().is_empty()
                || name.starts_with("__forge")
                || name.contains('/')
                || name.contains('\0')
            {
                return Err(Error::ReservedCommand { command: name });
            }
            if command_map.insert(name, cmd).is_some() {
                return Err(Error::DuplicateCommand { command: name });
            }
        }

        let mut states = StateMap::default();
        {
            let _enter = tokio_handle.enter();
            let mut setup_ctx = SetupContext {
                states: &mut states,
                config: &config,
                tokio_handle: &tokio_handle,
            };
            for hook in self.setup_hooks {
                hook(&mut setup_ctx)?;
            }
        }

        // Startup TypeId verification: fail fast before opening any window if a command
        // requests State<T> that was never registered.
        for (&cmd_name, cmd) in &command_map {
            for (type_id, type_name) in cmd.required_states() {
                if !states.contains_type_id(&type_id) {
                    return Err(Error::MissingState {
                        command: cmd_name,
                        type_name,
                    });
                }
            }
        }

        let (window_handle, directives) = WindowHandle::new(config.window.title.clone());
        let kernel = ForgeKernel::new(
            &self.root_dir,
            config,
            states,
            command_map,
            window_handle,
            tokio_handle,
        )?;

        Ok((Arc::new(kernel), directives, ()))
    }
}

fn build_tokio_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Runtime(format!("failed to initialize Tokio runtime: {e}")))
}
