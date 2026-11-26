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

/// Shared declarative `forge.toml` schema consumed by both the FORGE runtime and `forge-cli`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub app: AppMetaConfig,
    pub window: WindowConfig,
    pub windows: BTreeMap<String, WindowConfig>,
    pub runtime: RuntimeConfig,
    pub dev: DevConfig,
    pub tray: Option<TrayConfig>,
    pub menu: Option<MenuConfig>,
}

impl Manifest {
    /// Parses a `forge.toml` string into a strongly typed [`Manifest`].
    pub fn parse(raw_toml: &str) -> Result<Self> {
        toml::from_str::<Self>(raw_toml).map_err(|e| Error::Config {
            path: None,
            message: e.to_string(),
        })
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
        toml::from_str::<Self>(&content).map_err(|e| Error::Config {
            path: Some(manifest_path),
            message: e.to_string(),
        })
    }
}
