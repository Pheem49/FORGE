use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use tar::Archive;

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

/// Optional frontend dev-server settings (`[dev]` in `forge.toml`) for Vite / React / Vue HMR.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DevConfig {
    pub url: Option<String>,
    pub before_dev_command: Option<String>,
    pub before_build_command: Option<String>,
}

/// Resolved configuration for a FORGE application.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub app: AppMetaConfig,
    pub window: WindowConfig,
    pub runtime: RuntimeConfig,
    pub dev: DevConfig,
    #[serde(skip)]
    pub(crate) in_memory_assets: Option<HashMap<String, Vec<u8>>>,
}

impl Config {
    /// Registers an in-memory static asset (useful for single-file demos or headless tests).
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

    /// Unpacks a `.tar.gz` archive of static UI assets into the in-memory asset map.
    pub fn load_embedded_tar_gz(&mut self, archive_bytes: &[u8]) -> Result<&mut Self> {
        let map = unpack_tar_gz_assets(archive_bytes)?;
        let target = self.in_memory_assets.get_or_insert_with(HashMap::new);
        target.extend(map);
        Ok(self)
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
        } else if let Some(embedded_toml) = crate::kernel::EMBEDDED_MANIFEST_TOML {
            toml::from_str::<Config>(embedded_toml).map_err(|e| Error::Config {
                path: None,
                message: format!("invalid embedded forge.toml: {e}"),
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

        if let Ok(dev_url) = std::env::var("FORGE_DEV_URL")
            && !dev_url.trim().is_empty()
        {
            cfg.dev.url = Some(dev_url.trim().to_string());
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
        if let Some(embedded_toml) = crate::kernel::EMBEDDED_MANIFEST_TOML
            && let Ok(parsed) = toml::from_str::<Config>(embedded_toml)
        {
            return parsed.runtime.default;
        }
        RuntimeKind::Webview
    }
}

pub(crate) fn unpack_tar_gz_assets(archive_bytes: &[u8]) -> Result<HashMap<String, Vec<u8>>> {
    let gz = GzDecoder::new(archive_bytes);
    let mut archive = Archive::new(gz);
    let mut assets = HashMap::new();

    let entries = archive.entries().map_err(|e| Error::Config {
        path: None,
        message: format!("invalid embedded UI archive: {e}"),
    })?;

    for entry_res in entries {
        let mut entry = entry_res.map_err(|e| Error::Config {
            path: None,
            message: format!("corrupt embedded UI archive entry: {e}"),
        })?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let rel_path = entry
            .path()
            .map_err(|e| Error::Config {
                path: None,
                message: format!("invalid embedded asset path: {e}"),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let clean = rel_path.trim_start_matches("./").trim_start_matches('/');
        if clean.is_empty() {
            continue;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf).map_err(|e| Error::Config {
            path: None,
            message: format!("failed to decompress embedded asset '{clean}': {e}"),
        })?;
        assets.insert(format!("/{clean}"), buf);
    }

    Ok(assets)
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
