use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use forge_cli::build::{BuildOptions, run_build};
use forge_cli::cef_provisioner::CefProvisioner;
use forge_cli::dev::{CommandSpec, DevOptions, ProcessRunner, run_dev};
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
