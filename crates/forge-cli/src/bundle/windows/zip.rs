use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::bundle::staged::StagedApp;
use crate::bundle::target::BundleTarget;
use crate::dev::ProcessRunner;
use crate::{CliError, Result};

pub struct ZipTarget;

impl BundleTarget for ZipTarget {
    fn id(&self) -> &'static str {
        "zip"
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
        let zip_name = format!(
            "{}-{}-{}-{}.zip",
            app.manifest.app.name,
            app.manifest.app.version,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let zip_path = out_dir.join(zip_name);
        let file = File::create(&zip_path).map_err(|e| CliError::Io {
            path: zip_path.clone(),
            message: format!("failed to create zip archive: {e}"),
        })?;
        let mut writer = ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        append_dir_to_zip(
            &mut writer,
            &app.app_dir,
            &app.app_dir,
            &app.manifest.app.name,
            opts,
        )?;

        writer.finish().map_err(|e| CliError::Io {
            path: zip_path.clone(),
            message: format!("failed to finalize zip archive: {e}"),
        })?;

        Ok(zip_path)
    }
}

fn append_dir_to_zip<W: Write + std::io::Seek>(
    zip: &mut ZipWriter<W>,
    base_dir: &Path,
    current_dir: &Path,
    top_folder: &str,
    opts: SimpleFileOptions,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(current_dir)
        .map_err(|e| CliError::Io {
            path: current_dir.to_path_buf(),
            message: e.to_string(),
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| CliError::Io {
            path: current_dir.to_path_buf(),
            message: e.to_string(),
        })?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            append_dir_to_zip(zip, base_dir, &path, top_folder, opts)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base_dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let archive_path = format!("{top_folder}/{rel}");
            zip.start_file(&archive_path, opts)
                .map_err(|e| CliError::Io {
                    path: path.clone(),
                    message: format!("failed to start zip entry '{archive_path}': {e}"),
                })?;
            let mut f = File::open(&path).map_err(|e| CliError::Io {
                path: path.clone(),
                message: e.to_string(),
            })?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).map_err(|e| CliError::Io {
                path: path.clone(),
                message: e.to_string(),
            })?;
            zip.write_all(&buf).map_err(|e| CliError::Io {
                path: path.clone(),
                message: e.to_string(),
            })?;
        }
    }
    Ok(())
}
