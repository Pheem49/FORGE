use std::collections::BTreeMap;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use forge::manifest::Manifest;
use serde::Deserialize;

use crate::cef_provisioner::CefProvisioner;
use crate::{CliError, Result, RuntimeChoice};

/// Executable invocation prepared by CLI commands and passed across [`ProcessRunner`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
    pub env: BTreeMap<String, String>,
}

/// Seam abstracting child process execution (`cargo run` / `cargo build` / `patchelf`) for CLI testing.
pub trait ProcessRunner {
    fn run(&self, spec: &CommandSpec) -> Result<i32>;

    /// Spawns a background dev-server process (e.g., `npm run dev`).
    /// Production runners return `Ok(Some(Child))`; test runners can record the spec and return `Ok(None)`.
    fn spawn_background(&self, _spec: &CommandSpec) -> Result<Option<Child>> {
        Ok(None)
    }

    /// Checks whether an auxiliary CLI tool (`strip`, `patchelf`, `dpkg-deb`) is installed.
    fn has_tool(&self, name: &str) -> bool {
        self.probe_tool(name, &["--version"])
    }

    /// Runs a read-only probe command (`pkg-config --exists ...`, `<tool> --version`) and returns
    /// `true` when it exits with status `0`. Test runners default to `false` so no real OS
    /// processes are spawned unless explicitly simulated.
    fn probe_tool(&self, _program: &str, _args: &[&str]) -> bool {
        false
    }
}

/// Default OS child-process adapter inheriting stdio streams.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsProcessRunner;

impl ProcessRunner for OsProcessRunner {
    fn probe_tool(&self, program: &str, args: &[&str]) -> bool {
        Command::new(program)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }
    fn run(&self, spec: &CommandSpec) -> Result<i32> {
        let status = Command::new(&spec.program)
            .args(&spec.args)
            .current_dir(&spec.current_dir)
            .envs(&spec.env)
            .status()
            .map_err(|e| CliError::Io {
                path: spec.current_dir.clone(),
                message: format!("failed to spawn `{}`: {e}", spec.program),
            })?;

        Ok(status.code().unwrap_or(1))
    }

    fn spawn_background(&self, spec: &CommandSpec) -> Result<Option<Child>> {
        let child = Command::new(&spec.program)
            .args(&spec.args)
            .current_dir(&spec.current_dir)
            .envs(&spec.env)
            .spawn()
            .map_err(|e| CliError::Io {
                path: spec.current_dir.clone(),
                message: format!("failed to spawn background `{}`: {e}", spec.program),
            })?;
        Ok(Some(child))
    }
}

#[derive(Debug, Clone)]
pub struct DevOptions {
    pub manifest_path: Option<PathBuf>,
    pub runtime_override: Option<RuntimeChoice>,
    pub release: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PartialCargoToml {
    pub package: Option<PartialCargoPackage>,
    #[serde(default)]
    pub bin: Vec<PartialCargoBin>,
    pub features: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PartialCargoPackage {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PartialCargoBin {
    pub name: String,
}

struct DevServerGuard(Option<Child>);

impl Drop for DevServerGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Validates `forge.toml`, optionally launches `[dev].before_dev_command`, enables live reload,
/// provisions CEF when needed, and runs the project.
pub fn run_dev<P, R>(opts: &DevOptions, provisioner: &P, runner: &R) -> Result<()>
where
    P: CefProvisioner,
    R: ProcessRunner,
{
    let (manifest_file, project_root, parsed) = load_manifest(opts.manifest_path.as_deref())?;

    let has_dev_url = parsed
        .dev
        .url
        .as_ref()
        .is_some_and(|u| !u.trim().is_empty());

    if !has_dev_url {
        let assets_dir = if parsed.app.assets.is_absolute() {
            parsed.app.assets.clone()
        } else {
            project_root.join(&parsed.app.assets)
        };
        if !assets_dir.is_dir() {
            return Err(CliError::Manifest {
                path: manifest_file,
                message: format!(
                    "UI assets directory '{}' does not exist",
                    assets_dir.display()
                ),
            });
        }
    }

    let active_runtime = resolve_runtime(&parsed, opts.runtime_override);

    let cargo_toml_path = project_root.join("Cargo.toml");
    if !cargo_toml_path.is_file() {
        return Err(CliError::Manifest {
            path: cargo_toml_path,
            message: "Cargo.toml not found alongside forge.toml".into(),
        });
    }

    // Spawn frontend dev server (e.g. Vite) through the injected `ProcessRunner` seam.
    let _dev_server = if let Some(cmd_str) = parsed
        .dev
        .before_dev_command
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        let bg_spec = shell_command_spec(cmd_str, &project_root);
        let child_opt = runner.spawn_background(&bg_spec)?;
        if child_opt.is_some()
            && let Some(dev_url) = parsed.dev.url.as_deref()
        {
            wait_for_dev_port(dev_url, Duration::from_secs(20))?;
        }
        DevServerGuard(child_opt)
    } else {
        DevServerGuard(None)
    };

    let feature_args = resolve_cargo_feature_args(&cargo_toml_path, active_runtime)?;
    let mut args = vec!["run".to_string()];
    args.extend(feature_args);
    if opts.release {
        args.push("--release".to_string());
    }

    let mut env = BTreeMap::new();
    env.insert(
        "FORGE_MANIFEST_DIR".to_string(),
        project_root.to_string_lossy().into_owned(),
    );
    env.insert(
        "FORGE_RUNTIME".to_string(),
        active_runtime.as_str().to_string(),
    );
    env.insert("FORGE_DEV_WATCH".to_string(), "1".to_string());

    if let Some(dev_url) = parsed.dev.url.as_deref().filter(|s| !s.trim().is_empty()) {
        env.insert("FORGE_DEV_URL".to_string(), dev_url.trim().to_string());
    }

    if active_runtime == RuntimeChoice::Cef {
        let cef_dir = provisioner.ensure_cef_ready()?;
        let cef_str = cef_dir.to_string_lossy().into_owned();
        env.insert("CEF_PATH".to_string(), cef_str.clone());
        inject_library_search_path(&mut env, &cef_str);
    }

    let spec = CommandSpec {
        program: "cargo".to_string(),
        args,
        current_dir: project_root,
        env,
    };

    let code = runner.run(&spec)?;
    if code != 0 {
        return Err(CliError::ProcessFailed {
            command: format!("cargo {}", spec.args.join(" ")),
            code,
        });
    }
    Ok(())
}

pub(crate) fn load_manifest(manifest_opt: Option<&Path>) -> Result<(PathBuf, PathBuf, Manifest)> {
    let manifest_file = match manifest_opt {
        Some(p) => {
            if p.is_dir() {
                p.join("forge.toml")
            } else {
                p.to_path_buf()
            }
        }
        None => PathBuf::from("forge.toml"),
    };

    if !manifest_file.is_file() {
        return Err(CliError::Manifest {
            path: manifest_file,
            message:
                "manifest file not found (run inside a FORGE project or pass `--manifest-path`)"
                    .into(),
        });
    }

    let manifest_file = manifest_file.canonicalize().map_err(|e| CliError::Io {
        path: manifest_file,
        message: format!("failed to resolve manifest path: {e}"),
    })?;
    let project_root = manifest_file
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| CliError::Manifest {
            path: manifest_file.clone(),
            message: "manifest has no parent directory".into(),
        })?;

    let parsed = Manifest::load(&manifest_file).map_err(|e| CliError::Manifest {
        path: manifest_file.clone(),
        message: e.to_string(),
    })?;

    Ok((manifest_file, project_root, parsed))
}

pub(crate) fn resolve_runtime(
    parsed: &Manifest,
    override_choice: Option<RuntimeChoice>,
) -> RuntimeChoice {
    override_choice.unwrap_or_else(|| RuntimeChoice::from(parsed.runtime.default))
}

pub(crate) fn resolve_cargo_feature_args(
    cargo_toml_path: &Path,
    runtime: RuntimeChoice,
) -> Result<Vec<String>> {
    let parsed = parse_cargo_toml(cargo_toml_path)?;
    let rt_name = runtime.as_str();
    if parsed.features.contains_key(rt_name) {
        Ok(vec![
            "--no-default-features".to_string(),
            "--features".to_string(),
            rt_name.to_string(),
        ])
    } else {
        Ok(vec!["--features".to_string(), format!("forge/{rt_name}")])
    }
}

pub(crate) fn parse_cargo_toml(cargo_toml_path: &Path) -> Result<PartialCargoToml> {
    let content = fs::read_to_string(cargo_toml_path).map_err(|e| CliError::Io {
        path: cargo_toml_path.to_path_buf(),
        message: format!("failed to read Cargo.toml: {e}"),
    })?;
    toml::from_str(&content).map_err(|e| CliError::Manifest {
        path: cargo_toml_path.to_path_buf(),
        message: format!("invalid Cargo.toml: {e}"),
    })
}

pub(crate) fn inject_library_search_path(env: &mut BTreeMap<String, String>, cef_dir: &str) {
    #[cfg(target_os = "windows")]
    const LOADER_VAR: &str = "PATH";
    #[cfg(target_os = "windows")]
    const SEP: &str = ";";

    #[cfg(target_os = "macos")]
    const LOADER_VAR: &str = "DYLD_FALLBACK_LIBRARY_PATH";
    #[cfg(target_os = "macos")]
    const SEP: &str = ":";

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    const LOADER_VAR: &str = "LD_LIBRARY_PATH";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    const SEP: &str = ":";

    let combined = match std::env::var(LOADER_VAR) {
        Ok(existing) if !existing.is_empty() => format!("{cef_dir}{SEP}{existing}"),
        _ => cef_dir.to_string(),
    };
    env.insert(LOADER_VAR.to_string(), combined);
}

pub(crate) fn shell_command_spec(cmd_line: &str, cwd: &Path) -> CommandSpec {
    #[cfg(target_os = "windows")]
    let (program, args) = (
        "cmd".to_string(),
        vec!["/C".to_string(), cmd_line.to_string()],
    );
    #[cfg(not(target_os = "windows"))]
    let (program, args) = (
        "sh".to_string(),
        vec!["-c".to_string(), cmd_line.to_string()],
    );

    CommandSpec {
        program,
        args,
        current_dir: cwd.to_path_buf(),
        env: BTreeMap::new(),
    }
}

fn wait_for_dev_port(dev_url: &str, timeout: Duration) -> Result<()> {
    let stripped = dev_url
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host_port = stripped.split('/').next().unwrap_or("127.0.0.1:5173");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(host_port).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    Err(CliError::InvalidArgument(format!(
        "frontend dev server at '{dev_url}' did not accept connections within {}s",
        timeout.as_secs()
    )))
}
