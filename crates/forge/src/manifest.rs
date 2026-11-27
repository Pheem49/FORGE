use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Desktop rendering engine selected for the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    #[default]
    #[serde(alias = "wry")]
    Webview,
    #[serde(alias = "chromium")]
    Cef,
}

impl RuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webview => "webview",
            Self::Cef => "cef",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "webview" | "wry" => Some(Self::Webview),
            "cef" | "chromium" => Some(Self::Cef),
            _ => None,
        }
    }
}

impl fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn default_true() -> bool {
    true
}

fn default_entry_url() -> String {
    "/index.html".to_string()
}

/// Action taken when the user clicks a window's native close button (`[window].on_close`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CloseBehavior {
    /// Destroys the window immediately (default desktop behavior).
    #[default]
    Close,
    /// Cancels destruction and hides the window instead (ideal for background tray apps).
    #[serde(alias = "hide_to_tray")]
    Hide,
    /// Cancels destruction and emits `window:close-requested` (`{ label }`) so UI/Rust can confirm and call `destroy()`.
    #[serde(alias = "emit_event", alias = "confirm")]
    Emit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppMetaConfig {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub identifier: String,
    pub assets: PathBuf,
    pub icon: Option<PathBuf>,
    pub single_instance: bool,
    #[serde(default = "default_true")]
    pub exit_on_last_window_close: bool,
    /// Custom URI schemes registered with the OS (`[app] schemes = ["myapp"]` -> `myapp://...`).
    pub schemes: Vec<String>,
}

impl Default for AppMetaConfig {
    fn default() -> Self {
        Self {
            name: "forge-app".to_string(),
            version: "0.1.0".to_string(),
            description: None,
            identifier: "dev.forge.app".to_string(),
            assets: PathBuf::from("ui"),
            icon: None,
            single_instance: false,
            exit_on_last_window_close: true,
            schemes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    #[serde(default = "default_entry_url")]
    pub url: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    #[serde(default = "default_true")]
    pub resizable: bool,
    #[serde(default = "default_true")]
    pub decorations: bool,
    #[serde(default = "default_true")]
    pub visible: bool,
    pub always_on_top: bool,
    pub on_close: CloseBehavior,
    #[serde(default = "default_true")]
    pub devtools: bool,
    pub remember_state: bool,
    #[serde(default = "default_true")]
    pub create_on_startup: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            url: default_entry_url(),
            title: "FORGE App".to_string(),
            width: 1024,
            height: 768,
            min_width: None,
            min_height: None,
            x: None,
            y: None,
            resizable: true,
            decorations: true,
            visible: true,
            always_on_top: false,
            on_close: CloseBehavior::Close,
            devtools: true,
            remember_state: false,
            create_on_startup: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub default: RuntimeKind,
}

/// Optional frontend dev-server settings (`[dev]` in `forge.toml`) for Vite / React / Vue / Svelte HMR.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DevConfig {
    pub url: Option<String>,
    pub before_dev_command: Option<String>,
    pub before_build_command: Option<String>,
}

/// Declarative menu item inside `[tray].menu` or `[[menu.submenu]].items`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuItemConfig {
    pub id: Option<String>,
    pub label: Option<String>,
    pub accelerator: Option<String>,
    pub separator: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for MenuItemConfig {
    fn default() -> Self {
        Self {
            id: None,
            label: None,
            accelerator: None,
            separator: false,
            enabled: true,
        }
    }
}

impl MenuItemConfig {
    pub fn item(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            label: Some(label.into()),
            accelerator: None,
            separator: false,
            enabled: true,
        }
    }

    pub fn separator() -> Self {
        Self {
            separator: true,
            ..Self::default()
        }
    }

    pub fn with_accelerator(mut self, accel: impl Into<String>) -> Self {
        self.accelerator = Some(accel.into());
        self
    }
}

/// Declarative system tray configuration (`[tray]` in `forge.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayConfig {
    pub icon: Option<PathBuf>,
    pub tooltip: Option<String>,
    pub menu: Vec<MenuItemConfig>,
}

/// Top-level application submenu (`[[menu.submenu]]` in `forge.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubmenuConfig {
    pub label: String,
    pub items: Vec<MenuItemConfig>,
}

/// Declarative application menu bar (`[menu]` in `forge.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuConfig {
    pub submenu: Vec<SubmenuConfig>,
}

/// Enforcement mode for `[permissions]` in `forge.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// Default strict capability enforcement: commands not matched by `allow` are rejected with `403`.
    #[default]
    Enforce,
    /// Development escape hatch: allows registered commands unless matched by `deny`.
    Permissive,
}

/// Per-window capability override (`[permissions.windows.<label>]`), replacing global `allow` for that window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPermissionOverride {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

/// Declarative command capability policy (`[permissions]` in `forge.toml`).
/// Defaults to `mode = "enforce"` with an empty `allow` list (deny-by-default).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    pub mode: PermissionMode,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub windows: BTreeMap<String, WindowPermissionOverride>,
}

impl PermissionsConfig {
    /// Convenience helper allowing all registered commands (`allow = ["*"]`).
    pub fn allow_all(&mut self) -> &mut Self {
        self.allow = vec!["*".to_string()];
        self
    }

    /// Switches `[permissions]` into `permissive` mode.
    pub fn set_permissive(&mut self) -> &mut Self {
        self.mode = PermissionMode::Permissive;
        self
    }
}

/// Command metadata exported when `FORGE_EXPORT_COMMANDS=<path>` is set (used by `forge permissions init` and `forge bindings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedCommand {
    pub name: String,
    pub builtin: bool,
}

/// macOS code-signing and Apple Notarization configuration (`[bundle.macos]` in `forge.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MacOsSignConfig {
    /// Apple Developer ID Application certificate name (or overridden via `APPLE_SIGNING_IDENTITY`).
    pub signing_identity: Option<String>,
    /// Optional path to a custom `entitlements.plist` relative to project root.
    pub entitlements: Option<PathBuf>,
    /// `xcrun notarytool` stored keychain profile name (or overridden via `APPLE_NOTARY_PROFILE`).
    #[serde(alias = "notary_keychain_profile")]
    pub notary_profile: Option<String>,
}

/// Windows Authenticode code-signing configuration (`[bundle.windows]` in `forge.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsSignConfig {
    /// SHA-1 certificate thumbprint in the Windows certificate store (or `WINDOWS_CERTIFICATE_THUMBPRINT`).
    pub certificate_thumbprint: Option<String>,
    /// RFC 3161 timestamp server URL (defaults to `http://timestamp.digicert.com`).
    pub timestamp_url: Option<String>,
}

/// Optional installer and package metadata (`[bundle]` in `forge.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BundleConfig {
    pub publisher: Option<String>,
    pub category: Option<String>,
    pub copyright: Option<String>,
    pub homepage: Option<String>,
    pub macos: MacOsSignConfig,
    pub windows: WindowsSignConfig,
}

/// Declarative Ed25519-verified auto-updater configuration (`[updater]` in `forge.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdaterConfig {
    /// Static `latest.json` endpoint URLs (`https://...`).
    pub endpoints: Vec<String>,
    /// Base64-encoded 32-byte Ed25519 public key generated by `forge signer keygen`.
    pub pubkey: Option<String>,
}

/// Per-platform signed artifact entry inside `latest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePlatformEntry {
    pub url: String,
    pub sha256: String,
    /// Base64-encoded 64-byte Ed25519 signature over [`canonical_update_signing_payload`].
    pub signature: String,
}

/// Static release manifest (`latest.json`) produced by `forge build` and consumed by `UpdaterPlugin`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateReleaseManifest {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, UpdatePlatformEntry>,
}

/// Returns the canonical `<os>-<arch>` key used in `latest.json` (`linux-x86_64`, `windows-x86_64`, `darwin-aarch64`, etc.).
pub fn updater_platform_key(os: &str, arch: &str) -> String {
    let norm_os = match os {
        "macos" => "darwin",
        other => other,
    };
    format!("{norm_os}-{arch}")
}

/// Returns the canonical updater platform key for the current target binary.
pub fn current_updater_platform_key() -> String {
    updater_platform_key(std::env::consts::OS, std::env::consts::ARCH)
}

/// Constructs the deterministic metadata-bound message signed by `forge signer` and verified by `UpdaterPlugin`,
/// binding `version`, `platform`, `url`, and lowercase `sha256_hex` together to prevent downgrade, URL-swap,
/// or cross-platform replay attacks.
pub fn canonical_update_signing_payload(
    version: &str,
    platform: &str,
    url: &str,
    sha256_hex: &str,
) -> Vec<u8> {
    format!(
        "v1\n{}\n{}\n{}\n{}",
        version.trim(),
        platform.trim(),
        url.trim(),
        sha256_hex.trim().to_ascii_lowercase()
    )
    .into_bytes()
}

/// Shared declarative `forge.toml` schema consumed by both the FORGE runtime and `forge-cli`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub app: AppMetaConfig,
    pub window: WindowConfig,
    pub windows: BTreeMap<String, WindowConfig>,
    pub runtime: RuntimeConfig,
    pub dev: DevConfig,
    pub permissions: PermissionsConfig,
    pub tray: Option<TrayConfig>,
    pub menu: Option<MenuConfig>,
    pub bundle: BundleConfig,
    pub updater: UpdaterConfig,
}

const RESERVED_SCHEMES: &[&str] = &[
    "about",
    "blob",
    "chrome",
    "data",
    "file",
    "forge",
    "ftp",
    "http",
    "https",
    "javascript",
    "mailto",
    "tel",
    "vbscript",
    "ws",
    "wss",
];

/// Validates a custom URI scheme name (`[app].schemes`) per RFC 3986 (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`)
/// and blocks reserved browser/system schemes.
pub fn validate_custom_scheme(scheme: &str) -> std::result::Result<(), String> {
    if scheme.is_empty() {
        return Err("custom URI scheme in [app].schemes cannot be empty".into());
    }
    if scheme != scheme.to_ascii_lowercase() {
        return Err(format!(
            "custom URI scheme '{scheme}' must be lowercase ASCII"
        ));
    }
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return Err("custom URI scheme cannot be empty".into());
    };
    if !first.is_ascii_lowercase() {
        return Err(format!(
            "custom URI scheme '{scheme}' must start with a lowercase ASCII letter"
        ));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
    {
        return Err(format!(
            "custom URI scheme '{scheme}' contains invalid characters (allowed: a-z, 0-9, '+', '-', '.')"
        ));
    }
    if RESERVED_SCHEMES.contains(&scheme) {
        return Err(format!(
            "custom URI scheme '{scheme}' is reserved and cannot be registered in [app].schemes"
        ));
    }
    Ok(())
}

impl Manifest {
    /// Validates semantic invariants on a parsed [`Manifest`].
    pub fn validate(&self, source_path: Option<&Path>) -> Result<()> {
        for scheme in &self.app.schemes {
            if let Err(msg) = validate_custom_scheme(scheme) {
                return Err(Error::Config {
                    path: source_path.map(Path::to_path_buf),
                    message: msg,
                });
            }
        }
        Ok(())
    }

    /// Parses a `forge.toml` string into a strongly typed [`Manifest`].
    pub fn parse(raw_toml: &str) -> Result<Self> {
        let parsed = toml::from_str::<Self>(raw_toml).map_err(|e| Error::Config {
            path: None,
            message: e.to_string(),
        })?;
        parsed.validate(None)?;
        Ok(parsed)
    }

    /// Reads and parses `forge.toml` from an explicit file path (or `<dir>/forge.toml` if a directory is given).
    pub fn load(path_or_dir: &Path) -> Result<Self> {
        let manifest_path = if path_or_dir.is_dir() {
            path_or_dir.join("forge.toml")
        } else {
            path_or_dir.to_path_buf()
        };
        let content = std::fs::read_to_string(&manifest_path).map_err(|e| Error::Config {
            path: Some(manifest_path.clone()),
            message: format!("failed to read file: {e}"),
        })?;
        let parsed = toml::from_str::<Self>(&content).map_err(|e| Error::Config {
            path: Some(manifest_path.clone()),
            message: e.to_string(),
        })?;
        parsed.validate(Some(&manifest_path))?;
        Ok(parsed)
    }
}
