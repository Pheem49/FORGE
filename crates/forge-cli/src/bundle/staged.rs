use std::fs;
use std::path::{Path, PathBuf};

use forge::manifest::Manifest;

use super::icons::IconSet;
use crate::{CliError, Result, RuntimeChoice};

/// Immutable staged application context passed to every [`super::target::BundleTarget`] adapter,
/// replacing wide positional argument lists.
#[derive(Debug, Clone)]
pub struct StagedApp {
    pub project_root: PathBuf,
    pub staging_dir: PathBuf,
    pub app_dir: PathBuf,
    pub binary_path: PathBuf,
    pub bin_name: String,
    pub manifest: Manifest,
    pub runtime: RuntimeChoice,
    pub cef_dir: Option<PathBuf>,
    pub icons: IconSet,
}

impl StagedApp {
    pub fn description(&self) -> &str {
        self.manifest
            .app
            .description
            .as_deref()
            .unwrap_or("Desktop application built with FORGE")
    }

    pub fn publisher(&self) -> &str {
        self.manifest
            .bundle
            .publisher
            .as_deref()
            .unwrap_or("FORGE Builder")
    }

    pub fn category(&self) -> &str {
        self.manifest
            .bundle
            .category
            .as_deref()
            .unwrap_or("Utility")
    }
}

pub(crate) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        message: format!("failed to create directory: {e}"),
    })
}

pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    create_dir_all(dst)?;
    for entry_res in fs::read_dir(src).map_err(|e| CliError::Io {
        path: src.to_path_buf(),
        message: e.to_string(),
    })? {
        let entry = entry_res.map_err(|e| CliError::Io {
            path: src.to_path_buf(),
            message: e.to_string(),
        })?;
        let p = entry.path();
        let target = dst.join(entry.file_name());
        if p.is_dir() {
            copy_dir_all(&p, &target)?;
        } else {
            fs::copy(&p, &target).map_err(|e| CliError::Io {
                path: target,
                message: e.to_string(),
            })?;
        }
    }
    Ok(())
}

pub(crate) fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}
