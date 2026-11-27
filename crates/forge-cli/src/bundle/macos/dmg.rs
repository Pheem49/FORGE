use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::app::MacAppTarget;
use crate::bundle::sign::{SigningOptions, sign_macos_app_bundle};
use crate::bundle::staged::{StagedApp, create_dir_all};
use crate::bundle::target::BundleTarget;
use crate::dev::{CommandSpec, ProcessRunner};
use crate::{CliError, Result};

pub struct DmgTarget;

impl DmgTarget {
    /// Stages and inside-out signs `<Title>.app` inside `dmg_staging` *before* running `hdiutil create`
    /// so Apple `notarytool` and Gatekeeper validate both the `.dmg` container and the inner `.app`.
    pub fn package_signed(
        &self,
        app: &StagedApp,
        signing: &SigningOptions,
        runner: &dyn ProcessRunner,
        out_dir: &Path,
    ) -> Result<PathBuf> {
        let dmg_staging = app
            .staging_dir
            .join(format!("dmg-{}", app.manifest.app.name));
        if dmg_staging.exists() {
            let _ = fs::remove_dir_all(&dmg_staging);
        }
        create_dir_all(&dmg_staging)?;

        let staged_app = MacAppTarget.package(app, runner, &dmg_staging)?;
        let _ = sign_macos_app_bundle(runner, app, signing, &staged_app)?;

        let dmg_out = out_dir.join(format!(
            "{}-{}-{}.dmg",
            app.manifest.app.name,
            app.manifest.app.version,
            std::env::consts::ARCH
        ));
        if dmg_out.exists() {
            let _ = fs::remove_file(&dmg_out);
        }

        let spec = CommandSpec {
            program: "hdiutil".to_string(),
            args: vec![
                "create".to_string(),
                "-volname".to_string(),
                app.manifest.window.title.clone(),
                "-srcfolder".to_string(),
                dmg_staging.to_string_lossy().into_owned(),
                "-ov".to_string(),
                "-format".to_string(),
                "UDZO".to_string(),
                dmg_out.to_string_lossy().into_owned(),
            ],
            current_dir: app.project_root.clone(),
            env: BTreeMap::new(),
        };
        let code = runner.run(&spec)?;
        if code != 0 {
            return Err(CliError::ProcessFailed {
                command: "hdiutil create".to_string(),
                code,
            });
        }

        Ok(dmg_out)
    }
}

impl BundleTarget for DmgTarget {
    fn id(&self) -> &'static str {
        "dmg"
    }

    fn supported_on_host(&self) -> bool {
        cfg!(target_os = "macos")
    }

    fn required_tool(&self) -> Option<&'static str> {
        Some("hdiutil")
    }

    fn package(
        &self,
        app: &StagedApp,
        runner: &dyn ProcessRunner,
        out_dir: &Path,
    ) -> Result<PathBuf> {
        self.package_signed(app, &SigningOptions::default(), runner, out_dir)
    }
}
