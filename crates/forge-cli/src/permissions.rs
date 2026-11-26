use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use forge::manifest::{ExportedCommand, Manifest};

use crate::cef_provisioner::CefProvisioner;
use crate::dev::{
    CommandSpec, ProcessRunner, inject_library_search_path, load_manifest,
    resolve_cargo_feature_args, resolve_runtime,
};
use crate::{CliError, Result, RuntimeChoice};

#[derive(Debug, Clone)]
pub struct PermissionsInitOptions {
    pub manifest_path: Option<PathBuf>,
    pub runtime_override: Option<RuntimeChoice>,
}

/// Runs the user's FORGE binary in `FORGE_EXPORT_COMMANDS` mode via `ProcessRunner` and parses
/// the exported command manifest without opening any native window.
pub fn inspect_project_commands<P, R>(
    manifest_opt: Option<&Path>,
    runtime_override: Option<RuntimeChoice>,
    provisioner: &P,
    runner: &R,
) -> Result<(PathBuf, PathBuf, Manifest, Vec<ExportedCommand>)>
where
    P: CefProvisioner,
    R: ProcessRunner,
{
    let (manifest_file, project_root, parsed) = load_manifest(manifest_opt)?;
    let active_runtime = resolve_runtime(&parsed, runtime_override);

    let cargo_toml_path = project_root.join("Cargo.toml");
    if !cargo_toml_path.is_file() {
        return Err(CliError::Manifest {
            path: cargo_toml_path,
            message: "Cargo.toml not found alongside forge.toml".into(),
        });
    }

    let staging_dir = project_root.join("target").join("forge-inspect");
    fs::create_dir_all(&staging_dir).map_err(|e| CliError::Io {
        path: staging_dir.clone(),
        message: format!("failed to create inspect directory: {e}"),
    })?;
    let export_json_path = staging_dir.join("commands.json");
    if export_json_path.exists() {
        let _ = fs::remove_file(&export_json_path);
    }

    let feature_args = resolve_cargo_feature_args(&cargo_toml_path, active_runtime)?;
    let mut args = vec!["run".to_string()];
    args.extend(feature_args);

    let mut env = BTreeMap::new();
    env.insert(
        "FORGE_MANIFEST_DIR".to_string(),
        project_root.to_string_lossy().into_owned(),
    );
    env.insert(
        "FORGE_RUNTIME".to_string(),
        active_runtime.as_str().to_string(),
    );
    env.insert(
        "FORGE_EXPORT_COMMANDS".to_string(),
        export_json_path.to_string_lossy().into_owned(),
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
        current_dir: project_root.clone(),
        env,
    };

    let code = runner.run(&spec)?;
    if code != 0 {
        return Err(CliError::ProcessFailed {
            command: format!("cargo {}", spec.args.join(" ")),
            code,
        });
    }

    let raw_json = fs::read_to_string(&export_json_path).map_err(|e| CliError::Io {
        path: export_json_path.clone(),
        message: format!("expected exported command JSON from `FORGE_EXPORT_COMMANDS`: {e}"),
    })?;
    let commands: Vec<ExportedCommand> =
        serde_json::from_str(&raw_json).map_err(|e| CliError::Manifest {
            path: export_json_path,
            message: format!("invalid exported command JSON: {e}"),
        })?;

    Ok((manifest_file, project_root, parsed, commands))
}

/// Inspects all registered commands in a FORGE project and writes or updates a strict
/// `[permissions]` allow-list inside `forge.toml`.
pub fn run_permissions_init<P, R>(
    opts: &PermissionsInitOptions,
    provisioner: &P,
    runner: &R,
) -> Result<Vec<String>>
where
    P: CefProvisioner,
    R: ProcessRunner,
{
    let (manifest_file, _project_root, _parsed, exported) = inspect_project_commands(
        opts.manifest_path.as_deref(),
        opts.runtime_override,
        provisioner,
        runner,
    )?;

    let mut allow_entries: Vec<String> = exported
        .iter()
        .filter(|c| !c.builtin)
        .map(|c| c.name.clone())
        .collect();
    allow_entries.sort();
    allow_entries.dedup();
    allow_entries.push("window.*".to_string());
    allow_entries.push("event.*".to_string());

    let raw_toml = fs::read_to_string(&manifest_file).map_err(|e| CliError::Io {
        path: manifest_file.clone(),
        message: format!("failed to read forge.toml: {e}"),
    })?;

    let rendered_section = render_permissions_toml(&allow_entries);
    let updated_toml = upsert_permissions_section(&raw_toml, &rendered_section);

    fs::write(&manifest_file, updated_toml).map_err(|e| CliError::Io {
        path: manifest_file,
        message: format!("failed to write updated forge.toml: {e}"),
    })?;

    Ok(allow_entries)
}

fn render_permissions_toml(allow_entries: &[String]) -> String {
    let quoted: Vec<String> = allow_entries.iter().map(|s| format!("\"{s}\"")).collect();
    format!(
        "[permissions]\nmode = \"enforce\"\nallow = [\n  {},\n]\n",
        quoted.join(",\n  ")
    )
}

fn upsert_permissions_section(raw_toml: &str, new_section: &str) -> String {
    let lines: Vec<&str> = raw_toml.lines().collect();
    let mut out = Vec::with_capacity(lines.len() + 8);
    let mut i = 0;
    let mut replaced = false;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed == "[permissions]" {
            out.push(new_section.trim_end());
            replaced = true;
            i += 1;
            while i < lines.len() {
                let next_trim = lines[i].trim();
                if next_trim.starts_with('[') {
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(lines[i]);
        i += 1;
    }

    let mut joined = out.join("\n");
    if !joined.ends_with('\n') {
        joined.push('\n');
    }
    if !replaced {
        if !joined.trim().is_empty() {
            joined.push('\n');
        }
        joined.push_str(new_section);
    }
    joined
}
