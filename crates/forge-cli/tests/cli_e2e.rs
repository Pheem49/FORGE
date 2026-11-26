use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use forge_cli::bindings::{BindingsOptions, run_bindings};
use forge_cli::build::{BuildOptions, run_build};
use forge_cli::bundle::icons::IconSet;
use forge_cli::bundle::macos::{app::MacAppTarget, dmg::DmgTarget};
use forge_cli::bundle::staged::StagedApp;
use forge_cli::bundle::target::BundleTarget;
use forge_cli::bundle::windows::nsis::NsisTarget;
use forge_cli::cef_provisioner::CefProvisioner;
use forge_cli::dev::{CommandSpec, DevOptions, ProcessRunner, run_dev};
use forge_cli::doctor::{DoctorOptions, run_doctor};
use forge_cli::new::{NewOptions, create_project};
use forge_cli::permissions::{PermissionsInitOptions, run_permissions_init};
use forge_cli::{BundleFormat, Result, RuntimeChoice, TemplateChoice};

struct StubCefProvisioner {
    cef_dir: PathBuf,
    calls: RefCell<usize>,
}

impl CefProvisioner for StubCefProvisioner {
    fn ensure_cef_ready(&self) -> Result<PathBuf> {
        *self.calls.borrow_mut() += 1;
        Ok(self.cef_dir.clone())
    }
}

#[derive(Default)]
struct RecordingRunner {
    recorded: RefCell<Vec<CommandSpec>>,
    fake_release_bin_name: Option<String>,
    fake_target_dir: Option<PathBuf>,
}

impl ProcessRunner for RecordingRunner {
    fn run(&self, spec: &CommandSpec) -> Result<i32> {
        self.recorded.borrow_mut().push(spec.clone());
        // Run real `dpkg-deb` so the produced `.deb` can be inspected (hermetic: only touches temp dirs).
        if spec.program == "dpkg-deb" {
            let status = Command::new("dpkg-deb").args(&spec.args).output().unwrap();
            return Ok(status.status.code().unwrap_or(1));
        }
        if let Some(export_path) = spec.env.get("FORGE_EXPORT_COMMANDS") {
            let p = PathBuf::from(export_path);
            if let Some(parent) = p.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(
                p,
                r#"[{"name":"greet","builtin":false},{"name":"fs.read_text","builtin":false},{"name":"window.close","builtin":true}]"#,
            );
        }
        if spec.program == "cargo"
            && spec.args.first().map(String::as_str) == Some("build")
            && let (Some(bin_name), Some(target_dir)) =
                (&self.fake_release_bin_name, &self.fake_target_dir)
        {
            let rel_dir = target_dir.join("release");
            let _ = fs::create_dir_all(&rel_dir);
            let _ = fs::write(
                rel_dir.join(format!("{bin_name}{}", std::env::consts::EXE_SUFFIX)),
                b"#!/bin/sh\necho hello-from-forge\n",
            );
        }
        Ok(0)
    }

    fn probe_tool(&self, program: &str, _args: &[&str]) -> bool {
        if program == "dpkg-deb" {
            Command::new("dpkg-deb")
                .arg("--version")
                .output()
                .is_ok_and(|o| o.status.success())
        } else {
            matches!(program, "patchelf" | "strip")
        }
    }
}

#[test]
fn scaffolds_vanilla_and_react_projects_and_orchestrates_dev_runs() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("my_notes_app");

    // 1. Scaffold Vanilla project
    let created = create_project(&NewOptions {
        target_dir: app_dir.clone(),
        template: TemplateChoice::Vanilla,
        runtime: RuntimeChoice::Webview,
        forge_path: None,
    })
    .expect("vanilla scaffolding should succeed");

    assert_eq!(created, app_dir);
    assert!(app_dir.join("forge.toml").is_file());
    assert!(app_dir.join("Cargo.toml").is_file());
    assert!(app_dir.join("src/main.rs").is_file());
    assert!(app_dir.join("ui/index.html").is_file());

    let stub_cef_dir = temp.path().join("fake-cef-sdk");
    fs::create_dir_all(&stub_cef_dir).unwrap();
    let provisioner = StubCefProvisioner {
        cef_dir: stub_cef_dir.clone(),
        calls: RefCell::new(0),
    };
    let runner = RecordingRunner::default();

    run_dev(
        &DevOptions {
            manifest_path: Some(app_dir.join("forge.toml")),
            runtime_override: None,
            release: false,
        },
        &provisioner,
        &runner,
    )
    .expect("webview dev run should succeed");

    {
        let calls = runner.recorded.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].env.get("FORGE_DEV_WATCH").unwrap(), "1");
        assert_eq!(calls[0].env.get("FORGE_RUNTIME").unwrap(), "webview");
    }

    // 2. Scaffold React + Vite + TypeScript project
    let react_dir = temp.path().join("react_studio");
    create_project(&NewOptions {
        target_dir: react_dir.clone(),
        template: TemplateChoice::React,
        runtime: RuntimeChoice::Cef,
        forge_path: None,
    })
    .expect("react scaffolding should succeed");

    assert!(react_dir.join("package.json").is_file());
    assert!(react_dir.join("vite.config.ts").is_file());
    assert!(react_dir.join("tsconfig.json").is_file());
    assert!(react_dir.join("src-ui/App.tsx").is_file());
    assert!(react_dir.join("src-ui/forge.d.ts").is_file());

    run_dev(
        &DevOptions {
            manifest_path: Some(react_dir.join("forge.toml")),
            runtime_override: None,
            release: false,
        },
        &provisioner,
        &runner,
    )
    .expect("react cef dev run should succeed without pre-built dist-ui");

    {
        let calls = runner.recorded.borrow();
        let spec = &calls[1];
        assert_eq!(spec.env.get("FORGE_RUNTIME").unwrap(), "cef");
        assert_eq!(
            spec.env.get("FORGE_DEV_URL").unwrap(),
            "http://127.0.0.1:5173"
        );
        assert!(spec.env.contains_key("CEF_PATH"));
    }
}

#[test]
fn builds_embedded_release_bundles_tarball_and_deb_packages() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("packager_demo");

    create_project(&NewOptions {
        target_dir: app_dir.clone(),
        template: TemplateChoice::Vanilla,
        runtime: RuntimeChoice::Webview,
        forge_path: None,
    })
    .unwrap();

    let stub_cef_dir = temp.path().join("cef-runtime");
    fs::create_dir_all(stub_cef_dir.join("locales")).unwrap();
    fs::write(stub_cef_dir.join("libcef.so"), b"fake-libcef").unwrap();
    fs::write(stub_cef_dir.join("icudtl.dat"), b"fake-icu").unwrap();
    fs::write(stub_cef_dir.join("locales/en-US.pak"), b"fake-pak").unwrap();

    let provisioner = StubCefProvisioner {
        cef_dir: stub_cef_dir,
        calls: RefCell::new(0),
    };
    let runner = RecordingRunner {
        recorded: RefCell::new(Vec::new()),
        fake_release_bin_name: Some("packager-demo".to_string()),
        fake_target_dir: Some(app_dir.join("target")),
    };

    // Build CEF bundle (`all` = portable dir + tarball + .deb when dpkg-deb exists)
    let artifacts = run_build(
        &BuildOptions {
            manifest_path: Some(app_dir.join("forge.toml")),
            runtime_override: Some(RuntimeChoice::Cef),
            format: BundleFormat::All,
            out_dir: None,
        },
        &provisioner,
        &runner,
    )
    .expect("forge build should succeed");

    // Verify `FORGE_EMBED_ARCHIVE`, `FORGE_EMBED_MANIFEST`, and `patchelf --set-rpath $ORIGIN`
    {
        let calls = runner.recorded.borrow();
        let cargo_call = calls
            .iter()
            .find(|c| c.program == "cargo")
            .expect("cargo build should be invoked");
        let embed_path = PathBuf::from(cargo_call.env.get("FORGE_EMBED_ARCHIVE").unwrap());
        assert!(embed_path.is_file());
        assert!(cargo_call.env.contains_key("FORGE_EMBED_MANIFEST"));

        #[cfg(target_os = "linux")]
        {
            let patchelf_call = calls
                .iter()
                .find(|c| c.program == "patchelf")
                .expect("patchelf should be invoked for Linux CEF builds");
            assert_eq!(patchelf_call.args[0], "--set-rpath");
            assert_eq!(patchelf_call.args[1], "$ORIGIN");
        }
    }

    // Verify CEF runtime files were staged alongside the executable
    assert!(artifacts.binary_path.is_file());
    assert!(artifacts.app_dir.join("libcef.so").is_file());
    assert!(artifacts.app_dir.join("icudtl.dat").is_file());
    assert!(artifacts.app_dir.join("locales/en-US.pak").is_file());
    assert!(artifacts.tarball_path.as_ref().unwrap().is_file());

    // If `dpkg-deb` is installed on this Linux host, inspect the generated `.deb`!
    if let Some(deb_path) = &artifacts.deb_path {
        assert!(deb_path.is_file());
        let info = Command::new("dpkg-deb")
            .args(["--info", &deb_path.to_string_lossy()])
            .output()
            .unwrap();
        assert!(info.status.success());
        let stdout = String::from_utf8_lossy(&info.stdout);
        assert!(stdout.contains("Package: packager-demo"));
    }
}

#[test]
fn permissions_init_exports_commands_and_rewrites_forge_toml_allow_list() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("perm_demo");

    create_project(&NewOptions {
        target_dir: app_dir.clone(),
        template: TemplateChoice::Vanilla,
        runtime: RuntimeChoice::Webview,
        forge_path: None,
    })
    .unwrap();

    // Simulate a legacy v0.2 forge.toml missing `[permissions]`
    fs::write(
        app_dir.join("forge.toml"),
        "[app]\nname = \"perm-demo\"\nassets = \"ui\"\n",
    )
    .unwrap();

    let provisioner = StubCefProvisioner {
        cef_dir: temp.path().to_path_buf(),
        calls: RefCell::new(0),
    };
    let runner = RecordingRunner::default();

    let allowed = run_permissions_init(
        &PermissionsInitOptions {
            manifest_path: Some(app_dir.join("forge.toml")),
            runtime_override: None,
        },
        &provisioner,
        &runner,
    )
    .expect("forge permissions init should succeed");

    assert_eq!(
        allowed,
        vec![
            "fs.read_text".to_string(),
            "greet".to_string(),
            "window.*".to_string(),
            "event.*".to_string()
        ]
    );

    let updated = forge::manifest::Manifest::load(&app_dir.join("forge.toml")).unwrap();
    assert_eq!(updated.permissions.allow, allowed);
}

/// Runner that pretends every external packaging tool is installed and records argv.
#[derive(Default)]
struct FakeToolsRunner {
    recorded: RefCell<Vec<CommandSpec>>,
    target_dir: Option<PathBuf>,
}

impl ProcessRunner for FakeToolsRunner {
    fn run(&self, spec: &CommandSpec) -> Result<i32> {
        self.recorded.borrow_mut().push(spec.clone());
        if spec.program == "cargo"
            && let Some(t) = &self.target_dir
        {
            let rel = t.join("release");
            let _ = fs::create_dir_all(&rel);
            let _ = fs::write(
                rel.join(format!("fake-app{}", std::env::consts::EXE_SUFFIX)),
                b"bin",
            );
        }
        // Simulate tool outputs (last arg is the output file for appimagetool / dpkg-deb).
        if (spec.program == "appimagetool" || spec.program == "dpkg-deb")
            && let Some(out) = spec.args.last()
        {
            let _ = fs::write(out, b"fake-artifact");
        }
        Ok(0)
    }

    fn has_tool(&self, name: &str) -> bool {
        matches!(name, "dpkg-deb" | "appimagetool" | "makensis" | "hdiutil")
    }
}

#[test]
fn bundler_targets_invoke_exact_tools_and_write_checksums_and_icons() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("fake-app");
    create_project(&NewOptions {
        target_dir: app_dir.clone(),
        template: TemplateChoice::Vanilla,
        runtime: RuntimeChoice::Webview,
        forge_path: None,
    })
    .unwrap();

    let provisioner = StubCefProvisioner {
        cef_dir: temp.path().to_path_buf(),
        calls: RefCell::new(0),
    };
    let runner = FakeToolsRunner {
        recorded: RefCell::default(),
        target_dir: Some(app_dir.join("target")),
    };

    let mk = |format| BuildOptions {
        manifest_path: Some(app_dir.join("forge.toml")),
        runtime_override: None,
        format,
        out_dir: None,
    };

    #[cfg(target_os = "linux")]
    {
        let art = run_build(&mk(BundleFormat::Appimage), &provisioner, &runner).unwrap();
        let ai = art.appimage_path.expect("appimage path");
        assert!(ai.to_string_lossy().ends_with(".AppImage"));
        let calls = runner.recorded.borrow();
        let call = calls.iter().find(|c| c.program == "appimagetool").unwrap();
        assert!(call.args[0].ends_with(".AppDir"));
        assert!(
            app_dir
                .join("target/forge-build/fake-app.AppDir/AppRun")
                .is_file()
        );
        assert!(
            app_dir
                .join("target/forge-build/fake-app.AppDir/usr/bin/fake-app")
                .is_file()
        );
    }

    // Icon pipeline: png set, ico, icns
    let art = run_build(&mk(BundleFormat::Zip), &provisioner, &runner).unwrap();
    assert!(art.icons.png_512.is_file());
    assert_eq!(&fs::read(&art.icons.icns_path).unwrap()[..4], b"icns");
    assert_eq!(&fs::read(&art.icons.ico_path).unwrap()[..4], &[0, 0, 1, 0]);

    // Zip + SHA256SUMS
    let zip_path = art.zip_path.expect("zip path");
    assert!(zip_path.is_file());
    let sums = fs::read_to_string(art.checksums_path.expect("checksums")).unwrap();
    assert!(sums.contains(zip_path.file_name().unwrap().to_str().unwrap()));
    assert_eq!(sums.split_whitespace().next().unwrap().len(), 64);

    // Host-incompatible format fails fast naming the host OS.
    #[cfg(not(target_os = "windows"))]
    {
        let err = run_build(&mk(BundleFormat::Nsis), &provisioner, &runner).unwrap_err();
        assert!(err.to_string().contains(std::env::consts::OS));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let err = run_build(&mk(BundleFormat::Dmg), &provisioner, &runner).unwrap_err();
        assert!(err.to_string().contains("not supported on host"));
    }

    // Directly exercise Windows NSIS and macOS .app / .dmg BundleTargets against FakeToolsRunner
    let staging = app_dir.join("target/forge-build");
    let icons = IconSet::generate(&app_dir, None, &staging).unwrap();
    let manifest = forge::manifest::Manifest::load(&app_dir.join("forge.toml")).unwrap();
    let staged_app = StagedApp {
        project_root: app_dir.clone(),
        staging_dir: staging.clone(),
        app_dir: app_dir.join("dist/fake-app"),
        binary_path: app_dir.join("dist/fake-app/fake-app"),
        bin_name: "fake-app".to_string(),
        manifest,
        runtime: RuntimeChoice::Webview,
        cef_dir: None,
        icons,
    };
    let out_cross = temp.path().join("cross-dist");
    fs::create_dir_all(&out_cross).unwrap();

    let nsis_out = NsisTarget
        .package(&staged_app, &runner, &out_cross)
        .unwrap();
    assert!(nsis_out.to_string_lossy().ends_with("-setup.exe"));
    let nsi_script = fs::read_to_string(staging.join("fake-app-installer.nsi")).unwrap();
    assert!(nsi_script.contains("WebView2"));
    assert!(nsi_script.contains("CreateShortcut"));

    let mac_app_out = MacAppTarget
        .package(&staged_app, &runner, &out_cross)
        .unwrap();
    let plist = fs::read_to_string(mac_app_out.join("Contents/Info.plist")).unwrap();
    assert!(plist.contains("CFBundleIdentifier"));
    assert!(mac_app_out.join("Contents/Resources/icon.icns").is_file());

    let dmg_out = DmgTarget.package(&staged_app, &runner, &out_cross).unwrap();
    assert!(dmg_out.to_string_lossy().ends_with(".dmg"));
    assert!(
        runner
            .recorded
            .borrow()
            .iter()
            .any(|c| c.program == "hdiutil" && c.args.contains(&"UDZO".to_string()))
    );
}

#[test]
fn scaffolds_vue_and_svelte_templates_and_generates_ts_bindings_and_doctor_reports() {
    let temp = tempfile::tempdir().unwrap();

    // 1. Vue 3 + Vite + TS template
    let vue_dir = temp.path().join("vue_notes");
    create_project(&NewOptions {
        target_dir: vue_dir.clone(),
        template: TemplateChoice::Vue,
        runtime: RuntimeChoice::Webview,
        forge_path: None,
    })
    .expect("vue scaffolding should succeed");
    assert!(vue_dir.join("src-ui/App.vue").is_file());
    assert!(vue_dir.join("src-ui/env.d.ts").is_file());
    let vue_manifest = forge::manifest::Manifest::load(&vue_dir.join("forge.toml")).unwrap();
    assert!(
        vue_manifest
            .permissions
            .allow
            .contains(&"greet".to_string())
    );
    assert_eq!(vue_manifest.bundle.publisher.as_deref(), Some("Vue Notes"));

    // 2. Svelte 5 + Vite + TS template
    let svelte_dir = temp.path().join("svelte_desk");
    create_project(&NewOptions {
        target_dir: svelte_dir.clone(),
        template: TemplateChoice::Svelte,
        runtime: RuntimeChoice::Cef,
        forge_path: None,
    })
    .expect("svelte scaffolding should succeed");
    assert!(svelte_dir.join("src-ui/App.svelte").is_file());
    let svelte_manifest = forge::manifest::Manifest::load(&svelte_dir.join("forge.toml")).unwrap();
    assert_eq!(svelte_manifest.runtime.default.as_str(), "cef");

    // 3. `forge bindings` generates `.d.ts` with union of registered command names
    let provisioner = StubCefProvisioner {
        cef_dir: temp.path().to_path_buf(),
        calls: RefCell::new(0),
    };
    let runner = RecordingRunner::default();
    let dts_path = run_bindings(
        &BindingsOptions {
            manifest_path: Some(vue_dir.join("forge.toml")),
            runtime_override: None,
            out_path: None,
        },
        &provisioner,
        &runner,
    )
    .expect("forge bindings should succeed");
    assert_eq!(dts_path, vue_dir.join("src-ui/forge-commands.d.ts"));
    let dts = fs::read_to_string(&dts_path).unwrap();
    assert!(dts.contains(r#"| "greet""#));
    assert!(dts.contains(r#"| "fs.read_text""#));
    assert!(dts.contains(r#"| "window.close""#));
    assert!(dts.contains("export interface ForgeCommandTypes {}"));

    // 4. `forge doctor` reports exact ✔ / ✖ status with a fake missing-tools runner
    struct MissingToolsRunner;
    impl ProcessRunner for MissingToolsRunner {
        fn run(&self, _spec: &CommandSpec) -> Result<i32> {
            Ok(0)
        }
        fn probe_tool(&self, program: &str, _args: &[&str]) -> bool {
            program == "rustc" // cargo, cmake, webkit2gtk, node all missing!
        }
    }

    let failing_report = run_doctor(
        &DoctorOptions {
            manifest_path: Some(vue_dir.join("forge.toml")),
            runtime_filter: Some(RuntimeChoice::Cef),
            cef_cache_dir_override: Some(temp.path().join("empty-cef")),
        },
        &MissingToolsRunner,
    );
    assert!(!failing_report.is_healthy());
    let rendered = failing_report.render();
    assert!(rendered.contains("✔ Rust compiler"));
    assert!(rendered.contains("✖ Cargo package manager"));
    assert!(rendered.contains("✖ CMake (CEF build)"));
    assert!(rendered.contains("✖ Node.js runtime"));
}
