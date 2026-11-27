use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, VerifyingKey};
use forge::manifest::{
    Manifest, UpdateReleaseManifest, canonical_update_signing_payload, current_updater_platform_key,
};
use forge_cli::build::linux::{appimage::AppImageTarget, deb::DebTarget};
use forge_cli::build::macos::{app::MacAppTarget, dmg::DmgTarget};
use forge_cli::build::sign::{
    SigningOptions, sign_and_notarize_macos_dmg, sign_macos_app_bundle, sign_windows_executable,
};
use forge_cli::build::staged::StagedApp;
use forge_cli::build::target::BundleTarget;
use forge_cli::build::windows::nsis::NsisTarget;
use forge_cli::build::{BuildOptions, icons::IconSet, run_build_signed};
use forge_cli::cef_provisioner::CefProvisioner;
use forge_cli::dev::{CommandSpec, ProcessRunner};
use forge_cli::new::{NewOptions, create_project};
use forge_cli::signer::{KeygenOptions, run_keygen};
use forge_cli::{BundleFormat, Result, RuntimeChoice, TemplateChoice};

struct DummyCef(PathBuf);
impl CefProvisioner for DummyCef {
    fn ensure_cef_ready(&self) -> Result<PathBuf> {
        Ok(self.0.clone())
    }
}

#[derive(Default)]
struct RecordingRunner {
    calls: RefCell<Vec<CommandSpec>>,
    fake_release_bin: Option<PathBuf>,
}

impl ProcessRunner for RecordingRunner {
    fn run(&self, spec: &CommandSpec) -> Result<i32> {
        if spec.program == "cargo"
            && spec.args.first().map(String::as_str) == Some("build")
            && let Some(bin_path) = &self.fake_release_bin
        {
            if let Some(parent) = bin_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(bin_path, b"FAKE_ELF_BINARY_V04");
        }
        if (spec.program == "dpkg-deb"
            || spec.program == "appimagetool"
            || spec.program == "hdiutil")
            && let Some(out_arg) = spec.args.last()
        {
            let _ = fs::write(out_arg, b"FAKE_PACKAGED_INSTALLER");
        }
        self.calls.borrow_mut().push(spec.clone());
        Ok(0)
    }

    fn has_tool(&self, tool: &str) -> bool {
        matches!(
            tool,
            "dpkg-deb" | "appimagetool" | "makensis" | "codesign" | "xcrun" | "signtool"
        )
    }
}

fn make_staged_app(root: &Path, runtime: RuntimeChoice) -> StagedApp {
    let staging_dir = root.join("staging");
    let app_dir = root.join("app_dir");
    fs::create_dir_all(&staging_dir).unwrap();
    fs::create_dir_all(&app_dir).unwrap();

    let bin_path = app_dir.join("notes-pro");
    fs::write(&bin_path, b"BINARY").unwrap();

    let cef_dir = root.join("cef");
    let fw_libs = cef_dir.join("Chromium Embedded Framework.framework/Libraries");
    fs::create_dir_all(&fw_libs).unwrap();
    fs::write(fw_libs.join("libGLESv2.dylib"), b"DYLIB").unwrap();
    fs::write(fw_libs.join("libEGL.dylib"), b"DYLIB").unwrap();

    let icons = IconSet::generate(root, None, &staging_dir).unwrap();
    let manifest = Manifest::parse(
        r#"
[app]
name = "notes-pro"
version = "0.4.0"
identifier = "dev.forge.notespro"
schemes = ["notes-pro", "forgenotes"]

[window]
title = "Notes Pro"

[bundle]
publisher = "FORGE Team"
category = "Utility"

[bundle.macos]
signing_identity = "Developer ID Application: FORGE Labs (TEAM123)"
notary_profile = "forge-notary-profile"

[bundle.windows]
certificate_thumbprint = "0123456789ABCDEF0123456789ABCDEF01234567"
"#,
    )
    .unwrap();

    StagedApp {
        project_root: root.to_path_buf(),
        staging_dir,
        app_dir,
        binary_path: bin_path,
        bin_name: "notes-pro".to_string(),
        manifest,
        runtime,
        cef_dir: Some(cef_dir),
        icons,
    }
}

#[test]
fn keygen_and_build_signed_emit_verifiable_ed25519_latest_json() {
    let temp = tempfile::tempdir().unwrap();
    let key_path = temp.path().join("keys/updater.key");
    let kg = run_keygen(&KeygenOptions {
        out_key_path: key_path.clone(),
        force: false,
    })
    .expect("keygen should succeed");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&kg.secret_key_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "private key must have 0600 permissions on Unix"
        );
    }

    // Refuses to overwrite existing key unless `force = true`
    assert!(
        run_keygen(&KeygenOptions {
            out_key_path: key_path.clone(),
            force: false,
        })
        .is_err()
    );

    let forge_crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../forge");
    let proj_dir = temp.path().join("sample-app");
    create_project(&NewOptions {
        target_dir: proj_dir.clone(),
        template: TemplateChoice::Vanilla,
        runtime: RuntimeChoice::Webview,
        forge_path: Some(forge_crate_dir),
    })
    .unwrap();

    let runner = RecordingRunner {
        calls: RefCell::new(Vec::new()),
        fake_release_bin: Some(proj_dir.join("target/release/sample-app")),
    };
    let cef = DummyCef(temp.path().to_path_buf());

    let artifacts = run_build_signed(
        &BuildOptions {
            manifest_path: Some(proj_dir.join("forge.toml")),
            runtime_override: None,
            format: BundleFormat::Tarball,
            out_dir: None,
        },
        &SigningOptions {
            updater_key_path: Some(key_path),
            updater_base_url: Some(
                "https://github.com/Pheem49/FORGE/releases/download/v0.1.0".into(),
            ),
            release_notes: Some("Signed v0.4 pipeline test".into()),
            pub_date: Some("2026-11-27T11:00:00Z".into()),
            ..SigningOptions::default()
        },
        &cef,
        &runner,
    )
    .expect("signed tarball build should succeed");

    let latest_path = artifacts
        .latest_json_path
        .expect("latest.json must be emitted");
    let parsed: UpdateReleaseManifest =
        serde_json::from_slice(&fs::read(&latest_path).unwrap()).unwrap();
    assert_eq!(parsed.version, "0.1.0");
    assert_eq!(parsed.notes.as_deref(), Some("Signed v0.4 pipeline test"));

    let platform = current_updater_platform_key();
    let entry = parsed
        .platforms
        .get(&platform)
        .expect("host platform entry");
    assert!(
        entry
            .url
            .starts_with("https://github.com/Pheem49/FORGE/releases/download/v0.1.0/")
    );

    // Cryptographically verify the emitted `latest.json` signature using `ed25519-dalek`!
    let pub_bytes: [u8; 32] = BASE64
        .decode(kg.public_key_base64.trim())
        .unwrap()
        .try_into()
        .unwrap();
    let vk = VerifyingKey::from_bytes(&pub_bytes).unwrap();
    let sig_bytes = BASE64.decode(&entry.signature).unwrap();
    let sig = Signature::from_slice(&sig_bytes).unwrap();
    let msg =
        canonical_update_signing_payload(&parsed.version, &platform, &entry.url, &entry.sha256);
    vk.verify_strict(&msg, &sig)
        .expect("latest.json signature must verify strictly");
}

#[test]
fn signs_macos_cef_inside_out_notarizes_dmg_signs_windows_and_registers_deep_links() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("dist");
    fs::create_dir_all(&out_dir).unwrap();

    let staged = make_staged_app(temp.path(), RuntimeChoice::Cef);
    let runner = RecordingRunner::default();
    let signing = SigningOptions::default();

    // 1. Package macOS `.app` (verifying `CFBundleURLTypes` deep links) + add fake CEF helper `.app`
    let app_bundle = MacAppTarget.package(&staged, &runner, &out_dir).unwrap();
    let plist = fs::read_to_string(app_bundle.join("Contents/Info.plist")).unwrap();
    assert!(plist.contains("<key>CFBundleURLTypes</key>"));
    assert!(plist.contains("<string>notes-pro</string>"));
    assert!(plist.contains("<string>forgenotes</string>"));

    let helper_app = app_bundle.join("Contents/Frameworks/Notes Pro Helper.app");
    fs::create_dir_all(&helper_app).unwrap();

    // Sign macOS `.app` inside-out + verify `DmgTarget::package_signed` signs inner `.app` BEFORE `hdiutil create`!
    assert!(sign_macos_app_bundle(&runner, &staged, &signing, &app_bundle).unwrap());
    let dmg_out = DmgTarget
        .package_signed(&staged, &signing, &runner, &out_dir)
        .unwrap();
    assert!(sign_and_notarize_macos_dmg(&runner, &staged, &signing, &dmg_out).unwrap());

    {
        let calls = runner.calls.borrow();
        let hdiutil_idx = calls
            .iter()
            .position(|c| c.program == "hdiutil")
            .expect("hdiutil must run");
        let dmg_staging_sign_idx = calls
            .iter()
            .position(|c| {
                c.program == "codesign"
                    && c.args
                        .last()
                        .is_some_and(|a| a.contains("dmg-notes-pro") && a.ends_with(".app"))
            })
            .expect("inner .app inside dmg_staging must be signed");
        assert!(
            dmg_staging_sign_idx < hdiutil_idx,
            "inner .app must be codesigned BEFORE hdiutil packs the .dmg"
        );

        let codesign_targets: Vec<&str> = calls
            .iter()
            .take(6)
            .filter(|c| c.program == "codesign")
            .filter_map(|c| c.args.last().map(String::as_str))
            .collect();
        // Inside-out order on standalone .app: 2 dylibs -> Helper.app -> CEF.framework -> Contents/MacOS/notes-pro -> Notes Pro.app
        assert_eq!(codesign_targets.len(), 6);
        assert!(codesign_targets[0].ends_with("libEGL.dylib"));
        assert!(codesign_targets[1].ends_with("libGLESv2.dylib"));
        assert!(codesign_targets[2].ends_with("Notes Pro Helper.app"));
        assert!(codesign_targets[3].ends_with("Chromium Embedded Framework.framework"));
        assert!(codesign_targets[4].ends_with("Contents/MacOS/notes-pro"));
        assert!(codesign_targets[5].ends_with("Notes Pro.app"));

        let xcrun_calls: Vec<&CommandSpec> =
            calls.iter().filter(|c| c.program == "xcrun").collect();
        assert_eq!(xcrun_calls.len(), 2);
        assert_eq!(xcrun_calls[0].args[0], "notarytool");
        assert_eq!(xcrun_calls[1].args[0], "stapler");
    }

    // 2. Windows `signtool` + NSIS deep-link registry entries
    runner.calls.borrow_mut().clear();
    let _ = NsisTarget.package(&staged, &runner, &out_dir).unwrap();
    assert!(sign_windows_executable(&runner, &staged, &signing, &staged.binary_path).unwrap());

    let nsi_script =
        fs::read_to_string(staged.staging_dir.join("notes-pro-installer.nsi")).unwrap();
    assert!(nsi_script.contains("Software\\Classes\\notes-pro"));
    assert!(nsi_script.contains("URL Protocol"));
    assert!(nsi_script.contains("DeleteRegKey HKCU \"Software\\Classes\\forgenotes\""));

    let signtool_calls: Vec<CommandSpec> = runner
        .calls
        .borrow()
        .iter()
        .filter(|c| c.program == "signtool")
        .cloned()
        .collect();
    assert_eq!(signtool_calls.len(), 1);
    assert!(
        signtool_calls[0]
            .args
            .contains(&"0123456789ABCDEF0123456789ABCDEF01234567".to_string())
    );

    // 3. Linux `.deb` and `.AppImage` `.desktop` deep-link `MimeType=x-scheme-handler/...`
    let _ = DebTarget.package(&staged, &runner, &out_dir).unwrap();
    let deb_desktop = fs::read_to_string(
        staged
            .staging_dir
            .join("deb-notes-pro/usr/share/applications/dev.forge.notespro.desktop"),
    )
    .unwrap();
    assert!(deb_desktop.contains("Exec=/usr/bin/notes-pro %u"));
    assert!(
        deb_desktop.contains("MimeType=x-scheme-handler/notes-pro;x-scheme-handler/forgenotes;")
    );

    let _ = AppImageTarget.package(&staged, &runner, &out_dir).unwrap();
    let appimage_desktop = fs::read_to_string(
        staged
            .staging_dir
            .join("notes-pro.AppDir/dev.forge.notespro.desktop"),
    )
    .unwrap();
    assert!(appimage_desktop.contains("Exec=notes-pro %u"));
    assert!(
        appimage_desktop
            .contains("MimeType=x-scheme-handler/notes-pro;x-scheme-handler/forgenotes;")
    );
}
