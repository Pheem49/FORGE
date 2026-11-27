use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::bundle::staged::{StagedApp, copy_dir_all, create_dir_all, set_executable};
use crate::bundle::target::BundleTarget;
use crate::dev::{CommandSpec, ProcessRunner};
use crate::{CliError, Result, RuntimeChoice};

pub struct DebTarget;

impl BundleTarget for DebTarget {
    fn id(&self) -> &'static str {
        "deb"
    }

    fn supported_on_host(&self) -> bool {
        cfg!(target_os = "linux")
    }

    fn required_tool(&self) -> Option<&'static str> {
        Some("dpkg-deb")
    }

    fn package(
        &self,
        app: &StagedApp,
        runner: &dyn ProcessRunner,
        out_dir: &Path,
    ) -> Result<PathBuf> {
        let pkg_name = sanitize_deb_package_name(&app.manifest.app.name);
        let deb_arch = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        };

        let deb_root = app.staging_dir.join(format!("deb-{pkg_name}"));
        if deb_root.exists() {
            let _ = fs::remove_dir_all(&deb_root);
        }

        let debian_dir = deb_root.join("DEBIAN");
        let usr_bin = deb_root.join("usr/bin");
        let apps_dir = deb_root.join("usr/share/applications");
        let icons_dir = deb_root.join("usr/share/icons/hicolor/scalable/apps");
        let png_icons_dir = deb_root.join("usr/share/icons/hicolor/256x256/apps");
        create_dir_all(&debian_dir)?;
        create_dir_all(&usr_bin)?;
        create_dir_all(&apps_dir)?;
        create_dir_all(&icons_dir)?;
        create_dir_all(&png_icons_dir)?;

        match app.runtime {
            RuntimeChoice::Webview => {
                let dst_bin = usr_bin.join(&app.bin_name);
                fs::copy(&app.binary_path, &dst_bin).map_err(|e| CliError::Io {
                    path: dst_bin.clone(),
                    message: format!("failed to stage binary into .deb: {e}"),
                })?;
                set_executable(&dst_bin);
            }
            RuntimeChoice::Cef => {
                let opt_app = deb_root.join("opt").join(&pkg_name);
                copy_dir_all(&app.app_dir, &opt_app)?;
                set_executable(&opt_app.join(&app.bin_name));
                #[cfg(unix)]
                {
                    let link_target = format!("/opt/{pkg_name}/{}", app.bin_name);
                    let link_path = usr_bin.join(&app.bin_name);
                    std::os::unix::fs::symlink(&link_target, &link_path).map_err(|e| {
                        CliError::Io {
                            path: link_path,
                            message: format!("failed to create /usr/bin symlink: {e}"),
                        }
                    })?;
                }
            }
        }

        let icon_id = &app.manifest.app.identifier;
        let _ = fs::copy(
            &app.icons.svg_path,
            icons_dir.join(format!("{icon_id}.svg")),
        );
        let _ = fs::copy(
            &app.icons.png_256,
            png_icons_dir.join(format!("{icon_id}.png")),
        );

        let (exec_suffix, mime_line) = if app.manifest.app.schemes.is_empty() {
            ("", String::new())
        } else {
            let handlers: String = app
                .manifest
                .app
                .schemes
                .iter()
                .map(|s| format!("x-scheme-handler/{s};"))
                .collect();
            (" %u", format!("MimeType={handlers}\n"))
        };

        let desktop_entry = format!(
            "[Desktop Entry]\nType=Application\nName={}\nComment={}\nExec=/usr/bin/{}{exec_suffix}\nIcon={}\nTerminal=false\nCategories={};\n{mime_line}",
            app.manifest.window.title,
            app.description(),
            app.bin_name,
            icon_id,
            app.category()
        );
        let desktop_file = apps_dir.join(format!("{icon_id}.desktop"));
        fs::write(&desktop_file, desktop_entry).map_err(|e| CliError::Io {
            path: desktop_file,
            message: e.to_string(),
        })?;

        let depends = match app.runtime {
            RuntimeChoice::Webview => "libwebkit2gtk-4.1-0, libgtk-3-0",
            RuntimeChoice::Cef => "libgtk-3-0, libnss3, libx11-6",
        };
        let control = format!(
            "Package: {pkg_name}\nVersion: {}\nSection: utils\nPriority: optional\nArchitecture: {deb_arch}\nDepends: {depends}\nMaintainer: {} <noreply@forge.local>\nDescription: {}\n",
            app.manifest.app.version,
            app.publisher(),
            app.description()
        );
        let control_path = debian_dir.join("control");
        fs::write(&control_path, control).map_err(|e| CliError::Io {
            path: control_path,
            message: e.to_string(),
        })?;

        let deb_filename = format!("{pkg_name}_{}_{deb_arch}.deb", app.manifest.app.version);
        let deb_out = out_dir.join(deb_filename);

        let spec = CommandSpec {
            program: "dpkg-deb".to_string(),
            args: vec![
                "--root-owner-group".to_string(),
                "--build".to_string(),
                deb_root.to_string_lossy().into_owned(),
                deb_out.to_string_lossy().into_owned(),
            ],
            current_dir: app.project_root.clone(),
            env: BTreeMap::new(),
        };
        let code = runner.run(&spec)?;
        if code != 0 {
            return Err(CliError::ProcessFailed {
                command: "dpkg-deb --build".to_string(),
                code,
            });
        }

        Ok(deb_out)
    }
}

pub(crate) fn sanitize_deb_package_name(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '.' || ch == '+') && !out.is_empty() {
            out.push(ch);
        } else if ch == '_' && !out.is_empty() {
            out.push('-');
        }
    }
    if out.len() < 2 {
        format!("forge-{out}")
    } else {
        out
    }
}
