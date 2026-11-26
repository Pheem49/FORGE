use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=FORGE_EMBED_ARCHIVE");
    println!("cargo::rerun-if-env-changed=FORGE_EMBED_MANIFEST");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set by Cargo"));
    let dest = out_dir.join("embedded_assets.rs");

    let archive_const = match env::var("FORGE_EMBED_ARCHIVE") {
        Ok(raw_path) if !raw_path.trim().is_empty() => {
            let archive_path = PathBuf::from(raw_path.trim());
            let canonical = archive_path
                .canonicalize()
                .expect("FORGE_EMBED_ARCHIVE path does not exist");
            println!("cargo::rerun-if-changed={}", canonical.display());
            let escaped = canonical.to_string_lossy().replace('\\', "/");
            format!(
                "pub(crate) const EMBEDDED_UI_ARCHIVE: Option<&'static [u8]> = Some(include_bytes!(\"{escaped}\"));\n"
            )
        }
        _ => "pub(crate) const EMBEDDED_UI_ARCHIVE: Option<&'static [u8]> = None;\n".to_string(),
    };

    let manifest_const = match env::var("FORGE_EMBED_MANIFEST") {
        Ok(raw_path) if !raw_path.trim().is_empty() => {
            let manifest_path = PathBuf::from(raw_path.trim());
            let canonical = manifest_path
                .canonicalize()
                .expect("FORGE_EMBED_MANIFEST path does not exist");
            println!("cargo::rerun-if-changed={}", canonical.display());
            let escaped = canonical.to_string_lossy().replace('\\', "/");
            format!(
                "pub(crate) const EMBEDDED_MANIFEST_TOML: Option<&'static str> = Some(include_str!(\"{escaped}\"));\n"
            )
        }
        _ => "pub(crate) const EMBEDDED_MANIFEST_TOML: Option<&'static str> = None;\n".to_string(),
    };

    fs::write(&dest, format!("{archive_const}{manifest_const}"))
        .expect("failed to write embedded_assets.rs");
}
