#![cfg(all(
    feature = "plugin-opener",
    feature = "plugin-deep-link",
    feature = "plugin-shortcut",
    feature = "plugin-updater"
))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey};
use forge::manifest::{
    UpdatePlatformEntry, UpdateReleaseManifest, canonical_update_signing_payload,
};
use forge::plugins::{
    ApplyOutcome, AvailableUpdate, DeepLinkPlugin, GlobalShortcutPlugin, MemoryUpdateTransport,
    OpenerPlugin, RecordedUrlOpener, UpdateApplier, UpdaterPlugin,
};
use forge::{App, ErrorKind, ForgeError, codes};
use serde_json::json;
use sha2::{Digest, Sha256};

#[derive(Clone, Default)]
struct RecordedApplier {
    applied: Arc<Mutex<Vec<(PathBuf, String)>>>,
}

impl UpdateApplier for RecordedApplier {
    fn apply(&self, staged_artifact: &Path, version: &str) -> Result<ApplyOutcome, ForgeError> {
        if let Ok(mut g) = self.applied.lock() {
            g.push((staged_artifact.to_path_buf(), version.to_string()));
        }
        Ok(ApplyOutcome::InstalledNeedsRestart {
            version: version.to_string(),
        })
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn make_signed_entry(
    key: &SigningKey,
    version: &str,
    platform: &str,
    url: &str,
    artifact_bytes: &[u8],
) -> UpdatePlatformEntry {
    let sha256 = sha256_hex(artifact_bytes);
    let msg = canonical_update_signing_payload(version, platform, url, &sha256);
    let sig = key.sign(&msg);
    UpdatePlatformEntry {
        url: url.to_string(),
        sha256,
        signature: BASE64.encode(sig.to_bytes()),
    }
}

#[tokio::test]
async fn updater_plugin_verifies_ed25519_signatures_sha256_https_and_prevents_downgrade() {
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let wrong_key = SigningKey::from_bytes(&[99u8; 32]);
    let pubkey_b64 = BASE64.encode(signing_key.verifying_key().to_bytes());

    let temp = tempfile::tempdir().unwrap();
    let transport = MemoryUpdateTransport::new();
    let applier = RecordedApplier::default();

    let manifest_url = "https://updates.forge.example/latest.json";
    let artifact_url = "https://updates.forge.example/forge-notes-0.5.0-x86_64.AppImage";
    let valid_binary = b"#!/bin/sh\necho FORGE v0.5.0\n".to_vec();

    // 1. Valid signed 0.5.0 release (> current 0.4.0)
    let valid_entry = make_signed_entry(
        &signing_key,
        "0.5.0",
        "linux-x86_64",
        artifact_url,
        &valid_binary,
    );
    let mut platforms = BTreeMap::new();
    platforms.insert("linux-x86_64".to_string(), valid_entry.clone());
    transport.insert_manifest(
        manifest_url,
        UpdateReleaseManifest {
            version: "0.5.0".to_string(),
            notes: Some("Faster IPC & security hardening".to_string()),
            pub_date: Some("2026-11-27T10:00:00Z".to_string()),
            platforms,
        },
    );
    transport.insert_artifact(artifact_url, valid_binary.clone());

    let app = App::new()
        .configure({
            let pubkey = pubkey_b64.clone();
            move |cfg| {
                cfg.with_memory_asset("/index.html", b"<h1>Updater</h1>".to_vec());
                cfg.app.name = "forge-notes".into();
                cfg.app.version = "0.4.0".into();
                cfg.updater.endpoints = vec![manifest_url.into()];
                cfg.updater.pubkey = Some(pubkey);
                cfg.permissions.allow = vec!["updater.*".into()];
            }
        })
        .plugin(
            UpdaterPlugin::new()
                .with_transport(transport.clone())
                .with_applier(applier.clone())
                .with_staging_dir(temp.path())
                .with_platform_key("linux-x86_64"),
        )
        .headless()
        .unwrap();

    let check: Option<AvailableUpdate> = app.invoke_unit("updater.check").await.unwrap();
    let avail = check.expect("newer version 0.5.0 should be detected");
    assert_eq!(avail.version, "0.5.0");
    assert_eq!(avail.current_version, "0.4.0");

    let outcome: ApplyOutcome = app
        .invoke_unit("updater.download_and_install")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        ApplyOutcome::InstalledNeedsRestart {
            version: "0.5.0".to_string()
        }
    );
    let events = app.take_emitted_events_for("main");
    assert!(events.iter().any(|(ev, _)| ev == "updater:progress"));
    let recorded = applier.applied.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(std::fs::read(&recorded[0].0).unwrap(), valid_binary);

    // 2. Tampered binary payload fails SHA-256 verification before applier runs
    transport.insert_artifact(artifact_url, b"CORRUPTED_MALWARE_BYTES".to_vec());
    let hash_err = app
        .invoke_unit::<ApplyOutcome>("updater.download_and_install")
        .await
        .unwrap_err();
    assert_eq!(hash_err.code, "UPDATER_HASH_MISMATCH");
    assert_eq!(hash_err.kind(), ErrorKind::Forbidden);
    assert_eq!(applier.applied.lock().unwrap().len(), 1);

    // 3. Version-swap replay attack (valid signature for 0.5.0 pasted under version 0.6.0) is rejected!
    let mut replay_platforms = BTreeMap::new();
    replay_platforms.insert("linux-x86_64".to_string(), valid_entry.clone());
    transport.insert_manifest(
        manifest_url,
        UpdateReleaseManifest {
            version: "0.6.0".to_string(),
            notes: None,
            pub_date: None,
            platforms: replay_platforms,
        },
    );
    let replay_err = app
        .invoke_unit::<Option<AvailableUpdate>>("updater.check")
        .await
        .unwrap_err();
    assert_eq!(replay_err.code, "UPDATER_BAD_SIGNATURE");

    // 3b. URL-swap attack (valid signature for artifact_url swapped to attacker URL) is rejected!
    let mut url_tampered = valid_entry.clone();
    url_tampered.url = "https://attacker.example/forge-notes.exe".to_string();
    let mut url_tamper_platforms = BTreeMap::new();
    url_tamper_platforms.insert("linux-x86_64".to_string(), url_tampered);
    transport.insert_manifest(
        manifest_url,
        UpdateReleaseManifest {
            version: "0.5.0".to_string(),
            notes: None,
            pub_date: None,
            platforms: url_tamper_platforms,
        },
    );
    let url_err = app
        .invoke_unit::<Option<AvailableUpdate>>("updater.check")
        .await
        .unwrap_err();
    assert_eq!(url_err.code, "UPDATER_BAD_SIGNATURE");

    // 4. Forged signature from wrong key is rejected
    let forged = make_signed_entry(
        &wrong_key,
        "0.6.0",
        "linux-x86_64",
        artifact_url,
        &valid_binary,
    );
    let mut forged_platforms = BTreeMap::new();
    forged_platforms.insert("linux-x86_64".to_string(), forged);
    transport.insert_manifest(
        manifest_url,
        UpdateReleaseManifest {
            version: "0.6.0".to_string(),
            notes: None,
            pub_date: None,
            platforms: forged_platforms,
        },
    );
    let sig_err = app
        .invoke_unit::<Option<AvailableUpdate>>("updater.check")
        .await
        .unwrap_err();
    assert_eq!(sig_err.code, "UPDATER_BAD_SIGNATURE");

    // 5. Downgrade / older version (`0.3.9 <= 0.4.0`) returns `None` cleanly
    let older = make_signed_entry(
        &signing_key,
        "0.3.9",
        "linux-x86_64",
        artifact_url,
        &valid_binary,
    );
    let mut older_platforms = BTreeMap::new();
    older_platforms.insert("linux-x86_64".to_string(), older);
    transport.insert_manifest(
        manifest_url,
        UpdateReleaseManifest {
            version: "0.3.9".to_string(),
            notes: None,
            pub_date: None,
            platforms: older_platforms,
        },
    );
    let none_check: Option<AvailableUpdate> = app.invoke_unit("updater.check").await.unwrap();
    assert!(none_check.is_none());

    // 6. Plain HTTP remote endpoint (including `http://localhost.evil.com` prefix spoof!) is blocked with 403 FORGE_FORBIDDEN
    let spoofed_loopback_app = App::new()
        .configure({
            let pk = pubkey_b64.clone();
            move |cfg| {
                cfg.with_memory_asset("/index.html", b"<h1>Spoof</h1>".to_vec());
                cfg.app.version = "0.4.0".into();
                cfg.updater.endpoints = vec!["http://localhost.evil.com/latest.json".into()];
                cfg.updater.pubkey = Some(pk);
                cfg.permissions.allow = vec!["updater.*".into()];
            }
        })
        .plugin(UpdaterPlugin::new().allow_loopback_http(true))
        .headless()
        .unwrap();
    let spoof_err = spoofed_loopback_app
        .invoke_unit::<Option<AvailableUpdate>>("updater.check")
        .await
        .unwrap_err();
    assert_eq!(spoof_err.code, codes::FORBIDDEN);

    let insecure_app = App::new()
        .configure(move |cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Insecure</h1>".to_vec());
            cfg.app.version = "0.4.0".into();
            cfg.updater.endpoints = vec!["http://evil.example.com/latest.json".into()];
            cfg.updater.pubkey = Some(pubkey_b64);
            cfg.permissions.allow = vec!["updater.*".into()];
        })
        .plugin(UpdaterPlugin::new().allow_loopback_http(false))
        .headless()
        .unwrap();
    let http_err = insecure_app
        .invoke_unit::<Option<AvailableUpdate>>("updater.check")
        .await
        .unwrap_err();
    assert_eq!(http_err.code, codes::FORBIDDEN);
    assert_eq!(http_err.kind(), ErrorKind::Forbidden);
}

#[tokio::test]
async fn opener_plugin_allows_https_and_mailto_while_blocking_dangerous_schemes() {
    let recorded = RecordedUrlOpener::new();
    let app = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>Opener</h1>".to_vec());
            cfg.permissions.allow = vec!["opener.*".into()];
        })
        .plugin(OpenerPlugin::new().with_opener(recorded.clone()))
        .headless()
        .unwrap();

    app.invoke::<_, ()>(
        "opener.open_url",
        json!({ "url": "https://github.com/Pheem49/FORGE" }),
    )
    .await
    .unwrap();
    app.invoke::<_, ()>("opener.open_url", json!({ "url": "mailto:team@forge.dev" }))
        .await
        .unwrap();
    assert_eq!(
        recorded.take_opened(),
        vec![
            "https://github.com/Pheem49/FORGE".to_string(),
            "mailto:team@forge.dev".to_string()
        ]
    );

    for blocked in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "data:text/html,<script>alert(1)</script>",
    ] {
        let err = app
            .invoke::<_, ()>("opener.open_url", json!({ "url": blocked }))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Forbidden, "must block {blocked}");
    }

    for malformed in [
        "-flag-injection",
        "https://",
        "https://bad\nnewline.com",
        "https://evil.com/\"quote-breakout",
    ] {
        let err = app
            .invoke::<_, ()>("opener.open_url", json!({ "url": malformed }))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BadRequest, "must reject {malformed}");
    }
    assert!(recorded.take_opened().is_empty());
}

#[tokio::test]
async fn global_shortcut_and_deep_link_plugins_work_end_to_end_in_headless() {
    let rust_hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let hits_clone = Arc::clone(&rust_hits);

    let app = App::new()
        .configure(|cfg| {
            cfg.with_memory_asset("/index.html", b"<h1>OS Plugins</h1>".to_vec());
            cfg.app.schemes = vec!["forge-notes".into()];
            cfg.permissions.allow = vec!["shortcut.*".into(), "deep_link.*".into()];
        })
        .plugin(
            GlobalShortcutPlugin::new()
                .with_shortcut("CmdOrCtrl+Shift+K")
                .on_shortcut(move |_app, accel| {
                    hits_clone.lock().unwrap().push(accel.to_string());
                }),
        )
        .plugin(DeepLinkPlugin::new().with_initial_args([
            "--flag",
            "forge-notes://open/note-1",
            "https://ignored.example.com",
        ]))
        .headless()
        .unwrap();

    // 1. Startup deep link captured & buffered into `deep-link:open`
    let initial_links: Vec<String> = app.invoke_unit("deep_link.get_current").await.unwrap();
    assert_eq!(initial_links, vec!["forge-notes://open/note-1".to_string()]);
    let startup_events = app.take_emitted_events_for("main");
    assert!(startup_events.iter().any(|(ev, payload)| {
        ev == "deep-link:open" && payload["urls"] == json!(["forge-notes://open/note-1"])
    }));

    // 2. Secondary instance handoff forwards new deep link to `deep_link.get_current` & `deep-link:open`
    app.simulate_second_instance(
        vec![
            "/usr/bin/forge-notes".into(),
            "forge-notes://open/note-99?tab=preview".into(),
        ],
        PathBuf::from("/tmp"),
    );
    let updated_links: Vec<String> = app.invoke_unit("deep_link.get_current").await.unwrap();
    assert_eq!(
        updated_links,
        vec!["forge-notes://open/note-99?tab=preview".to_string()]
    );
    let handoff_events = app.take_emitted_events_for("main");
    assert!(handoff_events.iter().any(|(ev, payload)| {
        ev == "deep-link:open"
            && payload["urls"] == json!(["forge-notes://open/note-99?tab=preview"])
    }));

    // 2b. Native macOS `Event::Opened` URL dispatch (`simulate_urls_opened`)
    app.simulate_urls_opened(vec!["forge-notes://open/macos-doc-7".into()]);
    let macos_links: Vec<String> = app.invoke_unit("deep_link.get_current").await.unwrap();
    assert_eq!(
        macos_links,
        vec!["forge-notes://open/macos-doc-7".to_string()]
    );

    // 3. Global shortcuts register, reject invalid accelerators, fire Rust + JS events, and unregister (case-insensitive!)
    app.invoke::<_, ()>("shortcut.register", json!({ "accelerator": "Alt+Space" }))
        .await
        .unwrap();
    let registered: Vec<String> = app.invoke_unit("shortcut.list").await.unwrap();
    assert_eq!(
        registered,
        vec!["Alt+Space".to_string(), "CmdOrCtrl+Shift+K".to_string()]
    );

    let bad_hk = app
        .invoke::<_, ()>(
            "shortcut.register",
            json!({ "accelerator": "NotAModifier+???" }),
        )
        .await
        .unwrap_err();
    assert_eq!(bad_hk.kind(), ErrorKind::BadRequest);

    app.trigger_shortcut("CmdOrCtrl+Shift+K");
    assert_eq!(
        *rust_hits.lock().unwrap(),
        vec!["CmdOrCtrl+Shift+K".to_string()]
    );
    let hk_events = app.take_emitted_events_for("main");
    assert!(hk_events.iter().any(|(ev, payload)| {
        ev == "shortcut:triggered" && payload["accelerator"] == "CmdOrCtrl+Shift+K"
    }));

    // Unregister using lowercase `"alt+space"` matches `"Alt+Space"` via HotKey::id() normalization
    let removed: bool = app
        .invoke("shortcut.unregister", json!({ "accelerator": "alt+space" }))
        .await
        .unwrap();
    assert!(removed);
    app.invoke_unit::<()>("shortcut.unregister_all")
        .await
        .unwrap();
    let after_clear: Vec<String> = app.invoke_unit("shortcut.list").await.unwrap();
    assert!(after_clear.is_empty());
}

#[test]
fn rejects_reserved_or_malformed_custom_uri_schemes_in_manifest() {
    let err_https = forge::Manifest::parse(
        r#"
[app]
name = "bad"
schemes = ["https"]
"#,
    )
    .unwrap_err();
    assert!(err_https.to_string().contains("reserved"));

    let err_upper = forge::Manifest::parse(
        r#"
[app]
name = "bad"
schemes = ["MyApp"]
"#,
    )
    .unwrap_err();
    assert!(err_upper.to_string().contains("lowercase"));
}
