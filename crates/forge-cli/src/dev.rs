use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::cef_provisioner::CefProvisioner;
use crate::{CliError, Result, RuntimeChoice};

/// Executable invocation prepared by [`run_dev`] and passed across the [`ProcessRunner`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
    pub env: BTreeMap<String, String>,
}

/// Seam abstracting child process execution (`cargo run`) for deterministic CLI testing.
pub trait ProcessRunner {
    fn run(&self, spec: &CommandSpec) -> Result<i32>;
}

/// Default OS child-process adapter inheriting stdio streams.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsProcessRunner;

impl ProcessRunner for OsProcessRunner {
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
}

#[derive(Debug, Clone)]
pub struct DevOptions {
    pub manifest_path: Option<PathBuf>,
    pub runtime_override: Option<RuntimeChoice>,
    pub release: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialForgeManifest {
    app: PartialAppSection,
    runtime: PartialRuntimeSection,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct PartialAppSection {
    #[allow(dead_code)]
    name: String,
    assets: PathBuf,
}

impl Default for PartialAppSection {
    fn default() -> Self {
        Self {
            name: "forge-app".to_string(),
            assets: PathBuf::from("ui"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct PartialRuntimeSection {
    default: String,
}

impl Default for PartialRuntimeSection {
    fn default() -> Self {
        Self {
            default: "webview".to_string(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PartialCargoToml {
    features: BTreeMap<String, toml::Value>,
}

/// Validates `forge.toml` and UI assets, provisions CEF when needed, and runs the project.
pub fn run_dev<P, R>(opts: &DevOptions, provisioner: &P, runner: &R) -> Result<()>
where
    P: CefProvisioner,
    R: ProcessRunner,
{
    let manifest_file = match &opts.manifest_path {
        Some(p) => {
            if p.is_dir() {
                p.join("forge.toml")
            } else {
                p.clone()
            }
        }
        None => PathBuf::from("forge.toml"),
    };

    if !manifest_file.is_file() {
        return Err(CliError::Manifest {
            path: manifest_file,
            message: "manifest file not found (run `forge dev` inside a FORGE project or pass `--manifest-path`)".into(),
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

    let raw_manifest = fs::read_to_string(&manifest_file).map_err(|e| CliError::Io {
        path: manifest_file.clone(),
        message: format!("failed to read file: {e}"),
    })?;
    let parsed: PartialForgeManifest =
        toml::from_str(&raw_manifest).map_err(|e| CliError::Manifest {
            path: manifest_file.clone(),
            message: e.to_string(),
        })?;

    let assets_dir = if parsed.app.assets.is_absolute() {
        parsed.app.assets
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

    let active_runtime = match opts.runtime_override {
        Some(rt) => rt,
        None => RuntimeChoice::parse_manifest_str(&parsed.runtime.default).ok_or_else(|| {
            CliError::Manifest {
                path: manifest_file.clone(),
                message: format!(
                    "unsupported [runtime].default '{}' (expected 'webview' or 'cef')",
                    parsed.runtime.default
                ),
            }
        })?,
    };

    let cargo_toml_path = project_root.join("Cargo.toml");
    if !cargo_toml_path.is_file() {
        return Err(CliError::Manifest {
            path: cargo_toml_path,
            message: "Cargo.toml not found alongside forge.toml".into(),
        });
    }

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

fn resolve_cargo_feature_args(
    cargo_toml_path: &Path,
    runtime: RuntimeChoice,
) -> Result<Vec<String>> {
    let content = fs::read_to_string(cargo_toml_path).map_err(|e| CliError::Io {
        path: cargo_toml_path.to_path_buf(),
        message: format!("failed to read Cargo.toml: {e}"),
    })?;
    let parsed: PartialCargoToml = toml::from_str(&content).map_err(|e| CliError::Manifest {
        path: cargo_toml_path.to_path_buf(),
        message: format!("invalid Cargo.toml: {e}"),
    })?;

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

fn inject_library_search_path(env: &mut BTreeMap<String, String>, cef_dir: &str) {
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
