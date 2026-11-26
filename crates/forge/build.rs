use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=FORGE_EMBED_ARCHIVE");
    println!("cargo::rerun-if-env-changed=FORGE_EMBED_MANIFEST");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set by Cargo"));
    let dest = out_dir.join("embedded_assets.rs");

    let archive_const = embed_const(
        "EMBEDDED_UI_ARCHIVE",
        "FORGE_EMBED_ARCHIVE",
        "&'static [u8]",
        "include_bytes!",
    );
    let manifest_const = embed_const(
        "EMBEDDED_MANIFEST_TOML",
        "FORGE_EMBED_MANIFEST",
        "&'static str",
        "include_str!",
    );

    fs::write(&dest, format!("{archive_const}{manifest_const}"))
        .expect("failed to write embedded_assets.rs");
}

fn embed_const(const_name: &str, env_var: &str, rust_ty: &str, include_macro: &str) -> String {
    match env::var(env_var) {
        Ok(raw_path) if !raw_path.trim().is_empty() => {
            let path = PathBuf::from(raw_path.trim());
            let canonical = path
                .canonicalize()
                .unwrap_or_else(|_| panic!("{env_var} path does not exist: {}", path.display()));
            println!("cargo::rerun-if-changed={}", canonical.display());
            let escaped = canonical.to_string_lossy().replace('\\', "/");
            format!(
                "pub(crate) const {const_name}: Option<{rust_ty}> = Some({include_macro}(\"{escaped}\"));\n"
            )
        }
        _ => format!("pub(crate) const {const_name}: Option<{rust_ty}> = None;\n"),
    }
}
