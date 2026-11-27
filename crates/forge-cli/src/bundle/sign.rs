use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use forge::manifest::{UpdateReleaseManifest, current_updater_platform_key};

use crate::bundle::staged::StagedApp;
use crate::dev::{CommandSpec, ProcessRunner};
use crate::signer::{parse_signing_key, sign_artifact_with_key};
use crate::{CliError, Result, RuntimeChoice};

const DEFAULT_TIMESTAMP_URL: &str = "http://timestamp.digicert.com";

/// Optional signing, notarization, and release-manifest parameters accepted at the `run_build_signed` seam.
#[derive(Debug, Clone, Default)]
pub struct SigningOptions {
    /// Base64 Ed25519 private key or file path override for emitting `dist/latest.json`.
    pub updater_private_key: Option<String>,
    pub updater_key_path: Option<PathBuf>,
    /// Base download URL prefix (e.g. `https://github.com/org/app/releases/download/v0.4.0`).
    pub updater_base_url: Option<String>,
    /// Optional release notes embedded into `dist/latest.json`.
    pub release_notes: Option<String>,
    /// Optional ISO-8601 publication timestamp embedded into `dist/latest.json`.
    pub pub_date: Option<String>,
    /// Optional override for macOS Developer ID identity (defaults to `[bundle.macos].signing_identity` or `APPLE_SIGNING_IDENTITY`).
    pub macos_signing_identity: Option<String>,
    /// Optional override for `xcrun notarytool` keychain profile (`[bundle.macos].notary_profile` or `APPLE_NOTARY_PROFILE`).
    pub macos_notary_profile: Option<String>,
    /// Optional override for Windows certificate SHA-1 thumbprint (`[bundle.windows].certificate_thumbprint` or `WINDOWS_CERTIFICATE_THUMBPRINT`).
    pub windows_certificate_thumbprint: Option<String>,
}

impl SigningOptions {
    pub fn from_env() -> Self {
        Self {
            updater_private_key: env_non_empty("FORGE_UPDATER_PRIVATE_KEY"),
            updater_key_path: env_non_empty("FORGE_UPDATER_KEY_PATH").map(PathBuf::from),
            updater_base_url: env_non_empty("FORGE_UPDATER_BASE_URL"),
            release_notes: env_non_empty("FORGE_RELEASE_NOTES"),
            pub_date: env_non_empty("FORGE_RELEASE_PUB_DATE"),
            macos_signing_identity: env_non_empty("APPLE_SIGNING_IDENTITY"),
            macos_notary_profile: env_non_empty("APPLE_NOTARY_PROFILE"),
            windows_certificate_thumbprint: env_non_empty("WINDOWS_CERTIFICATE_THUMBPRINT"),
        }
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Signs a Windows PE executable (`.exe` or NSIS `*-setup.exe`) with `signtool` when a thumbprint is configured.
pub fn sign_windows_executable(
    runner: &dyn ProcessRunner,
    app: &StagedApp,
    signing: &SigningOptions,
    exe_path: &Path,
) -> Result<bool> {
    let thumbprint = signing
        .windows_certificate_thumbprint
        .as_deref()
        .or(app
            .manifest
            .bundle
            .windows
            .certificate_thumbprint
            .as_deref())
        .filter(|s| !s.trim().is_empty());

    let Some(thumb) = thumbprint else {
        return Ok(false);
    };

    let timestamp_url = app
        .manifest
        .bundle
        .windows
        .timestamp_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_TIMESTAMP_URL);

    let code = runner.run(&CommandSpec {
        program: "signtool".to_string(),
        args: vec![
            "sign".to_string(),
            "/sha1".to_string(),
            thumb.to_string(),
            "/fd".to_string(),
            "SHA256".to_string(),
            "/tr".to_string(),
            timestamp_url.to_string(),
            "/td".to_string(),
            "SHA256".to_string(),
            exe_path.to_string_lossy().into_owned(),
        ],
        current_dir: app.project_root.clone(),
        env: BTreeMap::new(),
    })?;
    if code != 0 {
        return Err(CliError::ProcessFailed {
            command: "signtool sign".to_string(),
            code,
        });
    }
    Ok(true)
}

/// Signs a macOS `.app` bundle inside-out (`dylib` -> CEF helpers -> CEF framework -> main binary -> outer `.app`)
/// using `codesign --force --options runtime --timestamp`.
pub fn sign_macos_app_bundle(
    runner: &dyn ProcessRunner,
    app: &StagedApp,
    signing: &SigningOptions,
    app_bundle: &Path,
) -> Result<bool> {
    let identity = signing
        .macos_signing_identity
        .as_deref()
        .or(app.manifest.bundle.macos.signing_identity.as_deref())
        .filter(|s| !s.trim().is_empty());

    let Some(sign_id) = identity else {
        return Ok(false);
    };

    let entitlements_path = app.manifest.bundle.macos.entitlements.as_ref().map(|rel| {
        if rel.is_absolute() {
            rel.clone()
        } else {
            app.project_root.join(rel)
        }
    });

    if app.runtime == RuntimeChoice::Cef {
        let frameworks_dir = app_bundle.join("Contents/Frameworks");
        let cef_fw = frameworks_dir.join("Chromium Embedded Framework.framework");
        let libs_dir = cef_fw.join("Libraries");

        // 1. Inside-out step 1: sign nested `.dylib` libraries inside CEF framework
        for dylib in sorted_matching_entries(&libs_dir, |p| {
            p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("dylib")
        }) {
            run_codesign(runner, &app.project_root, sign_id, None, false, &dylib)?;
        }

        // 2. Inside-out step 2: sign nested helper `.app` bundles under `Contents/Frameworks/`
        for helper_app in sorted_matching_entries(&frameworks_dir, |p| {
            p.is_dir() && p.extension().and_then(|e| e.to_str()) == Some("app")
        }) {
            run_codesign(
                runner,
                &app.project_root,
                sign_id,
                entitlements_path.as_deref(),
                true,
                &helper_app,
            )?;
        }

        // 3. Inside-out step 3: sign `Chromium Embedded Framework.framework`
        if cef_fw.is_dir() {
            run_codesign(runner, &app.project_root, sign_id, None, false, &cef_fw)?;
        }
    }

    // 4. Sign primary executable `Contents/MacOS/<bin_name>`
    let main_bin = app_bundle.join("Contents/MacOS").join(&app.bin_name);
    if main_bin.is_file() {
        run_codesign(
            runner,
            &app.project_root,
            sign_id,
            entitlements_path.as_deref(),
            true,
            &main_bin,
        )?;
    }

    // 5. Sign outer `.app` bundle
    run_codesign(
        runner,
        &app.project_root,
        sign_id,
        entitlements_path.as_deref(),
        true,
        app_bundle,
    )?;

    Ok(true)
}

/// Signs and notarizes a macOS `.dmg` disk image using `codesign`, `xcrun notarytool`, and `xcrun stapler`.
pub fn sign_and_notarize_macos_dmg(
    runner: &dyn ProcessRunner,
    app: &StagedApp,
    signing: &SigningOptions,
    dmg_path: &Path,
) -> Result<bool> {
    let identity = signing
        .macos_signing_identity
        .as_deref()
        .or(app.manifest.bundle.macos.signing_identity.as_deref())
        .filter(|s| !s.trim().is_empty());

    if let Some(sign_id) = identity {
        run_codesign(runner, &app.project_root, sign_id, None, false, dmg_path)?;
    }

    let notary_profile = signing
        .macos_notary_profile
        .as_deref()
        .or(app.manifest.bundle.macos.notary_profile.as_deref())
        .filter(|s| !s.trim().is_empty());

    let Some(profile) = notary_profile else {
        return Ok(false);
    };

    let submit_code = runner.run(&CommandSpec {
        program: "xcrun".to_string(),
        args: vec![
            "notarytool".to_string(),
            "submit".to_string(),
            dmg_path.to_string_lossy().into_owned(),
            "--keychain-profile".to_string(),
            profile.to_string(),
            "--wait".to_string(),
        ],
        current_dir: app.project_root.clone(),
        env: BTreeMap::new(),
    })?;
    if submit_code != 0 {
        return Err(CliError::ProcessFailed {
            command: "xcrun notarytool submit".to_string(),
            code: submit_code,
        });
    }

    let staple_code = runner.run(&CommandSpec {
        program: "xcrun".to_string(),
        args: vec![
            "stapler".to_string(),
            "staple".to_string(),
            dmg_path.to_string_lossy().into_owned(),
        ],
        current_dir: app.project_root.clone(),
        env: BTreeMap::new(),
    })?;
    if staple_code != 0 {
        return Err(CliError::ProcessFailed {
            command: "xcrun stapler staple".to_string(),
            code: staple_code,
        });
    }

    Ok(true)
}

/// Emits a signed `dist/latest.json` static release manifest when an Ed25519 updater key is supplied.
pub fn maybe_write_signed_update_manifest(
    dist_root: &Path,
    app: &StagedApp,
    signing: &SigningOptions,
    artifact_candidates: &[Option<PathBuf>],
) -> Result<Option<PathBuf>> {
    let raw_key = if let Some(inline) = &signing.updater_private_key {
        Some(inline.clone())
    } else if let Some(key_path) = &signing.updater_key_path {
        Some(fs::read_to_string(key_path).map_err(|e| CliError::Io {
            path: key_path.clone(),
            message: format!("failed to read updater signing key: {e}"),
        })?)
    } else {
        None
    };

    let Some(raw_key) = raw_key else {
        return Ok(None);
    };

    let Some(primary_artifact) = artifact_candidates.iter().flatten().find(|p| p.is_file()) else {
        return Ok(None);
    };

    let signing_key = parse_signing_key(&raw_key)?;
    let platform = current_updater_platform_key();
    let filename = primary_artifact
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("artifact.bin");

    let base_url = signing
        .updater_base_url
        .as_deref()
        .or_else(|| {
            app.manifest.updater.endpoints.first().and_then(|ep| {
                ep.trim_end_matches('/')
                    .rsplit_once('/')
                    .map(|(prefix, _)| prefix)
            })
        })
        .unwrap_or("https://localhost/releases/latest");
    let download_url = format!("{}/{filename}", base_url.trim_end_matches('/'));

    let entry = sign_artifact_with_key(
        &signing_key,
        primary_artifact,
        &app.manifest.app.version,
        &platform,
        &download_url,
    )?;

    let mut platforms = BTreeMap::new();
    platforms.insert(platform, entry);

    let manifest = UpdateReleaseManifest {
        version: app.manifest.app.version.clone(),
        notes: signing.release_notes.clone(),
        pub_date: signing.pub_date.clone(),
        platforms,
    };

    let json_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| CliError::InvalidArgument(format!("failed to serialize latest.json: {e}")))?;
    let out_path = dist_root.join("latest.json");
    fs::write(
        &out_path,
        format!("{}\n", String::from_utf8_lossy(&json_bytes)),
    )
    .map_err(|e| CliError::Io {
        path: out_path.clone(),
        message: format!("failed to write latest.json: {e}"),
    })?;

    Ok(Some(out_path))
}

fn run_codesign(
    runner: &dyn ProcessRunner,
    cwd: &Path,
    identity: &str,
    entitlements: Option<&Path>,
    hardened_runtime: bool,
    target: &Path,
) -> Result<()> {
    let mut args = vec![
        "--force".to_string(),
        "--timestamp".to_string(),
        "--sign".to_string(),
        identity.to_string(),
    ];
    if hardened_runtime {
        args.push("--options".to_string());
        args.push("runtime".to_string());
    }
    if let Some(ent) = entitlements {
        args.push("--entitlements".to_string());
        args.push(ent.to_string_lossy().into_owned());
    }
    args.push(target.to_string_lossy().into_owned());

    let code = runner.run(&CommandSpec {
        program: "codesign".to_string(),
        args,
        current_dir: cwd.to_path_buf(),
        env: BTreeMap::new(),
    })?;
    if code != 0 {
        return Err(CliError::ProcessFailed {
            command: "codesign".to_string(),
            code,
        });
    }
    Ok(())
}

fn sorted_matching_entries(dir: &Path, pred: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| pred(p))
        .collect();
    paths.sort();
    paths
}
