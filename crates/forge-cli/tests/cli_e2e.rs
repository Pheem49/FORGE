use std::cell::RefCell;
use std::path::PathBuf;

use forge_cli::cef_provisioner::CefProvisioner;
use forge_cli::dev::{CommandSpec, DevOptions, ProcessRunner, run_dev};
use forge_cli::new::{NewOptions, create_project};
use forge_cli::{Result, RuntimeChoice, TemplateChoice};

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
}

impl ProcessRunner for RecordingRunner {
    fn run(&self, spec: &CommandSpec) -> Result<i32> {
        self.recorded.borrow_mut().push(spec.clone());
        Ok(0)
    }
}

#[test]
fn scaffolds_vanilla_project_and_orchestrates_webview_and_cef_dev_runs() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("my_notes_app");

    // 1. Scaffold a new FORGE project via `create_project`
    let created = create_project(&NewOptions {
        target_dir: app_dir.clone(),
        template: TemplateChoice::Vanilla,
        runtime: RuntimeChoice::Webview,
        forge_path: None,
    })
    .expect("project scaffolding should succeed");

    assert_eq!(created, app_dir);
    assert!(app_dir.join("forge.toml").is_file());
    assert!(app_dir.join("Cargo.toml").is_file());
    assert!(app_dir.join("src/main.rs").is_file());
    assert!(app_dir.join("ui/index.html").is_file());
    assert!(app_dir.join("ui/style.css").is_file());
    assert!(app_dir.join("ui/main.js").is_file());

    let forge_toml = std::fs::read_to_string(app_dir.join("forge.toml")).unwrap();
    assert!(forge_toml.contains(r#"name = "my-notes-app""#));
    assert!(forge_toml.contains(r#"title = "My Notes App""#));
    assert!(forge_toml.contains(r#"default = "webview""#));

    // Rejects overwriting an existing directory
    assert!(
        create_project(&NewOptions {
            target_dir: app_dir.clone(),
            template: TemplateChoice::Vanilla,
            runtime: RuntimeChoice::Webview,
            forge_path: None,
        })
        .is_err()
    );

    // 2. Run `forge dev` in default WebView mode (must NOT invoke CefProvisioner)
    let stub_cef_dir = temp.path().join("fake-cef-sdk");
    std::fs::create_dir_all(&stub_cef_dir).unwrap();
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

    assert_eq!(*provisioner.calls.borrow(), 0);
    {
        let calls = runner.recorded.borrow();
        assert_eq!(calls.len(), 1);
        let spec = &calls[0];
        assert_eq!(spec.program, "cargo");
        assert_eq!(
            spec.args,
            vec!["run", "--no-default-features", "--features", "webview"]
        );
        assert_eq!(spec.env.get("FORGE_RUNTIME").unwrap(), "webview");
        assert!(!spec.env.contains_key("CEF_PATH"));
    }

    // 3. Switch the exact same project to `--runtime cef` (must provision CEF & set loader vars)
    run_dev(
        &DevOptions {
            manifest_path: Some(app_dir.join("forge.toml")),
            runtime_override: Some(RuntimeChoice::Cef),
            release: true,
        },
        &provisioner,
        &runner,
    )
    .expect("cef dev run should succeed");

    assert_eq!(*provisioner.calls.borrow(), 1);
    {
        let calls = runner.recorded.borrow();
        assert_eq!(calls.len(), 2);
        let spec = &calls[1];
        assert_eq!(
            spec.args,
            vec![
                "run",
                "--no-default-features",
                "--features",
                "cef",
                "--release"
            ]
        );
        assert_eq!(spec.env.get("FORGE_RUNTIME").unwrap(), "cef");
        assert_eq!(
            spec.env.get("CEF_PATH").unwrap(),
            &stub_cef_dir.to_string_lossy().into_owned()
        );
        #[cfg(target_os = "linux")]
        assert!(
            spec.env
                .get("LD_LIBRARY_PATH")
                .unwrap()
                .starts_with(stub_cef_dir.to_string_lossy().as_ref())
        );
    }
}
