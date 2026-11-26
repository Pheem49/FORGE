use std::fs::File;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use tar::Builder;

use crate::bundle::staged::StagedApp;
use crate::bundle::target::BundleTarget;
use crate::dev::ProcessRunner;
use crate::{CliError, Result};

pub struct TarballTarget;

impl BundleTarget for TarballTarget {
    fn id(&self) -> &'static str {
        "tarball"
    }

    fn supported_on_host(&self) -> bool {
        true
    }

    fn package(
        &self,
        app: &StagedApp,
        _runner: &dyn ProcessRunner,
        out_dir: &Path,
    ) -> Result<PathBuf> {
        let archive_name = format!(
            "{}-{}-{}-{}.tar.gz",
            app.manifest.app.name,
            app.manifest.app.version,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let tar_path = out_dir.join(archive_name);

        let file = File::create(&tar_path).map_err(|e| CliError::Io {
            path: tar_path.clone(),
            message: format!("failed to create tarball: {e}"),
        })?;
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = Builder::new(enc);
        tar.append_dir_all(&app.manifest.app.name, &app.app_dir)
            .map_err(|e| CliError::Io {
                path: tar_path.clone(),
                message: format!("failed to write bundle tarball: {e}"),
            })?;
        let enc = tar.into_inner().map_err(|e| CliError::Io {
            path: tar_path.clone(),
            message: format!("failed to finalize bundle tarball: {e}"),
        })?;
        enc.finish().map_err(|e| CliError::Io {
            path: tar_path.clone(),
            message: format!("failed to finish bundle gzip: {e}"),
        })?;

        Ok(tar_path)
    }
}
