use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::Compression;
use flate2::write::GzEncoder;
use tar::Builder;

use crate::cef_provisioner::CefProvisioner;
use crate::dev::{
    CommandSpec, PartialCargoToml, PartialForgeManifest, ProcessRunner, inject_library_search_path,
    load_manifest, parse_cargo_toml, resolve_cargo_feature_args, resolve_runtime,
    shell_command_spec,
};
use crate::{BundleFormat, CliError, Result, RuntimeChoice};

const DEFAULT_FORGE_SVG_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">
  <rect width="128" height="128" rx="24" fill="#181c20"/>
  <path d="M24 44h80l-12 20H40v18h36l-8 16H24V44z" fill="#e58529"/>
  <rect x="24" y="96" width="80" height="8" rx="4" fill="#2fa89a"/>
</svg>
"##;

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub manifest_path: Option<PathBuf>,
    pub runtime_override: Option<RuntimeChoice>,
    pub format: BundleFormat,
    pub out_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct BuildArtifacts {
    pub app_dir: PathBuf,
    pub binary_path: PathBuf,
    pub tarball_path: Option<PathBuf>,
    pub deb_path: Option<PathBuf>,
}

/// Compiles a release binary with single-file embedded UI assets and packages distributable
/// desktop bundles (`dist/<app>/`, `.tar.gz`, and `.deb`).
pub fn run_build<P, R>(opts: &BuildOptions, provisioner: &P, runner: &R) -> Result<BuildArtifacts>
where
    P: CefProvisioner,
    R: ProcessRunner,
{
    let (manifest_file, project_root, parsed) = load_manifest(opts.manifest_path.as_deref())?;
    let active_runtime = resolve_runtime(&manifest_file, &parsed, opts.runtime_override)?;

    let cargo_toml_path = project_root.join("Cargo.toml");
    if !cargo_toml_path.is_file() {
        return Err(CliError::Manifest {
            path: cargo_toml_path,
            message: "Cargo.toml not found alongside forge.toml".into(),
        });
    }
    let cargo_meta = parse_cargo_toml(&cargo_toml_path)?;
    let bin_name = infer_binary_name(&cargo_meta, &parsed.app.name);

    // 1. Run optional frontend build step (e.g., `npm run build` for Vite/React)
    if let Some(build_cmd) = parsed
        .dev
        .before_build_command
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        let spec = shell_command_spec(build_cmd, &project_root);
        let code = runner.run(&spec)?;
        if code != 0 {
            return Err(CliError::ProcessFailed {
                command: build_cmd.to_string(),
                code,
            });
        }
    }

    // 2. Verify UI assets directory & compress into deterministic `.tar.gz` archive
    let assets_dir = if parsed.app.assets.is_absolute() {
        parsed.app.assets.clone()
    } else {
        project_root.join(&parsed.app.assets)
    };
    if !assets_dir.is_dir() {
        return Err(CliError::Manifest {
            path: manifest_file,
            message: format!(
                "UI assets directory '{}' not found for bundling",
                assets_dir.display()
            ),
        });
    }

    let cargo_target_dir = resolve_cargo_target_dir(&project_root);
    let forge_staging = cargo_target_dir.join("forge-build");
    create_dir_all(&forge_staging)?;

    let ui_archive_path = forge_staging.join(format!("{}-ui.tar.gz", parsed.app.name));
    pack_directory_to_tar_gz(&assets_dir, &ui_archive_path)?;

    // 3. Build release executable with embedded UI archive
    let feature_args = resolve_cargo_feature_args(&cargo_toml_path, active_runtime)?;
    let mut args = vec!["build".to_string(), "--release".to_string()];
    args.extend(feature_args);

    let mut env = BTreeMap::new();
    env.insert(
        "FORGE_EMBED_ARCHIVE".to_string(),
        ui_archive_path.to_string_lossy().into_owned(),
    );
    env.insert(
        "FORGE_EMBED_MANIFEST".to_string(),
        manifest_file.to_string_lossy().into_owned(),
    );
    env.insert(
        "FORGE_RUNTIME".to_string(),
        active_runtime.as_str().to_string(),
    );

    let maybe_cef_dir = if active_runtime == RuntimeChoice::Cef {
        let cef_dir = provisioner.ensure_cef_ready()?;
        let cef_str = cef_dir.to_string_lossy().into_owned();
        env.insert("CEF_PATH".to_string(), cef_str.clone());
        inject_library_search_path(&mut env, &cef_str);
        Some(cef_dir)
    } else {
        None
    };

    let build_spec = CommandSpec {
        program: "cargo".to_string(),
        args,
        current_dir: project_root.clone(),
        env,
    };
    let code = runner.run(&build_spec)?;
    if code != 0 {
        return Err(CliError::ProcessFailed {
            command: format!("cargo {}", build_spec.args.join(" ")),
            code,
        });
    }

    let exe_suffix = std::env::consts::EXE_SUFFIX;
    let built_bin = cargo_target_dir
        .join("release")
        .join(format!("{bin_name}{exe_suffix}"));
    if !built_bin.is_file() {
        return Err(CliError::Io {
            path: built_bin,
            message: "expected compiled release binary was not found".into(),
        });
    }

    // 4. Stage portable output folder `dist/<app-name>/`
    let dist_root = opts
        .out_dir
        .clone()
        .unwrap_or_else(|| project_root.join("dist"));
    let app_bundle_dir = dist_root.join(&parsed.app.name);
    if app_bundle_dir.exists() {
        let _ = fs::remove_dir_all(&app_bundle_dir);
    }
    create_dir_all(&app_bundle_dir)?;

    // Also include a minimal runtime `forge.toml` metadata copy? Wait: better yet, embed default
    // window metadata or write `forge.toml` only when needed? Actually, copying `forge.toml` is NOT
    // needed if standalone binary runs anywhere, EXCEPT `App::new()` falls back to defaults if
    // `forge.toml` is missing! Even cleaner: let's stage `dist/<app-name>/<bin>` AND for `.deb`
    // install cleanly!
    let staged_bin = app_bundle_dir.join(format!("{bin_name}{exe_suffix}"));
    fs::copy(&built_bin, &staged_bin).map_err(|e| CliError::Io {
        path: staged_bin.clone(),
        message: format!("failed to copy release binary: {e}"),
    })?;
    set_executable(&staged_bin);
    try_strip_binary(&staged_bin);

    if let Some(cef_dir) = &maybe_cef_dir {
        stage_cef_runtime_files(cef_dir, &app_bundle_dir)?;
        try_patch_linux_rpath_origin(runner, &staged_bin);
    }

    // 5. Optional `.tar.gz` portable archive
    let tarball_path = if matches!(opts.format, BundleFormat::All | BundleFormat::Tarball) {
        let archive_name = format!(
            "{}-{}-{}-{}.tar.gz",
            parsed.app.name,
            parsed.app.version,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let tar_path = dist_root.join(archive_name);
        pack_app_bundle_tarball(&app_bundle_dir, &parsed.app.name, &tar_path)?;
        Some(tar_path)
    } else {
        None
    };

    // 6. Optional Debian/Ubuntu `.deb` installer package
    let deb_path = if matches!(opts.format, BundleFormat::All | BundleFormat::Deb) {
        if command_exists("dpkg-deb") {
            Some(build_deb_package(
                &project_root,
                &forge_staging,
                &dist_root,
                &app_bundle_dir,
                &bin_name,
                &parsed,
                active_runtime,
            )?)
        } else if opts.format == BundleFormat::Deb {
            return Err(CliError::InvalidArgument(
                "`dpkg-deb` is not installed on this system; cannot build `.deb` package".into(),
            ));
        } else {
            None
        }
    } else {
        None
    };

    Ok(BuildArtifacts {
        app_dir: app_bundle_dir,
        binary_path: staged_bin,
        tarball_path,
        deb_path,
    })
}

fn infer_binary_name(cargo_meta: &PartialCargoToml, fallback: &str) -> String {
    if let Some(first_bin) = cargo_meta.bin.first() {
        return first_bin.name.clone();
    }
    if let Some(pkg) = &cargo_meta.package {
        return pkg.name.clone();
    }
    fallback.to_string()
}

fn resolve_cargo_target_dir(project_root: &Path) -> PathBuf {
    if let Ok(explicit) = std::env::var("CARGO_TARGET_DIR")
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }

    let mut cursor = Some(project_root);
    while let Some(dir) = cursor {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file()
            && let Ok(raw) = fs::read_to_string(&candidate)
            && raw.contains("[workspace]")
        {
            return dir.join("target");
        }
        cursor = dir.parent();
    }

    project_root.join("target")
}

pub fn pack_directory_to_tar_gz(src_dir: &Path, dest_archive: &Path) -> Result<()> {
    let file = File::create(dest_archive).map_err(|e| CliError::Io {
        path: dest_archive.to_path_buf(),
        message: format!("failed to create UI archive: {e}"),
    })?;
    let enc = GzEncoder::new(file, Compression::default());
    let mut tar = Builder::new(enc);

    append_dir_recursively(&mut tar, src_dir, src_dir)?;

    let enc = tar.into_inner().map_err(|e| CliError::Io {
        path: dest_archive.to_path_buf(),
        message: format!("failed to finalize tar archive: {e}"),
    })?;
    enc.finish().map_err(|e| CliError::Io {
        path: dest_archive.to_path_buf(),
        message: format!("failed to finish gzip stream: {e}"),
    })?;
    Ok(())
}

fn append_dir_recursively<W: std::io::Write>(
    tar: &mut Builder<W>,
    base_dir: &Path,
    current_dir: &Path,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(current_dir)
        .map_err(|e| CliError::Io {
            path: current_dir.to_path_buf(),
            message: format!("failed to read directory: {e}"),
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| CliError::Io {
            path: current_dir.to_path_buf(),
            message: format!("failed to iterate directory: {e}"),
        })?;
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_str().is_some_and(|s| s.starts_with('.')) {
            continue;
        }
        let meta = entry.metadata().map_err(|e| CliError::Io {
            path: path.clone(),
            message: format!("failed to stat asset: {e}"),
        })?;
        if meta.is_dir() {
            append_dir_recursively(tar, base_dir, &path)?;
        } else if meta.is_file() {
            let rel = path.strip_prefix(base_dir).unwrap_or(&path);
            tar.append_path_with_name(&path, rel)
                .map_err(|e| CliError::Io {
                    path: path.clone(),
                    message: format!("failed to archive asset: {e}"),
                })?;
        }
    }
    Ok(())
}

fn pack_app_bundle_tarball(bundle_dir: &Path, top_folder: &str, dest_tar_gz: &Path) -> Result<()> {
    let file = File::create(dest_tar_gz).map_err(|e| CliError::Io {
        path: dest_tar_gz.to_path_buf(),
        message: format!("failed to create tarball: {e}"),
    })?;
    let enc = GzEncoder::new(file, Compression::default());
    let mut tar = Builder::new(enc);
    tar.append_dir_all(top_folder, bundle_dir)
        .map_err(|e| CliError::Io {
            path: dest_tar_gz.to_path_buf(),
            message: format!("failed to write bundle tarball: {e}"),
        })?;
    let enc = tar.into_inner().map_err(|e| CliError::Io {
        path: dest_tar_gz.to_path_buf(),
        message: format!("failed to finalize bundle tarball: {e}"),
    })?;
    enc.finish().map_err(|e| CliError::Io {
        path: dest_tar_gz.to_path_buf(),
        message: format!("failed to finish bundle gzip: {e}"),
    })?;
    Ok(())
}

fn stage_cef_runtime_files(cef_dir: &Path, app_bundle_dir: &Path) -> Result<()> {
    let entries = fs::read_dir(cef_dir).map_err(|e| CliError::Io {
        path: cef_dir.to_path_buf(),
        message: format!("failed to read CEF directory: {e}"),
    })?;

    for entry_res in entries {
        let entry = entry_res.map_err(|e| CliError::Io {
            path: cef_dir.to_path_buf(),
            message: e.to_string(),
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if name == "locales" && path.is_dir() {
            copy_dir_all(&path, &app_bundle_dir.join("locales"))?;
            continue;
        }

        let should_copy = name.ends_with(".so")
            || name.contains(".so.")
            || name.ends_with(".dll")
            || name.ends_with(".pak")
            || name.ends_with(".dat")
            || name.ends_with(".bin")
            || name == "vk_swiftshader_icd.json";

        if should_copy && path.is_file() {
            let dst = app_bundle_dir.join(name);
            fs::copy(&path, &dst).map_err(|e| CliError::Io {
                path: dst,
                message: format!("failed to copy CEF runtime file '{name}': {e}"),
            })?;
        }
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
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

fn build_deb_package(
    project_root: &Path,
    forge_staging: &Path,
    dist_root: &Path,
    app_bundle_dir: &Path,
    bin_name: &str,
    manifest: &PartialForgeManifest,
    runtime: RuntimeChoice,
) -> Result<PathBuf> {
    let pkg_name = sanitize_deb_package_name(&manifest.app.name);
    let deb_arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };

    let deb_root = forge_staging.join(format!("deb-{pkg_name}"));
    if deb_root.exists() {
        let _ = fs::remove_dir_all(&deb_root);
    }

    let debian_dir = deb_root.join("DEBIAN");
    let usr_bin = deb_root.join("usr/bin");
    let apps_dir = deb_root.join("usr/share/applications");
    let icons_dir = deb_root.join("usr/share/icons/hicolor/scalable/apps");
    create_dir_all(&debian_dir)?;
    create_dir_all(&usr_bin)?;
    create_dir_all(&apps_dir)?;
    create_dir_all(&icons_dir)?;

    match runtime {
        RuntimeChoice::Webview => {
            let src_bin = app_bundle_dir.join(bin_name);
            let dst_bin = usr_bin.join(bin_name);
            fs::copy(&src_bin, &dst_bin).map_err(|e| CliError::Io {
                path: dst_bin.clone(),
                message: format!("failed to stage binary into .deb: {e}"),
            })?;
            set_executable(&dst_bin);
        }
        RuntimeChoice::Cef => {
            let opt_app = deb_root.join("opt").join(&pkg_name);
            copy_dir_all(app_bundle_dir, &opt_app)?;
            set_executable(&opt_app.join(bin_name));
            #[cfg(unix)]
            {
                let link_target = format!("/opt/{pkg_name}/{bin_name}");
                let link_path = usr_bin.join(bin_name);
                std::os::unix::fs::symlink(&link_target, &link_path).map_err(|e| CliError::Io {
                    path: link_path,
                    message: format!("failed to create /usr/bin symlink: {e}"),
                })?;
            }
        }
    }

    // Icon (.svg)
    let icon_id = &manifest.app.identifier;
    let dst_icon = icons_dir.join(format!("{icon_id}.svg"));
    if let Some(custom_icon) = &manifest.app.icon {
        let icon_src = if custom_icon.is_absolute() {
            custom_icon.clone()
        } else {
            project_root.join(custom_icon)
        };
        if icon_src.is_file() {
            fs::copy(&icon_src, &dst_icon).map_err(|e| CliError::Io {
                path: dst_icon.clone(),
                message: format!("failed to copy custom icon: {e}"),
            })?;
        } else {
            fs::write(&dst_icon, DEFAULT_FORGE_SVG_ICON).map_err(|e| CliError::Io {
                path: dst_icon.clone(),
                message: e.to_string(),
            })?;
        }
    } else {
        fs::write(&dst_icon, DEFAULT_FORGE_SVG_ICON).map_err(|e| CliError::Io {
            path: dst_icon.clone(),
            message: e.to_string(),
        })?;
    }

    // Desktop entry
    let desc = manifest
        .app
        .description
        .as_deref()
        .unwrap_or("Desktop application built with FORGE");
    let desktop_entry = format!(
        "[Desktop Entry]\nType=Application\nName={}\nComment={}\nExec=/usr/bin/{}\nIcon={}\nTerminal=false\nCategories=Utility;\n",
        manifest.window.title, desc, bin_name, icon_id
    );
    let desktop_file = apps_dir.join(format!("{icon_id}.desktop"));
    fs::write(&desktop_file, desktop_entry).map_err(|e| CliError::Io {
        path: desktop_file,
        message: e.to_string(),
    })?;

    // DEBIAN/control
    let depends = match runtime {
        RuntimeChoice::Webview => "libwebkit2gtk-4.1-0, libgtk-3-0",
        RuntimeChoice::Cef => "libgtk-3-0, libnss3, libx11-6",
    };
    let control = format!(
        "Package: {pkg_name}\nVersion: {}\nSection: utils\nPriority: optional\nArchitecture: {deb_arch}\nDepends: {depends}\nMaintainer: FORGE Builder <noreply@forge.local>\nDescription: {desc}\n",
        manifest.app.version
    );
    let control_path = debian_dir.join("control");
    fs::write(&control_path, control).map_err(|e| CliError::Io {
        path: control_path,
        message: e.to_string(),
    })?;

    let deb_filename = format!("{pkg_name}_{}_{deb_arch}.deb", manifest.app.version);
    let deb_out = dist_root.join(deb_filename);

    let status = Command::new("dpkg-deb")
        .args([
            "--root-owner-group",
            "--build",
            &deb_root.to_string_lossy(),
            &deb_out.to_string_lossy(),
        ])
        .output()
        .map_err(|e| CliError::Io {
            path: deb_out.clone(),
            message: format!("failed to run dpkg-deb: {e}"),
        })?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(CliError::ProcessFailed {
            command: format!("dpkg-deb ({})", stderr.trim()),
            code: status.status.code().unwrap_or(1),
        });
    }

    Ok(deb_out)
}

fn sanitize_deb_package_name(raw: &str) -> String {
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

fn command_exists(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn try_strip_binary(bin_path: &Path) {
    #[cfg(unix)]
    {
        let _ = Command::new("strip").arg(bin_path).output();
    }
    #[cfg(not(unix))]
    let _ = bin_path;
}

fn try_patch_linux_rpath_origin<R: ProcessRunner>(runner: &R, bin_path: &Path) {
    #[cfg(target_os = "linux")]
    if runner.has_tool("patchelf") {
        let cwd = bin_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = runner.run(&CommandSpec {
            program: "patchelf".to_string(),
            args: vec![
                "--set-rpath".to_string(),
                "$ORIGIN".to_string(),
                bin_path.to_string_lossy().into_owned(),
            ],
            current_dir: cwd,
            env: BTreeMap::new(),
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (runner, bin_path);
}

fn set_executable(path: &Path) {
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

fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        message: format!("failed to create directory: {e}"),
    })
}
