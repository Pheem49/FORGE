use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    pub identifier: String,
    pub assets: PathBuf,
}

impl Default for AppMetaConfig {
    fn default() -> Self {
        Self {
            name: "forge-app".to_string(),
            identifier: "dev.forge.app".to_string(),
            assets: PathBuf::from("ui"),
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
            devtools: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub default: RuntimeKind,
}

/// Resolved configuration for a FORGE application.
/// Loaded from `forge.toml` (single source of truth), layered with `FORGE_*` environment
/// overrides and optional programmatic [`crate::App::configure`] hooks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub app: AppMetaConfig,
    pub window: WindowConfig,
    pub runtime: RuntimeConfig,
    #[serde(skip)]
    pub(crate) in_memory_assets: Option<HashMap<String, Vec<u8>>>,
}

impl Config {
    /// Registers in-memory static assets (primarily useful for single-file demos or headless tests).
    pub fn with_memory_asset(
        &mut self,
        path: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> &mut Self {
        let mut normalized = path.into();
        if !normalized.starts_with('/') {
            normalized.insert(0, '/');
        }
        self.in_memory_assets
            .get_or_insert_with(HashMap::new)
            .insert(normalized, bytes.into());
        self
    }

    pub(crate) fn load_from_root(root_dir: &Path) -> Result<Self> {
        let manifest_path = root_dir.join("forge.toml");
        let mut cfg = if manifest_path.is_file() {
            let content = std::fs::read_to_string(&manifest_path).map_err(|e| Error::Config {
                path: Some(manifest_path.clone()),
                message: format!("failed to read file: {e}"),
            })?;
            toml::from_str::<Config>(&content).map_err(|e| Error::Config {
                path: Some(manifest_path),
                message: e.to_string(),
            })?
        } else {
            Config::default()
        };

        if let Ok(raw_runtime) = std::env::var("FORGE_RUNTIME")
            && !raw_runtime.trim().is_empty()
        {
            cfg.runtime.default =
                RuntimeKind::parse(&raw_runtime).ok_or_else(|| Error::Config {
                    path: None,
                    message: format!(
                        "invalid FORGE_RUNTIME value '{raw_runtime}' (expected 'webview' or 'cef')"
                    ),
                })?;
        }

        if let Ok(assets_override) = std::env::var("FORGE_ASSETS_DIR")
            && !assets_override.trim().is_empty()
        {
            cfg.app.assets = PathBuf::from(assets_override);
        }

        Ok(cfg)
    }

    /// Lightweight fast-path check for the active runtime before full initialization,
    /// allowing CEF helper subprocesses (`--type=*`) to exit immediately.
    pub(crate) fn peek_runtime_kind(root_dir: &Path) -> RuntimeKind {
        if let Ok(raw_runtime) = std::env::var("FORGE_RUNTIME")
            && let Some(kind) = RuntimeKind::parse(&raw_runtime)
        {
            return kind;
        }
        let manifest_path = root_dir.join("forge.toml");
        if let Ok(content) = std::fs::read_to_string(manifest_path)
            && let Ok(parsed) = toml::from_str::<Config>(&content)
        {
            return parsed.runtime.default;
        }
        RuntimeKind::Webview
    }
}

/// Read-only extractor providing access to the resolved [`Config`] inside command handlers.
#[derive(Clone, Debug)]
pub struct AppConfig(pub(crate) Arc<Config>);

impl Deref for AppConfig {
    type Target = Config;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
