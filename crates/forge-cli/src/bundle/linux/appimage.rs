use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::bundle::staged::{StagedApp, copy_dir_all, create_dir_all, set_executable};
use crate::bundle::target::BundleTarget;
use crate::dev::{CommandSpec, ProcessRunner};
use crate::{CliError, Result};

pub struct AppImageTarget;

impl BundleTarget for AppImageTarget {
    fn id(&self) -> &'static str {
        "appimage"
    }

    fn supported_on_host(&self) -> bool {
        cfg!(target_os = "linux")
    }

    fn required_tool(&self) -> Option<&'static str> {
        Some("appimagetool")
    }

    fn package(
        &self,
        app: &StagedApp,
        runner: &dyn ProcessRunner,
        out_dir: &Path,
    ) -> Result<PathBuf> {
        let appdir = app
            .staging_dir
            .join(format!("{}.AppDir", app.manifest.app.name));
        if appdir.exists() {
            let _ = fs::remove_dir_all(&appdir);
        }

        let usr_bin = appdir.join("usr/bin");
        create_dir_all(&usr_bin)?;
        copy_dir_all(&app.app_dir, &usr_bin)?;
        set_executable(&usr_bin.join(&app.bin_name));

        let apprun_script = format!(
            "#!/bin/sh\nSELF_DIR=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexport LD_LIBRARY_PATH=\"$SELF_DIR/usr/bin:${{LD_LIBRARY_PATH:-}}\"\nexec \"$SELF_DIR/usr/bin/{}\" \"$@\"\n",
            app.bin_name
        );
        let apprun_path = appdir.join("AppRun");
        fs::write(&apprun_path, apprun_script).map_err(|e| CliError::Io {
            path: apprun_path.clone(),
            message: format!("failed to write AppRun: {e}"),
        })?;
        set_executable(&apprun_path);

        let icon_stem = &app.manifest.app.identifier;
        fs::copy(&app.icons.png_256, appdir.join(format!("{icon_stem}.png"))).map_err(|e| {
            CliError::Io {
                path: appdir.join(format!("{icon_stem}.png")),
                message: format!("failed to stage AppImage icon: {e}"),
            }
        })?;

        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName={}\nComment={}\nExec={}\nIcon={}\nCategories={};\nTerminal=false\n",
            app.manifest.window.title,
            app.description(),
            app.bin_name,
            icon_stem,
            app.category()
        );
        let desktop_path = appdir.join(format!("{icon_stem}.desktop"));
        fs::write(&desktop_path, desktop).map_err(|e| CliError::Io {
            path: desktop_path,
            message: format!("failed to write AppDir .desktop file: {e}"),
        })?;

        let out_file = out_dir.join(format!(
            "{}-{}-{}.AppImage",
            app.manifest.app.name,
            app.manifest.app.version,
            std::env::consts::ARCH
        ));

        let mut env = BTreeMap::new();
        env.insert("ARCH".to_string(), std::env::consts::ARCH.to_string());
        let spec = CommandSpec {
            program: "appimagetool".to_string(),
            args: vec![
                appdir.to_string_lossy().into_owned(),
                out_file.to_string_lossy().into_owned(),
            ],
            current_dir: app.project_root.clone(),
            env,
        };
        let code = runner.run(&spec)?;
        if code != 0 {
            return Err(CliError::ProcessFailed {
                command: "appimagetool".to_string(),
                code,
            });
        }

        Ok(out_file)
    }
}
