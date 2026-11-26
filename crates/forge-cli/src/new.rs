use std::fs;
use std::path::{Path, PathBuf};

use crate::{CliError, Result, RuntimeChoice, TemplateChoice};

const TMPL_FORGE_TOML: &str = include_str!("../../../templates/vanilla/forge.toml.tmpl");
const TMPL_CARGO_TOML: &str = include_str!("../../../templates/vanilla/Cargo.toml.tmpl");
const TMPL_GITIGNORE: &str = include_str!("../../../templates/vanilla/gitignore.tmpl");
const TMPL_MAIN_RS: &str = include_str!("../../../templates/vanilla/src/main.rs");
const TMPL_INDEX_HTML: &str = include_str!("../../../templates/vanilla/ui/index.html");
const TMPL_STYLE_CSS: &str = include_str!("../../../templates/vanilla/ui/style.css");
const TMPL_MAIN_JS: &str = include_str!("../../../templates/vanilla/ui/main.js");

#[derive(Debug, Clone)]
pub struct NewOptions {
    pub target_dir: PathBuf,
    pub template: TemplateChoice,
    pub runtime: RuntimeChoice,
    pub forge_path: Option<PathBuf>,
}

/// Scaffolds a new FORGE application directory from the requested starter template.
pub fn create_project(opts: &NewOptions) -> Result<PathBuf> {
    if opts.target_dir.exists() {
        return Err(CliError::InvalidArgument(format!(
            "destination '{}' already exists",
            opts.target_dir.display()
        )));
    }

    let raw_name = opts
        .target_dir
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            CliError::InvalidArgument(format!(
                "cannot infer project name from '{}'",
                opts.target_dir.display()
            ))
        })?;

    let crate_name = sanitize_crate_name(raw_name)?;
    let app_slug: String = crate_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let app_title = humanize_title(&crate_name);
    let forge_dep_spec = resolve_forge_dep_spec(opts.forge_path.as_deref());

    match opts.template {
        TemplateChoice::Vanilla => {
            write_vanilla_project(
                &opts.target_dir,
                &crate_name,
                &app_slug,
                &app_title,
                opts.runtime,
                &forge_dep_spec,
            )?;
        }
    }

    Ok(opts.target_dir.clone())
}

fn write_vanilla_project(
    root: &Path,
    crate_name: &str,
    app_slug: &str,
    app_title: &str,
    runtime: RuntimeChoice,
    forge_dep_spec: &str,
) -> Result<()> {
    let src_dir = root.join("src");
    let ui_dir = root.join("ui");
    create_dir(&src_dir)?;
    create_dir(&ui_dir)?;

    let apply = |tmpl: &str| -> String {
        tmpl.replace("{{APP_NAME}}", crate_name)
            .replace("{{CRATE_NAME}}", crate_name)
            .replace("{{APP_SLUG}}", app_slug)
            .replace("{{APP_TITLE}}", app_title)
            .replace("{{DEFAULT_RUNTIME}}", runtime.as_str())
            .replace("{{FORGE_DEP_SPEC}}", forge_dep_spec)
    };

    write_text(&root.join("forge.toml"), &apply(TMPL_FORGE_TOML))?;
    write_text(&root.join("Cargo.toml"), &apply(TMPL_CARGO_TOML))?;
    write_text(&root.join(".gitignore"), TMPL_GITIGNORE)?;
    write_text(&src_dir.join("main.rs"), TMPL_MAIN_RS)?;
    write_text(&ui_dir.join("index.html"), &apply(TMPL_INDEX_HTML))?;
    write_text(&ui_dir.join("style.css"), TMPL_STYLE_CSS)?;
    write_text(&ui_dir.join("main.js"), TMPL_MAIN_JS)?;

    Ok(())
}

fn resolve_forge_dep_spec(explicit_path: Option<&Path>) -> String {
    if let Some(p) = explicit_path {
        let escaped = p.to_string_lossy().replace('\\', "/");
        return format!(r#"{{ path = "{escaped}", default-features = false }}"#);
    }

    // Automatically wire to the local workspace `crates/forge` when running from source,
    // so scaffolded projects compile immediately before `forge` is published to crates.io.
    let cli_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(crates_dir) = cli_manifest_dir.parent() {
        let sibling_forge = crates_dir.join("forge");
        if sibling_forge.join("Cargo.toml").is_file() {
            let escaped = sibling_forge.to_string_lossy().replace('\\', "/");
            return format!(r#"{{ path = "{escaped}", default-features = false }}"#);
        }
    }

    r#"{ version = "0.1.0", default-features = false }"#.to_string()
}

fn sanitize_crate_name(raw: &str) -> Result<String> {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' || ch == ' ' {
            if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        } else {
            return Err(CliError::InvalidArgument(format!(
                "invalid character '{ch}' in project name '{raw}'"
            )));
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        return Err(CliError::InvalidArgument(format!(
            "project name '{raw}' must contain ASCII letters or digits"
        )));
    }
    if trimmed.as_bytes()[0].is_ascii_digit() {
        return Ok(format!("app-{trimmed}"));
    }
    Ok(trimmed)
}

fn humanize_title(crate_name: &str) -> String {
    crate_name
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        message: format!("failed to create directory: {e}"),
    })
}

fn write_text(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        message: format!("failed to write file: {e}"),
    })
}
