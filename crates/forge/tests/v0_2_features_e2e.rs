use std::convert::Infallible;
use std::path::PathBuf;

use flate2::Compression;
use flate2::write::GzEncoder;
use forge::plugins::{AppFsPlugin, ClipboardPlugin, DialogPlugin, NotificationPlugin};
use forge::{App, ForgeError, WindowHandle};
use serde_json::json;
use tar::Builder;

fn build_sample_ui_tar_gz() -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = Builder::new(&mut enc);
        let html = b"<!doctype html><html><head></head><body><h1>Bundled UI</h1></body></html>";
        let mut header = tar::Header::new_gnu();
        header.set_size(html.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "index.html", &html[..])
            .unwrap();

        let css = b"body { background: #181c20; }";
        let mut css_header = tar::Header::new_gnu();
        css_header.set_size(css.len() as u64);
        css_header.set_mode(0o644);
        css_header.set_cksum();
        tar.append_data(&mut css_header, "assets/app.css", &css[..])
            .unwrap();
        tar.finish().unwrap();
    }
    enc.finish().unwrap()
}

async fn emit_and_resize(win: WindowHandle) -> Result<&'static str, ForgeError> {
    win.set_size(1280, 800);
    win.set_maximized(true);
    win.set_fullscreen(true);
    win.emit("job.completed", json!({ "ok": true, "items": 42 }))?;
    Ok("done")
}

#[tokio::test]
async fn supports_push_events_window_controls_and_embedded_tar_gz_assets() {
    let archive = build_sample_ui_tar_gz();

    let app = App::new()
        .configure(move |cfg| {
            cfg.load_embedded_tar_gz(&archive)
                .expect("embedded tar.gz should unpack");
        })
        .setup(|ctx| {
            // Emit an early event during startup BEFORE any JS listener is ready!
            ctx.window()
                .emit("app.booted", json!({ "stage": "setup" }))
                .map_err(|e| forge::Error::SetupFailed(e.to_string()))?;
            Ok(())
        })
        .command("run.job", emit_and_resize)
        .headless()
        .expect("headless app with embedded tar.gz should initialize without disk ui/");

    // 1. Embedded tar.gz serves "/" and "/assets/app.css" with injected bootstrap
    let root = app.get_asset("/").await;
    assert_eq!(root.status, forge::http::StatusCode::OK);
    let html = root.text().unwrap();
    assert!(html.contains("Bundled UI"));
    assert!(html.contains("/__forge/bootstrap.js"));

    let css = app.get_asset("/assets/app.css").await;
    assert_eq!(css.status, forge::http::StatusCode::OK);
    assert!(css.text().unwrap().contains("#181c20"));

    // 2. Early setup event is observable through HeadlessApp's public event surface
    let early_events = app.take_emitted_events();
    assert!(
        early_events
            .iter()
            .any(|(ev, data)| ev == "app.booted" && data["stage"] == "setup")
    );

    // 3. Handler emits live push event and updates window geometry/state
    let _: String = app.invoke_unit("run.job").await.unwrap();
    assert_eq!(app.window_size(), (1280, 800));
    assert!(app.is_maximized());
    assert!(app.is_fullscreen());

    let events = app.take_emitted_events();
    assert!(
        events
            .iter()
            .any(|(ev, data)| ev == "job.completed" && data["items"] == 42)
    );
}

#[tokio::test]
async fn standard_plugins_operate_and_enforce_sandbox_headlessly() {
    let temp = tempfile::tempdir().unwrap();
    let storage_dir = temp.path().join("app-data");
    let (notif_plugin, notif_log) = NotificationPlugin::recording();

    async fn ping() -> Result<&'static str, Infallible> {
        Ok("ok")
    }

    let app = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Plugins</h1>".to_vec());
        })
        .plugin(DialogPlugin::mock(
            Some(PathBuf::from("/tmp/selected.md")),
            Some(PathBuf::from("/tmp/saved.md")),
            true,
        ))
        .plugin(ClipboardPlugin::in_memory())
        .plugin(notif_plugin)
        .plugin(AppFsPlugin::with_root(&storage_dir))
        .command("ping", ping)
        .headless()
        .expect("app with all 4 plugins should boot");

    // 1. DialogPlugin mock seam
    let opened: Option<String> = app
        .invoke("dialog.open", json!({ "title": "Pick markdown" }))
        .await
        .unwrap();
    assert_eq!(opened.as_deref(), Some("/tmp/selected.md"));

    let confirmed: bool = app
        .invoke(
            "dialog.confirm",
            json!({ "title": "Delete?", "description": "Are you sure?" }),
        )
        .await
        .unwrap();
    assert!(confirmed);

    // 2. ClipboardPlugin in-memory seam
    let (): () = app
        .invoke("clipboard.write_text", json!({ "text": "FORGE v0.2" }))
        .await
        .unwrap();
    let clip: String = app.invoke_unit("clipboard.read_text").await.unwrap();
    assert_eq!(clip, "FORGE v0.2");

    // 3. NotificationPlugin recording seam
    let (): () = app
        .invoke(
            "notification.send",
            json!({ "title": "Build Ready", "body": "Packaged .deb" }),
        )
        .await
        .unwrap();
    assert_eq!(notif_log.lock().unwrap().len(), 1);
    assert_eq!(notif_log.lock().unwrap()[0].title, "Build Ready");

    // 4. AppFsPlugin sandboxed read/write/list + traversal rejection
    let (): () = app
        .invoke(
            "fs.write_text",
            json!({ "path": "notes/todo.txt", "contents": "Ship FORGE v0.2" }),
        )
        .await
        .unwrap();
    let files: Vec<String> = app
        .invoke("fs.list_dir", json!({ "path": "notes" }))
        .await
        .unwrap();
    assert_eq!(files, vec!["todo.txt"]);

    let text: String = app
        .invoke("fs.read_text", json!({ "path": "notes/todo.txt" }))
        .await
        .unwrap();
    assert_eq!(text, "Ship FORGE v0.2");

    let escape_err = app
        .invoke::<_, String>("fs.read_text", json!({ "path": "../../etc/passwd" }))
        .await
        .unwrap_err();
    assert_eq!(escape_err.code, "FS_FORBIDDEN");
}
