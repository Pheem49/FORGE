pub mod config;
pub mod error;
mod handler;
mod headless;
mod kernel;
pub mod manifest;
pub mod plugins;
mod runtime;
pub(crate) mod sandbox;
pub mod single_instance;
pub mod window_state;

pub use http;
pub use tokio;

pub use config::{
    AppConfig, AppMetaConfig, Config, DevConfig, ExportedCommand, Manifest, MenuConfig,
    MenuItemConfig, PermissionMode, PermissionsConfig, RuntimeConfig, RuntimeKind, SubmenuConfig,
    TrayConfig, WindowConfig, WindowPermissionOverride,
};
pub use error::{Error, ErrorKind, ForgeError, Result, codes};
pub use handler::{CommandHandler, FromContext, State};
pub use headless::{HeadlessApp, RawResponse};
pub use runtime::{AppHandle, WindowHandle, WindowOptions};
pub use single_instance::{AcquireOutcome, SecondInstancePayload, SingleInstanceGuard};
pub use window_state::{MonitorBounds, SavedWindowGeometry, WindowStateStore};

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use handler::{CommandEntry, ErasedCommand, StateMap};
use kernel::ForgeKernel;
use runtime::{HostCommand, ProcessDisposition, WindowRegistry};

type SetupHook = Box<dyn FnOnce(&mut SetupContext) -> Result<()> + Send + 'static>;
type ConfigHook = Box<dyn FnOnce(&mut Config) + Send + 'static>;

/// Extension trait for grouping state initialization and commands into reusable modules.
pub trait Plugin {
    fn apply(self, app: App) -> App;
}

/// Context passed exclusively to [`App::setup`] hooks inside the primary host process.
/// Guaranteed never to run inside CEF helper subprocesses (`--type=renderer`, `--type=gpu-process`, etc.).
pub struct SetupContext<'a> {
    states: &'a mut StateMap,
    config: &'a Config,
    app_handle: AppHandle,
    window: WindowHandle,
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

    /// Returns an [`AppHandle`] capable of creating windows, broadcasting events, or configuring tray/menus.
    pub fn app_handle(&self) -> AppHandle {
        self.app_handle.clone()
    }

    /// Returns a thread-safe [`WindowHandle`] for the primary `"main"` window.
    pub fn window(&self) -> WindowHandle {
        self.window.clone()
    }

    /// Blocks on an async initialization future using FORGE's internal Tokio runtime.
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
    pub fn new() -> Self {
        let root = std::env::var("FORGE_MANIFEST_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::from_root(root)
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root.into(),
            config_hooks: Vec::new(),
            setup_hooks: Vec::new(),
            commands: Vec::new(),
        }
    }

    pub fn configure<F>(mut self, override_fn: F) -> Self
    where
        F: FnOnce(&mut Config) + Send + 'static,
    {
        self.config_hooks.push(Box::new(override_fn));
        self
    }

    pub fn state<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.setup(move |ctx| {
            ctx.insert_state(value);
            Ok(())
        })
    }

    pub fn setup<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(&mut SetupContext) -> Result<()> + Send + 'static,
    {
        self.setup_hooks.push(Box::new(hook));
        self
    }

    pub fn plugin(self, plugin: impl Plugin) -> Self {
        plugin.apply(self)
    }

    pub fn command<H, Marker>(mut self, name: &'static str, handler: H) -> Self
    where
        H: CommandHandler<Marker>,
        Marker: 'static,
    {
        self.commands
            .push((name, Arc::new(CommandEntry::new(handler))));
        self
    }

    pub fn run(self) -> Result<()> {
        if let Some(export_path) = export_commands_env_path() {
            let exported = self.export_commands()?;
            write_exported_commands(&export_path, &exported)?;
            return Ok(());
        }

        let early_kind = Config::peek_runtime_kind(&self.root_dir);
        if let ProcessDisposition::SubprocessExited(_code) = runtime::enter_process(early_kind)? {
            return Ok(());
        }

        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(Error::Config {
                path: None,
                message: "do not annotate `fn main()` with `#[tokio::main]`; FORGE manages its own Tokio runtime".into(),
            });
        }
        if std::thread::current().name() != Some("main") {
            return Err(Error::NotMainThread);
        }

        let tokio_rt = build_tokio_runtime()?;
        let (kernel, commands_rx) = self.build_kernel(tokio_rt.handle().clone())?;
        let active_kind = kernel.config.runtime.default;

        if active_kind != early_kind
            && let ProcessDisposition::SubprocessExited(_code) =
                runtime::enter_process(active_kind)?
        {
            drop(tokio_rt);
            return Ok(());
        }

        let _single_instance_guard = if kernel.config.app.single_instance {
            let lock_dir = SingleInstanceGuard::default_lock_dir();
            let args: Vec<String> = std::env::args().collect();
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let app_handle = AppHandle::new(Arc::clone(&kernel.registry));

            match SingleInstanceGuard::acquire(
                &lock_dir,
                &kernel.config.app.identifier,
                args,
                cwd,
                move |payload| {
                    if let Some(main_win) = app_handle.window("main") {
                        main_win.show();
                        main_win.focus();
                    }
                    let _ = app_handle.emit_all("app:second-instance", payload);
                },
            )? {
                AcquireOutcome::Primary(guard) => Some(guard),
                AcquireOutcome::SecondaryForwarded => {
                    drop(tokio_rt);
                    return Ok(());
                }
            }
        } else {
            None
        };

        let res = runtime::run_host(active_kind, kernel, commands_rx);
        drop(tokio_rt);
        res
    }

    pub fn headless(self) -> Result<HeadlessApp> {
        let export_path = export_commands_env_path();
        if let Some(path) = &export_path {
            let exported = self.collect_exported_commands();
            write_exported_commands(path, &exported)?;
        }

        let (handle, owned_rt) = match tokio::runtime::Handle::try_current() {
            Ok(h) => (h, None),
            Err(_) => {
                let rt = build_tokio_runtime()?;
                (rt.handle().clone(), Some(rt))
            }
        };
        let (kernel, commands_rx) = self.build_kernel(handle)?;
        Ok(HeadlessApp::new(kernel, commands_rx, owned_rt))
    }

    /// Returns the sorted list of registered commands (both user/plugin commands and built-ins)
    /// without starting a Tokio runtime or opening any native window.
    pub fn export_commands(&self) -> Result<Vec<ExportedCommand>> {
        for &(name, _) in &self.commands {
            if name.trim().is_empty()
                || name.starts_with("__forge")
                || name.contains('/')
                || name.contains('\0')
            {
                return Err(Error::ReservedCommand { command: name });
            }
        }
        Ok(self.collect_exported_commands())
    }

    fn collect_exported_commands(&self) -> Vec<ExportedCommand> {
        let mut user_set = std::collections::HashSet::with_capacity(self.commands.len());
        for &(name, _) in &self.commands {
            user_set.insert(name);
        }
        let mut full_map: HashMap<&'static str, Arc<dyn ErasedCommand>> = HashMap::new();
        for &(name, ref cmd) in &self.commands {
            full_map.insert(name, Arc::clone(cmd));
        }
        kernel::builtins::insert_builtin_commands(&mut full_map);

        let mut list: Vec<ExportedCommand> = full_map
            .keys()
            .map(|&name| ExportedCommand {
                name: name.to_string(),
                builtin: !user_set.contains(name),
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    fn build_kernel(
        self,
        tokio_handle: tokio::runtime::Handle,
    ) -> Result<(
        Arc<ForgeKernel>,
        tokio::sync::mpsc::UnboundedReceiver<HostCommand>,
    )> {
        let mut config = Config::load_from_root(&self.root_dir)?;
        for hook in self.config_hooks {
            hook(&mut config);
        }

        let mut user_command_names: Vec<&'static str> = Vec::with_capacity(self.commands.len());
        let mut command_map: HashMap<&'static str, Arc<dyn ErasedCommand>> =
            HashMap::with_capacity(self.commands.len() + 16);
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
            user_command_names.push(name);
        }
        kernel::builtins::insert_builtin_commands(&mut command_map);

        let all_registered: Vec<&str> = command_map.keys().copied().collect();
        let permissions = kernel::permissions::PermissionPolicy::compile(&config.permissions)?;
        let permission_warnings = permissions.validate(&all_registered, &user_command_names)?;
        for warn in &permission_warnings {
            eprintln!("[FORGE] warning: {warn}");
        }

        let (registry, commands_rx) = WindowRegistry::new(&config);
        let app_handle = AppHandle::new(Arc::clone(&registry));
        let main_window = match registry.window_handle("main") {
            Some(w) => w,
            None => app_handle
                .create_window(
                    "main",
                    WindowOptions::from_window_config(&config.window).visible(false),
                )
                .map_err(|e| Error::Config {
                    path: None,
                    message: e.message,
                })?,
        };

        let mut states = StateMap::default();
        {
            let _enter = tokio_handle.enter();
            let mut setup_ctx = SetupContext {
                states: &mut states,
                config: &config,
                app_handle,
                window: main_window,
                tokio_handle: &tokio_handle,
            };
            for hook in self.setup_hooks {
                hook(&mut setup_ctx)?;
            }
        }

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

        let kernel = ForgeKernel::new(
            &self.root_dir,
            config,
            states,
            command_map,
            permissions,
            permission_warnings,
            registry,
            tokio_handle,
        )?;

        Ok((Arc::new(kernel), commands_rx))
    }
}

fn export_commands_env_path() -> Option<PathBuf> {
    std::env::var("FORGE_EXPORT_COMMANDS")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
}

fn write_exported_commands(dest: &PathBuf, commands: &[ExportedCommand]) -> Result<()> {
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            Error::Runtime(format!(
                "failed to create FORGE_EXPORT_COMMANDS parent directory '{}': {e}",
                parent.display()
            ))
        })?;
    }
    let json = serde_json::to_vec_pretty(commands)
        .map_err(|e| Error::Runtime(format!("failed to serialize exported commands: {e}")))?;
    std::fs::write(dest, json).map_err(|e| {
        Error::Runtime(format!(
            "failed to write FORGE_EXPORT_COMMANDS to '{}': {e}",
            dest.display()
        ))
    })
}

fn build_tokio_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Runtime(format!("failed to initialize Tokio runtime: {e}")))
}
