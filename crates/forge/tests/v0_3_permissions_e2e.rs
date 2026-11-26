use std::convert::Infallible;

use forge::{App, Error, ErrorKind, WindowOptions, WindowPermissionOverride, codes};
use serde_json::{Value, json};

async fn greet_cmd() -> Result<&'static str, Infallible> {
    Ok("hello")
}

async fn read_note() -> Result<&'static str, Infallible> {
    Ok("note-body")
}

async fn delete_note() -> Result<&'static str, Infallible> {
    Ok("deleted")
}

#[tokio::test]
async fn enforces_deny_by_default_when_permissions_section_is_omitted() {
    let app = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Default Deny</h1>".to_vec());
        })
        .command("greet", greet_cmd)
        .headless()
        .expect("app with omitted [permissions] should boot with startup diagnostic");

    // 1. Startup diagnostic warns that registered user commands are blocked by empty policy
    assert!(
        app.permission_warnings()
            .iter()
            .any(|w| w.contains("empty allow-list") && w.contains("greet"))
    );

    // 2. Both user commands AND built-in commands are denied with 403 FORGE_PERMISSION_DENIED
    let err = app.invoke_unit::<String>("greet").await.unwrap_err();
    assert_eq!(err.code, codes::PERMISSION_DENIED);
    assert_eq!(err.kind(), ErrorKind::Forbidden);
    let details = err.details.unwrap();
    assert_eq!(details["command"], "greet");
    assert_eq!(details["window"], "main");
    assert!(details["hint"].as_str().unwrap().contains("forge.toml"));

    let win_err = app
        .invoke_unit::<Vec<String>>("window.list")
        .await
        .unwrap_err();
    assert_eq!(win_err.code, codes::PERMISSION_DENIED);

    // 3. Internal `__forge_drain_events` bypasses `[permissions]` so bootstrap always works,
    // whereas `__forge_ready` is blocked outside `FORGE_SMOKE_TEST=1` so untrusted frames cannot quit!
    let drained: Vec<Value> = app.invoke_unit("__forge_drain_events").await.unwrap();
    assert!(drained.is_empty());

    let ready_err = app.invoke_unit::<Value>("__forge_ready").await.unwrap_err();
    assert_eq!(ready_err.code, codes::PERMISSION_DENIED);
    assert!(!app.is_closed());
}

#[tokio::test]
async fn evaluates_globs_deny_precedence_and_per_window_overrides() {
    let app = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Main</h1>".to_vec());
            cfg.with_memory_asset("/settings.html", b"<h1>Settings</h1>".to_vec());
            cfg.permissions.allow = vec!["greet".into(), "notes.*".into(), "window.create".into()];
            // Deny ALWAYS wins over `notes.*`!
            cfg.permissions.deny = vec!["notes.delete".into()];
            // Per-window override REPLACES `allow` for `"settings"` window
            cfg.permissions.windows.insert(
                "settings".into(),
                WindowPermissionOverride {
                    allow: vec!["greet".into()],
                    deny: Vec::new(),
                },
            );
        })
        .command("greet", greet_cmd)
        .command("notes.read", read_note)
        .command("notes.delete", delete_note)
        .headless()
        .expect("app with valid globs should boot");

    // Open `settings` window from `main`
    let _ = app
        .app_handle()
        .create_window("settings", WindowOptions::new("/settings.html"))
        .unwrap();

    // `main` can call `greet` and `notes.read`, but `notes.delete` is blocked by `deny`
    assert_eq!(app.invoke_unit::<String>("greet").await.unwrap(), "hello");
    assert_eq!(
        app.invoke_unit::<String>("notes.read").await.unwrap(),
        "note-body"
    );
    let deny_err = app.invoke_unit::<String>("notes.delete").await.unwrap_err();
    assert_eq!(deny_err.code, codes::PERMISSION_DENIED);

    // `settings` can call `greet`, but CANNOT call `notes.read` (per-window allow replaced global allow)
    let ok_settings: String = app
        .invoke_as("settings", "greet", Value::Null)
        .await
        .unwrap();
    assert_eq!(ok_settings, "hello");

    let settings_denied = app
        .invoke_as::<_, String>("settings", "notes.read", Value::Null)
        .await
        .unwrap_err();
    assert_eq!(settings_denied.code, codes::PERMISSION_DENIED);
    assert_eq!(settings_denied.details.unwrap()["window"], "settings");

    // Unknown or already-closed secondary windows NEVER fall back or escalate to `"main"`!
    let closed_err = app
        .invoke_as::<_, String>("closed-window", "notes.read", Value::Null)
        .await
        .unwrap_err();
    assert_eq!(closed_err.code, codes::WINDOW_NOT_FOUND);
    assert_eq!(closed_err.kind(), ErrorKind::NotFound);
}

#[test]
fn startup_typo_guard_errors_in_enforce_and_warns_in_permissive() {
    // 1. In `enforce` mode, a typo matching zero commands fails fast at startup!
    let bad = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Typo</h1>".to_vec());
            cfg.permissions.allow = vec!["notes.red_text".into()];
        })
        .command("notes.read_text", read_note)
        .headless();

    match bad {
        Err(Error::Config { message, .. }) => {
            assert!(message.contains("notes.red_text"));
        }
        other => panic!(
            "expected Error::Config for permission typo, got {}",
            other.is_ok()
        ),
    }

    // 2. In `permissive` mode, a typo becomes a startup warning instead of aborting boot
    let permissive = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Permissive</h1>".to_vec());
            cfg.permissions.set_permissive();
            cfg.permissions.allow = vec!["notes.red_text".into()];
        })
        .command("notes.read_text", read_note)
        .headless()
        .expect("permissive mode should warn rather than fail");

    assert!(
        permissive
            .permission_warnings()
            .iter()
            .any(|w| w.contains("notes.red_text"))
    );

    // 3. `App::export_commands` lists both user commands and built-in commands deterministically
    let exported = App::new()
        .command("greet", greet_cmd)
        .export_commands()
        .unwrap();
    assert!(exported.iter().any(|c| c.name == "greet" && !c.builtin));
    assert!(
        exported
            .iter()
            .any(|c| c.name == "window.close" && c.builtin)
    );
    let _ = json!(exported);
}
