use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::bundle::staged::{StagedApp, copy_dir_all, create_dir_all, set_executable};
use crate::bundle::target::BundleTarget;
use crate::dev::{CommandSpec, ProcessRunner};
use crate::{CliError, Result, RuntimeChoice};

pub struct MacAppTarget;

impl BundleTarget for MacAppTarget {
    fn id(&self) -> &'static str {
        "app"
    }

    fn supported_on_host(&self) -> bool {
        cfg!(target_os = "macos")
    }

    fn package(
        &self,
        app: &StagedApp,
        runner: &dyn ProcessRunner,
        out_dir: &Path,
    ) -> Result<PathBuf> {
        let bundle_name = format!("{}.app", app.manifest.window.title);
        let app_bundle = out_dir.join(bundle_name);
        if app_bundle.exists() {
            let _ = fs::remove_dir_all(&app_bundle);
        }

        let contents = app_bundle.join("Contents");
        let macos_dir = contents.join("MacOS");
        let resources_dir = contents.join("Resources");
        create_dir_all(&macos_dir)?;
        create_dir_all(&resources_dir)?;

        let dst_bin = macos_dir.join(&app.bin_name);
        fs::copy(&app.binary_path, &dst_bin).map_err(|e| CliError::Io {
            path: dst_bin.clone(),
            message: format!("failed to copy macOS binary: {e}"),
        })?;
        set_executable(&dst_bin);

        fs::copy(&app.icons.icns_path, resources_dir.join("icon.icns")).map_err(|e| {
            CliError::Io {
                path: resources_dir.join("icon.icns"),
                message: format!("failed to copy icon.icns: {e}"),
            }
        })?;

        if app.runtime == RuntimeChoice::Cef
            && let Some(cef_dir) = &app.cef_dir
        {
            if runner.has_tool("bundle-cef-app") {
                let _ = runner.run(&CommandSpec {
                    program: "bundle-cef-app".to_string(),
                    args: vec![
                        "--cef-path".to_string(),
                        cef_dir.to_string_lossy().into_owned(),
                        app_bundle.to_string_lossy().into_owned(),
                    ],
                    current_dir: app.project_root.clone(),
                    env: BTreeMap::new(),
                });
            } else {
                let frameworks_dir = contents.join("Frameworks");
                create_dir_all(&frameworks_dir)?;
                let fw_src = cef_dir.join("Chromium Embedded Framework.framework");
                if fw_src.is_dir() {
                    copy_dir_all(
                        &fw_src,
                        &frameworks_dir.join("Chromium Embedded Framework.framework"),
                    )?;
                }
            }
        }

        let copyright = app
            .manifest
            .bundle
            .copyright
            .as_deref()
            .unwrap_or("Copyright (c) FORGE App");
        let info_plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>{title}</string>
  <key>CFBundleExecutable</key>
  <string>{exe}</string>
  <key>CFBundleIconFile</key>
  <string>icon.icns</string>
  <key>CFBundleIdentifier</key>
  <string>{identifier}</string>
  <key>CFBundleName</key>
  <string>{title}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>{version}</string>
  <key>CFBundleVersion</key>
  <string>{version}</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>{copyright}</string>
</dict>
</plist>
"#,
            title = xml_escape(&app.manifest.window.title),
            exe = xml_escape(&app.bin_name),
            identifier = xml_escape(&app.manifest.app.identifier),
            version = xml_escape(&app.manifest.app.version),
            copyright = xml_escape(copyright),
        );

        let plist_path = contents.join("Info.plist");
        fs::write(&plist_path, info_plist).map_err(|e| CliError::Io {
            path: plist_path,
            message: format!("failed to write Info.plist: {e}"),
        })?;

        Ok(app_bundle)
    }
}

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
