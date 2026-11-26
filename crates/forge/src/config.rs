use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use tar::Archive;

use crate::error::{Error, Result};
pub use crate::manifest::{
    AppMetaConfig, DevConfig, Manifest, MenuConfig, MenuItemConfig, RuntimeConfig, RuntimeKind,
    SubmenuConfig, TrayConfig, WindowConfig,
};

/// Resolved configuration for a FORGE application.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub app: AppMetaConfig,
    pub window: WindowConfig,
    pub windows: BTreeMap<String, WindowConfig>,
    pub runtime: RuntimeConfig,
    pub dev: DevConfig,
    pub tray: Option<TrayConfig>,
    pub menu: Option<MenuConfig>,
    #[serde(skip)]
    pub(crate) in_memory_assets: Option<HashMap<String, Vec<u8>>>,
    #[serde(skip)]
    pub(crate) explicit_state_dir: Option<PathBuf>,
}

impl From<Manifest> for Config {
    fn from(m: Manifest) -> Self {
        Self {
            app: m.app,
            window: m.window,
            windows: m.windows,
            runtime: m.runtime,
            dev: m.dev,
            tray: m.tray,
            menu: m.menu,
            in_memory_assets: None,
            explicit_state_dir: None,
        }
    }
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

    /// Overrides the directory used for persisting window geometry (`window-state.json`).
    pub fn with_state_dir(&mut self, dir: impl Into<PathBuf>) -> &mut Self {
        self.explicit_state_dir = Some(dir.into());
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
        let manifest = if manifest_path.is_file() {
            Manifest::load(&manifest_path)?
        } else if let Some(embedded_toml) = crate::kernel::EMBEDDED_MANIFEST_TOML {
            Manifest::parse(embedded_toml).map_err(|e| Error::Config {
                path: None,
                message: format!("invalid embedded forge.toml: {e}"),
            })?
        } else {
            Manifest::default()
        };
        let mut cfg = Config::from(manifest);

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
        if manifest_path.is_file()
            && let Ok(parsed) = Manifest::load(&manifest_path)
        {
            return parsed.runtime.default;
        }
        if let Some(embedded_toml) = crate::kernel::EMBEDDED_MANIFEST_TOML
            && let Ok(parsed) = Manifest::parse(embedded_toml)
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
