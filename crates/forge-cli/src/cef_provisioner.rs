use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use download_cef::{CefIndex, DEFAULT_TARGET, OsAndArch};

use crate::{CliError, Result};

/// Matches the `cef` / `cef-dll-sys` crate version used by FORGE v0.1.
pub const PINNED_CEF_CRATE_VERSION: &str = "156.7.0+156.0.31";

/// Seam abstracting CEF binary distribution provisioning so CLI workflows are testable
/// without network downloads.
pub trait CefProvisioner {
    /// Ensures a valid CEF SDK/runtime directory is ready on disk and returns its path
    /// (compatible with `CEF_PATH` and dynamic library loaders).
    fn ensure_cef_ready(&self) -> Result<PathBuf>;
}

/// Default production adapter downloading pre-built CEF archives into `~/.cache/forge/cef`.
#[derive(Debug, Default, Clone)]
pub struct CachedCefProvisioner {
    pub cache_root_override: Option<PathBuf>,
}

impl CachedCefProvisioner {
    pub fn cache_root(&self) -> PathBuf {
        match &self.cache_root_override {
            Some(p) => p.clone(),
            None => dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("forge")
                .join("cef"),
        }
    }
}

impl CefProvisioner for CachedCefProvisioner {
    fn ensure_cef_ready(&self) -> Result<PathBuf> {
        if let Ok(existing) = std::env::var("CEF_PATH") {
            let path = PathBuf::from(existing.trim());
            if path.is_dir()
                && download_cef::check_archive_json(
                    PINNED_CEF_CRATE_VERSION,
                    &path.to_string_lossy(),
                )
                .is_ok()
            {
                return Ok(path);
            }
        }

        let base_cache = match &self.cache_root_override {
            Some(p) => p.clone(),
            None => dirs::cache_dir()
                .ok_or_else(|| {
                    CliError::CefProvision("could not determine user cache directory".into())
                })?
                .join("forge")
                .join("cef"),
        };

        let cef_version = download_cef::default_version(PINNED_CEF_CRATE_VERSION);
        let target = DEFAULT_TARGET;
        let os_arch = OsAndArch::try_from(target).map_err(|e| {
            CliError::CefProvision(format!("unsupported host target {target}: {e}"))
        })?;

        let versioned_dir = base_cache.join(&cef_version);
        let extracted_target_dir = versioned_dir.join(os_arch.to_string());

        if is_valid_cef_dir(&extracted_target_dir) {
            return Ok(extracted_target_dir);
        }

        fs::create_dir_all(&versioned_dir).map_err(|e| CliError::Io {
            path: versioned_dir.clone(),
            message: format!("failed to create CEF cache directory: {e}"),
        })?;

        println!(
            "Provisioning Chromium (CEF {}) into {} ...",
            cef_version,
            extracted_target_dir.display()
        );

        let download_url = download_cef::default_download_url();
        let index = CefIndex::download_from(&download_url)
            .map_err(|e| CliError::CefProvision(format!("failed to download CEF index: {e}")))?;
        let platform = index.platform(target).map_err(|e| {
            CliError::CefProvision(format!("platform {target} missing in CEF index: {e}"))
        })?;
        let version = platform.version(&cef_version).map_err(|e| {
            CliError::CefProvision(format!("CEF version {cef_version} not found: {e}"))
        })?;

        let archive_path = version
            .download_archive_with_retry_from(
                &download_url,
                &versioned_dir,
                true,
                Duration::from_secs(15),
                3,
            )
            .map_err(|e| CliError::CefProvision(format!("archive download failed: {e}")))?;

        let out_dir =
            download_cef::extract_target_archive(target, &archive_path, &versioned_dir, true)
                .map_err(|e| CliError::CefProvision(format!("archive extraction failed: {e}")))?;

        let _ = fs::remove_file(&archive_path);

        version
            .write_archive_json(&out_dir)
            .map_err(|e| CliError::CefProvision(format!("failed to write archive.json: {e}")))?;

        Ok(out_dir)
    }
}

fn is_valid_cef_dir(path: &Path) -> bool {
    path.is_dir()
        && download_cef::check_archive_json(PINNED_CEF_CRATE_VERSION, &path.to_string_lossy())
            .is_ok()
}
