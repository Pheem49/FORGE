use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge::{
    AcquireOutcome, App, AppHandle, ErrorKind, ForgeError, MonitorBounds, RuntimeKind,
    SavedWindowGeometry, SingleInstanceGuard, TrayConfig, WindowHandle, WindowOptions, codes,
};
use serde_json::{Value, json};

async fn caller_info(win: WindowHandle, app: AppHandle) -> Result<Value, Infallible> {
    Ok(json!({
        "calling_window": win.label(),
        "open_windows": app.windows(),
    }))
}

async fn open_inspector(app: AppHandle) -> Result<String, ForgeError> {
    let win = app.create_window(
        "inspector",
        WindowOptions::new("/inspector.html")
            .title("Inspector")
            .size(640, 480),
    )?;
    Ok(win.label().to_string())
}

#[tokio::test]
async fn supports_multi_window_targeted_events_and_builtin_window_commands() {
    let temp = tempfile::tempdir().unwrap();
    let ui_dir = temp.path().join("ui");
    std::fs::create_dir_all(&ui_dir).unwrap();
    std::fs::write(ui_dir.join("index.html"), "<h1>Main</h1>").unwrap();
    std::fs::write(ui_dir.join("settings.html"), "<h1>Settings</h1>").unwrap();
    std::fs::write(ui_dir.join("inspector.html"), "<h1>Inspector</h1>").unwrap();
    std::fs::write(
        temp.path().join("forge.toml"),
        r#"
[app]
name = "multi-win"
assets = "ui"
exit_on_last_window_close = true

[window]
title = "Primary"
width = 1000
height = 700

[windows.settings]
url = "/settings.html"
title = "Preferences"
width = 520
height = 420
create_on_startup = false
"#,
    )
    .unwrap();

    let app = App::from_root(temp.path())
        .command("caller.info", caller_info)
        .command("open.inspector", open_inspector)
        .headless()
        .expect("multi-window headless app should boot");

    assert_eq!(app.windows(), vec!["main".to_string()]);

    // 1. Open configured `settings` window via built-in `window.create` IPC command
    let created_label: String = app
        .invoke("window.create", json!({ "label": "settings" }))
        .await
        .unwrap();
    assert_eq!(created_label, "settings");
    assert_eq!(
        app.window_title_for("settings").as_deref(),
        Some("Preferences")
    );

    // 2. Duplicate window label fails with FORGE_DUPLICATE_WINDOW (400 BadRequest)
    let dup_err = app
        .invoke::<_, String>("window.create", json!({ "label": "settings" }))
        .await
        .unwrap_err();
    assert_eq!(dup_err.code, codes::DUPLICATE_WINDOW);
    assert_eq!(dup_err.kind(), ErrorKind::BadRequest);

    // 3. Open dynamic `inspector` window via AppHandle extractor
    let insp: String = app.invoke_unit("open.inspector").await.unwrap();
    assert_eq!(insp, "inspector");
    assert_eq!(
        app.windows(),
        vec![
            "inspector".to_string(),
            "main".to_string(),
            "settings".to_string()
        ]
    );

    // 4. Bootstrap script stamps each window's own label (`window.forge.window.label`)
    let settings_boot = app.get_asset_for("settings", "/__forge/bootstrap.js").await;
    assert!(
        settings_boot
            .text()
            .unwrap()
            .contains(r#"const WINDOW_LABEL = "settings";"#)
    );

    // 5. WindowHandle extractor injects the calling window (`invoke_as("settings", ...)`)
    let info: Value = app
        .invoke_as("settings", "caller.info", Value::Null)
        .await
        .unwrap();
    assert_eq!(info["calling_window"], "settings");

    // 6. Targeted `event.emit_to` lands ONLY on `settings`, whereas `event.emit_all` reaches all 3
    let (): () = app
        .invoke(
            "event.emit_to",
            json!({ "target": "settings", "event": "prefs.saved", "payload": { "ok": true } }),
        )
        .await
        .unwrap();
    assert_eq!(app.take_emitted_events_for("main").len(), 0);
    assert_eq!(app.take_emitted_events_for("inspector").len(), 0);
    let settings_evs = app.take_emitted_events_for("settings");
    assert_eq!(settings_evs.len(), 1);
    assert_eq!(settings_evs[0].0, "prefs.saved");

    let (): () = app
        .invoke(
            "event.emit_all",
            json!({ "event": "theme.changed", "payload": { "dark": true } }),
        )
        .await
        .unwrap();
    assert_eq!(app.take_emitted_events_for("main").len(), 1);
    assert_eq!(app.take_emitted_events_for("settings").len(), 1);
    assert_eq!(app.take_emitted_events_for("inspector").len(), 1);

    // 7. Closing `settings` and `inspector` keeps app alive; closing last window (`main`) exits!
    let (): () = app
        .invoke_as("settings", "window.close", Value::Null)
        .await
        .unwrap();
    assert!(!app.is_closed());
    let (): () = app
        .invoke_as("inspector", "window.close", Value::Null)
        .await
        .unwrap();
    assert!(!app.is_closed());
    let (): () = app
        .invoke_as("main", "window.close", Value::Null)
        .await
        .unwrap();
    assert!(app.is_closed());
}

#[tokio::test]
async fn declarative_tray_and_menu_dispatch_events_and_report_cef_unsupported() {
    let temp = tempfile::tempdir().unwrap();
    let ui_dir = temp.path().join("ui");
    std::fs::create_dir_all(&ui_dir).unwrap();
    std::fs::write(ui_dir.join("index.html"), "<h1>Tray</h1>").unwrap();
    std::fs::write(
        temp.path().join("forge.toml"),
        r#"
[app]
name = "tray-demo"
assets = "ui"

[tray]
tooltip = "Notes Tray"
menu = [
  { id = "tray.sync", label = "Sync Now" },
  { separator = true },
  { id = "forge.quit", label = "Quit" }
]

[menu]
[[menu.submenu]]
label = "File"
items = [
  { id = "file.new", label = "New Note", accelerator = "CmdOrCtrl+N" },
  { id = "forge.quit", label = "Quit", accelerator = "CmdOrCtrl+Q" }
]
"#,
    )
    .unwrap();

    let app = App::from_root(temp.path())
        .headless()
        .expect("tray/menu app should boot");

    assert_eq!(
        app.tray().and_then(|t| t.tooltip),
        Some("Notes Tray".to_string())
    );
    assert_eq!(app.menu().map(|m| m.submenu.len()), Some(1));

    // Clicking custom menu & tray items emits `menu:click` and `tray:click`
    app.click_menu("file.new");
    app.click_tray("tray.sync");
    let evs = app.take_emitted_events();
    assert!(
        evs.iter()
            .any(|(e, p)| e == "menu:click" && p["id"] == "file.new")
    );
    assert!(
        evs.iter()
            .any(|(e, p)| e == "tray:click" && p["id"] == "tray.sync")
    );
    assert!(!app.is_closed());

    // Clicking `forge.quit` terminates cleanly
    app.click_menu("forge.quit");
    assert!(app.is_closed());

    // On CEF runtime, `AppHandle::set_tray` / `set_menu` return typed FORGE_UNSUPPORTED (501)
    let cef_app = App::new()
        .configure(|cfg| {
            cfg.runtime.default = RuntimeKind::Cef;
            cfg.with_memory_asset("/index.html", b"<h1>CEF</h1>".to_vec());
        })
        .headless()
        .unwrap();
    let err = cef_app
        .app_handle()
        .set_tray(TrayConfig::default())
        .unwrap_err();
    assert_eq!(err.code, codes::UNSUPPORTED);
    assert_eq!(err.kind(), ErrorKind::Unsupported);
}

#[test]
fn single_instance_guard_blocks_duplicate_and_forwards_args_over_loopback() {
    let temp = tempfile::tempdir().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_clone = Arc::clone(&received);

    let first = SingleInstanceGuard::acquire(
        temp.path(),
        "dev.forge.notes",
        vec!["notes".into()],
        PathBuf::from("/home/user"),
        move |payload| {
            received_clone.lock().unwrap().push(payload);
        },
    )
    .expect("primary instance should acquire lock");
    let AcquireOutcome::Primary(primary_guard) = first else {
        panic!("expected primary guard");
    };

    let second = SingleInstanceGuard::acquire(
        temp.path(),
        "dev.forge.notes",
        vec!["notes".into(), "--open".into(), "todo.md".into()],
        PathBuf::from("/tmp"),
        |_| {},
    )
    .expect("secondary instance should forward cleanly");
    assert!(matches!(second, AcquireOutcome::SecondaryForwarded));

    // Wait briefly for loopback listener thread to deliver payload
    for _ in 0..40 {
        if !received.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }

    {
        let log = received.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].args, vec!["notes", "--open", "todo.md"]);
        assert_eq!(log[0].cwd, PathBuf::from("/tmp"));
    }

    // Dropping primary guard releases lock so a fresh launch becomes primary
    drop(primary_guard);
    let relaunched = SingleInstanceGuard::acquire(
        temp.path(),
        "dev.forge.notes",
        vec!["notes".into()],
        PathBuf::from("/home/user"),
        |_| {},
    )
    .expect("relaunch after drop should acquire primary lock");
    assert!(matches!(relaunched, AcquireOutcome::Primary(_)));
}

#[tokio::test]
async fn window_state_persists_across_restarts_and_clamps_offscreen_coordinates() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("state");

    // 1st launch: resize, move, maximize, then close
    {
        let sdir = state_dir.clone();
        let app = App::new()
            .configure(move |cfg| {
                cfg.window.remember_state = true;
                cfg.with_state_dir(sdir);
                cfg.with_memory_asset("/index.html", b"<h1>State</h1>".to_vec());
            })
            .headless()
            .unwrap();

        let main = app.app_handle().window("main").unwrap();
        main.set_size(1366, 840);
        main.set_position(180, 95);
        main.set_maximized(true);
        main.close_window();
        assert!(app.is_closed());
    }

    // 2nd launch: restored automatically from `window-state.json`
    {
        let sdir = state_dir.clone();
        let app2 = App::new()
            .configure(move |cfg| {
                cfg.window.remember_state = true;
                cfg.with_state_dir(sdir);
                cfg.with_memory_asset("/index.html", b"<h1>State</h1>".to_vec());
            })
            .headless()
            .unwrap();

        assert_eq!(app2.window_size(), (1366, 840));
        assert_eq!(app2.window_position(), Some((180, 95)));
        assert!(app2.is_maximized());
    }

    // Off-screen monitor clamping
    let offscreen = SavedWindowGeometry {
        x: Some(4200),
        y: Some(-900),
        width: 2400,
        height: 1600,
        maximized: false,
    };
    let clamped = offscreen.clamp_to_monitor(MonitorBounds {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    });
    assert_eq!(clamped.width, 1920);
    assert_eq!(clamped.height, 1080);
    assert_eq!(clamped.x, Some(0));
    assert_eq!(clamped.y, Some(0));
}
