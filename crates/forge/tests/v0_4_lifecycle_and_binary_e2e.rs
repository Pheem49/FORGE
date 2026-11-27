use std::convert::Infallible;

use forge::plugins::AppFsPlugin;
use forge::{App, CloseBehavior, ErrorKind, ForgeError, Meta, RawBytes, State, codes};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize)]
struct ChunkMeta {
    filename: String,
    xor_key: u8,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
struct UploadSummary {
    filename: String,
    len: usize,
    checksum: u32,
}

struct SecretKey(u8);

async fn xor_cipher(key: State<SecretKey>, input: RawBytes) -> Result<RawBytes, Infallible> {
    let out: Vec<u8> = input.iter().map(|b| b ^ key.0).collect();
    Ok(RawBytes(out))
}

async fn upload_with_meta(
    Meta(meta): Meta<ChunkMeta>,
    payload: RawBytes,
) -> Result<UploadSummary, Infallible> {
    let checksum: u32 = payload.iter().map(|&b| u32::from(b ^ meta.xor_key)).sum();
    Ok(UploadSummary {
        filename: meta.filename,
        len: payload.len(),
        checksum,
    })
}

async fn generate_magic_header() -> Result<RawBytes, Infallible> {
    Ok(RawBytes(vec![0x00, 0xFF, 0x46, 0x4F, 0x52, 0x47, 0x45]))
}

#[tokio::test]
async fn intercepts_window_close_for_hide_and_emit_until_destroy_is_called() {
    let app = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Lifecycle</h1>".to_vec());
            cfg.window.on_close = CloseBehavior::Hide;
            cfg.window.always_on_top = true;
            cfg.permissions.allow = vec!["window.*".into()];
        })
        .headless()
        .expect("headless app should initialize");

    assert!(app.is_always_on_top());
    assert!(app.is_visible_for("main"));

    // 1. `on_close = "hide"` hides the window instead of closing or terminating the app
    app.request_close("main");
    assert!(!app.is_closed());
    assert!(!app.is_visible_for("main"));
    assert_eq!(app.windows(), vec!["main".to_string()]);

    // Re-show and switch to `on_close = "emit"` dynamically via built-in IPC
    app.invoke_unit::<()>("window.show").await.unwrap();
    assert!(app.is_visible_for("main"));
    app.invoke::<_, ()>("window.set_on_close", json!({ "on_close": "emit" }))
        .await
        .unwrap();
    app.invoke::<_, ()>(
        "window.set_always_on_top",
        json!({ "always_on_top": false }),
    )
    .await
    .unwrap();
    assert!(!app.is_always_on_top());

    // 2. Polite `window.close` (or OS `[X]`) cancels destruction and emits `window:close-requested`
    app.invoke_unit::<()>("window.close").await.unwrap();
    assert!(!app.is_closed());
    assert_eq!(app.windows(), vec!["main".to_string()]);
    let events = app.take_emitted_events_for("main");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "window:close-requested");
    assert_eq!(events[0].1["label"], "main");

    // 3. `window.destroy` bypasses `on_close = "emit"` and closes the window cleanly
    app.invoke_unit::<()>("window.destroy").await.unwrap();
    assert!(app.windows().is_empty());
    assert!(app.is_closed());
}

#[tokio::test]
async fn transfers_raw_binary_ipc_and_meta_headers_without_json_encoding() {
    let app = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Binary</h1>".to_vec());
            cfg.permissions.allow =
                vec!["cipher.xor".into(), "upload.chunk".into(), "magic".into()];
        })
        .state(SecretKey(0xAA))
        .command("cipher.xor", xor_cipher)
        .command("upload.chunk", upload_with_meta)
        .command("magic", generate_magic_header)
        .headless()
        .expect("binary IPC app should boot");

    // 1. 1 MB non-UTF8 buffer with 0x00 and 0xFF round-trips byte-for-byte
    let original: Vec<u8> = (0..1_048_576).map(|i| (i % 256) as u8).collect();
    let encrypted = app
        .invoke_raw("cipher.xor", original.clone())
        .await
        .unwrap();
    assert_eq!(encrypted.len(), original.len());
    assert_ne!(encrypted, original);
    let decrypted = app.invoke_raw("cipher.xor", encrypted).await.unwrap();
    assert_eq!(decrypted, original);

    // 2. Structured UTF-8 + `%` metadata in `X-Forge-Meta` alongside raw bytes
    let summary: UploadSummary = app
        .invoke_raw_with_meta(
            "upload.chunk",
            ChunkMeta {
                filename: "รายงาน-100%.bin".to_string(),
                xor_key: 0x0F,
            },
            vec![1u8, 2, 3, 4],
        )
        .await
        .unwrap();
    assert_eq!(
        summary,
        UploadSummary {
            filename: "รายงาน-100%.bin".to_string(),
            len: 4,
            checksum: u32::from(1u8 ^ 0x0F)
                + u32::from(2u8 ^ 0x0F)
                + u32::from(3u8 ^ 0x0F)
                + u32::from(4u8 ^ 0x0F),
        }
    );

    // 3. No-payload binary reply (`magic`)
    let header = app.invoke_json_to_raw("magic", json!(null)).await.unwrap();
    assert_eq!(header, vec![0x00, 0xFF, b'F', b'O', b'R', b'G', b'E']);

    // 4. Sending JSON Content-Type to a `RawBytes` command fails with 400 FORGE_INVALID_PAYLOAD
    let mismatch: ForgeError = app
        .invoke::<_, serde_json::Value>("cipher.xor", json!({ "bad": true }))
        .await
        .unwrap_err();
    assert_eq!(mismatch.code, codes::INVALID_PAYLOAD);
    assert_eq!(mismatch.kind(), ErrorKind::BadRequest);
}

#[cfg(feature = "plugin-fs")]
#[tokio::test]
async fn sandboxed_fs_plugin_reads_and_writes_raw_bytes_safely() {
    let temp = tempfile::tempdir().unwrap();
    let app = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>FS Bytes</h1>".to_vec());
            cfg.permissions.allow = vec!["fs.*".into()];
        })
        .plugin(AppFsPlugin::with_root(temp.path()))
        .headless()
        .unwrap();

    let png_like = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0xFF];
    app.invoke_raw_with_meta::<_, ()>(
        "fs.write_bytes",
        json!({ "path": "avatars/hero.png" }),
        png_like.clone(),
    )
    .await
    .unwrap();

    let read_back = app
        .invoke_json_to_raw("fs.read_bytes", json!({ "path": "avatars/hero.png" }))
        .await
        .unwrap();
    assert_eq!(read_back, png_like);

    // Path traversal (`../escape.bin`) is rejected with 403 Forbidden before touching disk
    let escape_err = app
        .invoke_raw_with_meta::<_, ()>(
            "fs.write_bytes",
            json!({ "path": "../escape.bin" }),
            vec![1, 2, 3],
        )
        .await
        .unwrap_err();
    assert_eq!(escape_err.kind(), ErrorKind::Forbidden);
}
