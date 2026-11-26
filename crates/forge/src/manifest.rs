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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppMetaConfig {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub identifier: String,
    pub assets: PathBuf,
    pub icon: Option<PathBuf>,
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub resizable: bool,
    pub decorations: bool,
    pub devtools: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "FORGE App".to_string(),
            width: 1024,
            height: 768,
            min_width: None,
            min_height: None,
            resizable: true,
            decorations: true,
            devtools: true,
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

/// Shared declarative `forge.toml` schema consumed by both the FORGE runtime and `forge-cli`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub app: AppMetaConfig,
    pub window: WindowConfig,
    pub runtime: RuntimeConfig,
    pub dev: DevConfig,
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
