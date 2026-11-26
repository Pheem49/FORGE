pub mod bindings;
pub mod bundle;
pub use bundle as build;
pub mod cef_provisioner;
pub mod dev;
pub mod doctor;
pub mod new;
pub mod permissions;

use std::fmt;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use forge::manifest::RuntimeKind;

pub type Result<T, E = CliError> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum CliError {
    InvalidArgument(String),
    Io { path: PathBuf, message: String },
    Manifest { path: PathBuf, message: String },
    CefProvision(String),
    ProcessFailed { command: String, code: i32 },
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(msg) => write!(f, "{msg}"),
            Self::Io { path, message } => write!(f, "{} ({})", message, path.display()),
            Self::Manifest { path, message } => {
                write!(f, "invalid manifest {}: {message}", path.display())
            }
            Self::CefProvision(msg) => write!(f, "CEF provisioning failed: {msg}"),
            Self::ProcessFailed { command, code } => {
                write!(f, "`{command}` exited with status code {code}")
            }
        }
    }
}

impl std::error::Error for CliError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RuntimeChoice {
    #[value(alias = "wry")]
    Webview,
    #[value(alias = "chromium")]
    Cef,
}

impl RuntimeChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webview => "webview",
            Self::Cef => "cef",
        }
    }
}

impl From<RuntimeKind> for RuntimeChoice {
    fn from(kind: RuntimeKind) -> Self {
        match kind {
            RuntimeKind::Webview => Self::Webview,
            RuntimeKind::Cef => Self::Cef,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum TemplateChoice {
    #[default]
    Vanilla,
    React,
    Vue,
    Svelte,
}

impl TemplateChoice {
    pub fn uses_npm(self) -> bool {
        matches!(self, Self::React | Self::Vue | Self::Svelte)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum BundleFormat {
    /// Produce every bundle format supported on the current host OS whose tools are available
    #[default]
    All,
    /// Produce only the standalone binary / portable directory in `dist/<app-name>/`
    Bin,
    /// Produce portable `.tar.gz` archive (Linux / macOS / Windows)
    Tarball,
    /// Produce Debian/Ubuntu `.deb` installer package (Linux)
    Deb,
    /// Produce Linux `.AppImage` bundle via `appimagetool` (Linux)
    Appimage,
    /// Produce Windows NSIS `.exe` installer via `makensis` (Windows)
    Nsis,
    /// Produce portable `.zip` archive (Windows / cross-OS)
    Zip,
    /// Produce macOS `<App>.app` bundle (macOS)
    App,
    /// Produce macOS `.dmg` disk image via `hdiutil` (macOS)
    Dmg,
}

#[derive(Debug, Parser)]
#[command(
    name = "forge",
    version,
    about = "FORGE — Rust Desktop Application Platform (WebView & Chromium)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Scaffold a new FORGE desktop application project
    New {
        /// Directory path (or project name) to create
        path: PathBuf,

        /// Starter UI template (`vanilla`, `react`, `vue`, or `svelte`)
        #[arg(long, value_enum, default_value_t = TemplateChoice::Vanilla)]
        template: TemplateChoice,

        /// Default runtime written into `forge.toml`
        #[arg(long, value_enum, default_value_t = RuntimeChoice::Webview)]
        runtime: RuntimeChoice,

        /// Optional local path to the `forge` crate
        #[arg(long)]
        forge_path: Option<PathBuf>,
    },
    /// Run a FORGE project in live development mode (`Hot Reload` & dev proxy)
    Dev {
        /// Optional path to `forge.toml` (defaults to `./forge.toml`)
        #[arg(long)]
        manifest_path: Option<PathBuf>,

        /// Override the runtime specified in `forge.toml`
        #[arg(long, value_enum)]
        runtime: Option<RuntimeChoice>,

        /// Build and run in Cargo `--release` mode
        #[arg(long)]
        release: bool,
    },
    /// Compile a release binary with embedded UI assets and package distributable bundles
    Build {
        /// Optional path to `forge.toml` (defaults to `./forge.toml`)
        #[arg(long)]
        manifest_path: Option<PathBuf>,

        /// Override the runtime specified in `forge.toml`
        #[arg(long, value_enum)]
        runtime: Option<RuntimeChoice>,

        /// Output bundle format (`all`, `bin`, `tarball`, `deb`, `appimage`, `nsis`, `zip`, `app`, `dmg`)
        #[arg(long, value_enum, default_value_t = BundleFormat::All)]
        format: BundleFormat,

        /// Output directory for packaged artifacts (defaults to `<project>/dist`)
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
    /// Manage declarative `[permissions]` policies in `forge.toml`
    Permissions {
        #[command(subcommand)]
        action: PermissionsAction,
    },
    /// Generate TypeScript `.d.ts` command declarations (`ForgeCommandName` + `ForgeCommandTypes`)
    Bindings {
        /// Optional path to `forge.toml` (defaults to `./forge.toml`)
        #[arg(long)]
        manifest_path: Option<PathBuf>,

        /// Output `.d.ts` path (defaults to `src-ui/forge-commands.d.ts` or `ui/forge-commands.d.ts`)
        #[arg(long = "out")]
        out_path: Option<PathBuf>,

        /// Override the runtime used while inspecting commands
        #[arg(long, value_enum)]
        runtime: Option<RuntimeChoice>,
    },
    /// Verify Rust, WebView, Chromium CEF, Node.js, and OS packaging toolchains
    Doctor {
        /// Optional path to `forge.toml` (defaults to `./forge.toml` if present)
        #[arg(long)]
        manifest_path: Option<PathBuf>,

        /// Restrict required checks to a specific runtime (`webview` or `cef`)
        #[arg(long, value_enum)]
        runtime: Option<RuntimeChoice>,
    },
}

#[derive(Debug, Subcommand)]
pub enum PermissionsAction {
    /// Inspect registered commands and generate a strict `[permissions]` allow-list in `forge.toml`
    Init {
        /// Optional path to `forge.toml` (defaults to `./forge.toml`)
        #[arg(long)]
        manifest_path: Option<PathBuf>,

        /// Override the runtime used while inspecting commands
        #[arg(long, value_enum)]
        runtime: Option<RuntimeChoice>,
    },
}
