use std::convert::Infallible;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use forge::http::{Method, Request, StatusCode};
use forge::{App, AppConfig, Error, ForgeError, Plugin, State, WindowHandle};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default)]
struct Counter(AtomicUsize);

#[derive(Debug, Deserialize, Serialize)]
struct AddReq {
    delta: usize,
    label: String,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
struct AddRes {
    total: usize,
    runtime: String,
}

async fn add_count(
    counter: State<Counter>,
    cfg: AppConfig,
    window: WindowHandle,
    req: AddReq,
) -> Result<AddRes, ForgeError> {
    if req.label.trim().is_empty() {
        return Err(
            ForgeError::bad_request("EMPTY_LABEL", "Label must not be blank")
                .with_details(json!({ "field": "label" })),
        );
    }
    let total = counter.0.fetch_add(req.delta, Ordering::SeqCst) + req.delta;
    window.set_title(format!("{} ({total})", req.label));
    Ok(AddRes {
        total,
        runtime: cfg.runtime.default.as_str().to_string(),
    })
}

async fn current_count(counter: State<Counter>) -> Result<usize, Infallible> {
    Ok(counter.0.load(Ordering::SeqCst))
}

async fn close_now(window: WindowHandle) -> Result<&'static str, Infallible> {
    window.close_window();
    Ok("closing")
}

async fn trigger_panic() -> Result<(), Infallible> {
    panic!("deliberate test explosion");
}

struct CounterPlugin;

impl Plugin for CounterPlugin {
    fn apply(self, app: App) -> App {
        app.setup(|ctx| {
            ctx.insert_state(Counter::default());
            Ok(())
        })
        .command("counter.add", add_count)
        .command("counter.get", current_count)
        .command("window.close", close_now)
        .command("debug.panic", trigger_panic)
    }
}

#[tokio::test]
async fn serves_assets_injects_bootstrap_and_blocks_path_traversal() {
    let temp = tempfile::tempdir().unwrap();
    let ui_dir = temp.path().join("ui");
    fs::create_dir_all(&ui_dir).unwrap();
    fs::write(
        temp.path().join("forge.toml"),
        r#"
[app]
name = "notes"
assets = "ui"

[window]
title = "Forge Notes"
width = 900
height = 600
"#,
    )
    .unwrap();
    fs::write(
        ui_dir.join("index.html"),
        "<!doctype html><html><head><title>Notes</title></head><body><h1>Ready</h1></body></html>",
    )
    .unwrap();
    fs::write(ui_dir.join("secret_outside.txt"), "safe").unwrap();
    fs::write(temp.path().join("outside.txt"), "top-secret").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(temp.path().join("outside.txt"), ui_dir.join("escaped.txt"))
        .unwrap();

    let app = App::from_root(temp.path())
        .plugin(CounterPlugin)
        .headless()
        .expect("headless app should boot");

    assert_eq!(app.config().window.title, "Forge Notes");
    assert_eq!(app.window_title(), "Forge Notes");

    // 1. Root "/" resolves to index.html with injected bootstrap script & CSP
    let root_res = app.get_asset("/").await;
    assert_eq!(root_res.status, StatusCode::OK);
    assert_eq!(root_res.headers["content-type"], "text/html; charset=utf-8");
    assert!(root_res.headers.contains_key("content-security-policy"));
    let html = root_res.text().unwrap();
    assert!(html.contains(r#"<script src="/__forge/bootstrap.js"></script>"#));
    assert!(html.contains("<h1>Ready</h1>"));

    // 2. Bootstrap script serves frozen window.forge runtime bridge
    let boot_res = app.get_asset("/__forge/bootstrap.js").await;
    assert_eq!(boot_res.status, StatusCode::OK);
    assert!(boot_res.text().unwrap().contains("window, \"forge\""));

    // 3. Dot-dot traversal is blocked with 403 Forbidden
    let dotdot_res = app.get_asset("/../outside.txt").await;
    assert_eq!(dotdot_res.status, StatusCode::FORBIDDEN);

    let encoded_dotdot = app.get_asset("/%2e%2e/outside.txt").await;
    assert_eq!(encoded_dotdot.status, StatusCode::FORBIDDEN);

    // 4. Symlink escaping assets directory is blocked with 403 Forbidden
    #[cfg(unix)]
    {
        let symlink_res = app.get_asset("/escaped.txt").await;
        assert_eq!(symlink_res.status, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn executes_concurrent_commands_enforces_csrf_and_contains_panics() {
    let app = App::new()
        .configure(|cfg| {
            cfg.window.title = "Memory App".into();
            cfg.with_memory_asset("/index.html", b"<!doctype html><h1>Mem</h1>".to_vec());
        })
        .plugin(CounterPlugin)
        .headless()
        .expect("headless app with memory assets should boot");

    // 1. Concurrent typed command invocations sharing State<Counter>
    let (r1, r2) = tokio::join!(
        app.invoke::<_, AddRes>("counter.add", json!({ "delta": 3, "label": "Tasks" })),
        app.invoke::<_, AddRes>("counter.add", json!({ "delta": 4, "label": "Tasks" })),
    );
    assert!(r1.is_ok());
    assert!(r2.is_ok());

    let final_total: usize = app.invoke_unit("counter.get").await.unwrap();
    assert_eq!(final_total, 7);
    assert!(app.window_title().starts_with("Tasks ("));

    // 2. Structured domain error preserves code + details
    let domain_err = app
        .invoke::<_, AddRes>("counter.add", json!({ "delta": 1, "label": "   " }))
        .await
        .unwrap_err();
    assert_eq!(domain_err.code, "EMPTY_LABEL");
    assert_eq!(domain_err.details, Some(json!({ "field": "label" })));

    // 3. Unknown command & invalid payload produce distinct built-in error codes
    let unknown_err = app
        .invoke_unit::<serde_json::Value>("does.not.exist")
        .await
        .unwrap_err();
    assert_eq!(unknown_err.code, "FORGE_UNKNOWN_COMMAND");

    let payload_err = app
        .invoke::<_, AddRes>("counter.add", json!({ "wrong": true }))
        .await
        .unwrap_err();
    assert_eq!(payload_err.code, "FORGE_INVALID_PAYLOAD");

    // 4. Handler panic is caught cleanly without crashing the host
    let panic_err = app.invoke_unit::<()>("debug.panic").await.unwrap_err();
    assert_eq!(panic_err.code, "FORGE_HANDLER_PANIC");
    assert!(panic_err.message.contains("deliberate test explosion"));

    // 5. Forged IPC without X-Forge-Token is rejected with 403 Forbidden
    let forged_req = Request::builder()
        .method(Method::POST)
        .uri("forge://app/__forge_ipc/counter.get")
        .body(b"null".to_vec())
        .unwrap();
    let forged_res = app.raw_exchange(forged_req).await;
    assert_eq!(forged_res.status, StatusCode::FORBIDDEN);

    // 6. Window close directive is observable through HeadlessApp
    assert!(!app.is_closed());
    let _: String = app.invoke_unit("window.close").await.unwrap();
    assert!(app.is_closed());
}

#[test]
fn fails_fast_at_startup_on_missing_state_or_invalid_command_registration() {
    struct UnregisteredDb;

    async fn broken_cmd(_db: State<UnregisteredDb>) -> Result<(), Infallible> {
        Ok(())
    }

    // Missing State<UnregisteredDb> caught at boot before any command runs!
    let missing = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Hi</h1>".to_vec());
        })
        .command("broken", broken_cmd)
        .headless();
    assert!(matches!(
        missing,
        Err(Error::MissingState {
            command: "broken",
            ..
        })
    ));

    // Duplicate command caught at boot
    async fn ok_cmd() -> Result<(), Infallible> {
        Ok(())
    }
    let dup = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Hi</h1>".to_vec());
        })
        .command("ping", ok_cmd)
        .command("ping", ok_cmd)
        .headless();
    assert!(matches!(
        dup,
        Err(Error::DuplicateCommand { command: "ping" })
    ));

    // Reserved __forge prefix caught at boot
    let reserved = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Hi</h1>".to_vec());
        })
        .command("__forge_internal", ok_cmd)
        .headless();
    assert!(matches!(
        reserved,
        Err(Error::ReservedCommand {
            command: "__forge_internal"
        })
    ));
}
