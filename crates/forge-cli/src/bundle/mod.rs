pub mod icons;
pub mod linux;
pub mod macos;
pub mod staged;
pub mod target;
pub mod windows;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use tar::Builder;

use self::icons::IconSet;
use self::linux::{appimage::AppImageTarget, deb::DebTarget, tarball::TarballTarget};
use self::macos::{app::MacAppTarget, dmg::DmgTarget};
use self::staged::{StagedApp, copy_dir_all, create_dir_all, set_executable};
use self::target::BundleTarget;
use self::windows::{nsis::NsisTarget, zip::ZipTarget};
use crate::cef_provisioner::CefProvisioner;
use crate::dev::{
    CommandSpec, PartialCargoToml, ProcessRunner, inject_library_search_path, load_manifest,
    parse_cargo_toml, resolve_cargo_feature_args, resolve_runtime, shell_command_spec,
};
use crate::{BundleFormat, CliError, Result, RuntimeChoice};

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
    pub icons: IconSet,
    pub tarball_path: Option<PathBuf>,
    pub deb_path: Option<PathBuf>,
    pub appimage_path: Option<PathBuf>,
    pub nsis_path: Option<PathBuf>,
    pub zip_path: Option<PathBuf>,
    pub macos_app_path: Option<PathBuf>,
    pub dmg_path: Option<PathBuf>,
    pub checksums_path: Option<PathBuf>,
}

/// Compiles a release binary with single-file embedded UI assets, generates multi-OS icons,
/// runs requested [`BundleTarget`] adapters, and writes `SHA256SUMS` into `dist/`.
pub fn run_build<P, R>(opts: &BuildOptions, provisioner: &P, runner: &R) -> Result<BuildArtifacts>
where
    P: CefProvisioner,
    R: ProcessRunner,
{
    let (manifest_file, project_root, parsed) = load_manifest(opts.manifest_path.as_deref())?;
    let active_runtime = resolve_runtime(&parsed, opts.runtime_override);

    let cargo_toml_path = project_root.join("Cargo.toml");
    if !cargo_toml_path.is_file() {
        return Err(CliError::Manifest {
            path: cargo_toml_path,
            message: "Cargo.toml not found alongside forge.toml".into(),
        });
    }
    let cargo_meta = parse_cargo_toml(&cargo_toml_path)?;
    let bin_name = infer_binary_name(&cargo_meta, &parsed.app.name);

    // Fail fast before compiling if an explicit host-incompatible format was requested.
    validate_requested_format_on_host(opts.format, runner)?;

    // 1. Optional frontend build step (`npm run build`)
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

    // 2. Pack static UI directory into deterministic `.tar.gz`
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

    // 3. Generate multi-OS icon pipeline (`.svg`, `.png` set, `.ico`, `.icns`)
    let icons = IconSet::generate(&project_root, parsed.app.icon.as_deref(), &forge_staging)?;

    // 4. Build release executable with embedded UI archive & manifest
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

    // 5. Stage portable output folder `dist/<app-name>/`
    let dist_root = opts
        .out_dir
        .clone()
        .unwrap_or_else(|| project_root.join("dist"));
    let app_bundle_dir = dist_root.join(&parsed.app.name);
    if app_bundle_dir.exists() {
        let _ = fs::remove_dir_all(&app_bundle_dir);
    }
    create_dir_all(&app_bundle_dir)?;

    let staged_bin = app_bundle_dir.join(format!("{bin_name}{exe_suffix}"));
    fs::copy(&built_bin, &staged_bin).map_err(|e| CliError::Io {
        path: staged_bin.clone(),
        message: format!("failed to copy release binary: {e}"),
    })?;
    set_executable(&staged_bin);
    try_strip_binary(runner, &staged_bin);

    if let Some(cef_dir) = &maybe_cef_dir {
        stage_cef_runtime_files(cef_dir, &app_bundle_dir)?;
        try_patch_linux_rpath_origin(runner, &staged_bin);
    }

    let staged = StagedApp {
        project_root,
        staging_dir: forge_staging,
        app_dir: app_bundle_dir.clone(),
        binary_path: staged_bin.clone(),
        bin_name,
        manifest: parsed,
        runtime: active_runtime,
        cef_dir: maybe_cef_dir,
        icons: icons.clone(),
    };

    let mut artifacts = BuildArtifacts {
        app_dir: app_bundle_dir,
        binary_path: staged_bin,
        icons,
        tarball_path: None,
        deb_path: None,
        appimage_path: None,
        nsis_path: None,
        zip_path: None,
        macos_app_path: None,
        dmg_path: None,
        checksums_path: None,
    };

    if opts.format == BundleFormat::Bin {
        return Ok(artifacts);
    }

    let mut hashable_files = Vec::new();

    if should_run_target(&TarballTarget, opts.format, BundleFormat::Tarball, runner) {
        let p = TarballTarget.package(&staged, runner, &dist_root)?;
        hashable_files.push(p.clone());
        artifacts.tarball_path = Some(p);
    }
    if should_run_target(&DebTarget, opts.format, BundleFormat::Deb, runner) {
        let p = DebTarget.package(&staged, runner, &dist_root)?;
        hashable_files.push(p.clone());
        artifacts.deb_path = Some(p);
    }
    if should_run_target(&AppImageTarget, opts.format, BundleFormat::Appimage, runner) {
        let p = AppImageTarget.package(&staged, runner, &dist_root)?;
        hashable_files.push(p.clone());
        artifacts.appimage_path = Some(p);
    }
    if should_run_target(&NsisTarget, opts.format, BundleFormat::Nsis, runner) {
        let p = NsisTarget.package(&staged, runner, &dist_root)?;
        hashable_files.push(p.clone());
        artifacts.nsis_path = Some(p);
    }
    if should_run_target(&ZipTarget, opts.format, BundleFormat::Zip, runner) {
        let p = ZipTarget.package(&staged, runner, &dist_root)?;
        hashable_files.push(p.clone());
        artifacts.zip_path = Some(p);
    }
    if should_run_target(&MacAppTarget, opts.format, BundleFormat::App, runner) {
        let p = MacAppTarget.package(&staged, runner, &dist_root)?;
        artifacts.macos_app_path = Some(p);
    }
    if should_run_target(&DmgTarget, opts.format, BundleFormat::Dmg, runner) {
        let p = DmgTarget.package(&staged, runner, &dist_root)?;
        hashable_files.push(p.clone());
        artifacts.dmg_path = Some(p);
    }

    if !hashable_files.is_empty() {
        artifacts.checksums_path = write_sha256sums(&dist_root, &hashable_files)?;
    }

    Ok(artifacts)
}

fn validate_requested_format_on_host(
    format: BundleFormat,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let target: Option<&dyn BundleTarget> = match format {
        BundleFormat::All | BundleFormat::Bin => None,
        BundleFormat::Tarball => Some(&TarballTarget),
        BundleFormat::Deb => Some(&DebTarget),
        BundleFormat::Appimage => Some(&AppImageTarget),
        BundleFormat::Nsis => Some(&NsisTarget),
        BundleFormat::Zip => Some(&ZipTarget),
        BundleFormat::App => Some(&MacAppTarget),
        BundleFormat::Dmg => Some(&DmgTarget),
    };

    if let Some(t) = target {
        if !t.supported_on_host() {
            return Err(CliError::InvalidArgument(format!(
                "bundle format '{}' is not supported on host OS '{}' (build natively on the target OS or in CI)",
                t.id(),
                std::env::consts::OS
            )));
        }
        if let Some(tool) = t.required_tool()
            && !runner.has_tool(tool)
        {
            return Err(CliError::InvalidArgument(format!(
                "`{tool}` is not installed on this system; cannot build '{}' bundle (run `forge doctor` for install instructions)",
                t.id()
            )));
        }
    }
    Ok(())
}

fn should_run_target(
    target: &dyn BundleTarget,
    requested: BundleFormat,
    specific: BundleFormat,
    runner: &dyn ProcessRunner,
) -> bool {
    if requested == specific {
        return true;
    }
    if requested != BundleFormat::All {
        return false;
    }
    // Under `--format all`, produce OS-native packages whose external tools are installed.
    if specific == BundleFormat::Zip && !cfg!(target_os = "windows") {
        return false;
    }
    if specific == BundleFormat::Tarball && cfg!(target_os = "windows") {
        return false;
    }
    target.supported_on_host() && target.required_tool().is_none_or(|t| runner.has_tool(t))
}

fn write_sha256sums(dist_root: &Path, files: &[PathBuf]) -> Result<Option<PathBuf>> {
    let mut lines = Vec::new();
    for path in files {
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(path).map_err(|e| CliError::Io {
            path: path.clone(),
            message: format!("failed to read artifact for SHA256SUMS: {e}"),
        })?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{b:02x}");
        }
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("artifact");
        lines.push(format!("{hex}  {file_name}"));
    }
    if lines.is_empty() {
        return Ok(None);
    }
    lines.sort();
    let out_path = dist_root.join("SHA256SUMS");
    fs::write(&out_path, format!("{}\n", lines.join("\n"))).map_err(|e| CliError::Io {
        path: out_path.clone(),
        message: format!("failed to write SHA256SUMS: {e}"),
    })?;
    Ok(Some(out_path))
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

fn try_strip_binary<R: ProcessRunner>(runner: &R, bin_path: &Path) {
    #[cfg(unix)]
    if runner.has_tool("strip") {
        let cwd = bin_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = runner.run(&CommandSpec {
            program: "strip".to_string(),
            args: vec![bin_path.to_string_lossy().into_owned()],
            current_dir: cwd,
            env: BTreeMap::new(),
        });
    }
    #[cfg(not(unix))]
    let _ = (runner, bin_path);
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
