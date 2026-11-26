use std::convert::Infallible;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use forge::plugins::{AppFsPlugin, ClipboardPlugin, DialogPlugin, NotificationPlugin};
use forge::{App, AppConfig, ForgeError, State, WindowHandle};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default)]
pub struct WorkbenchState {
    invocations: AtomicU64,
    counter: AtomicU64,
}

#[derive(Debug, Deserialize)]
pub struct GreetInput {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GreetOutput {
    pub greeting: String,
    pub runtime: String,
    pub invocations: u64,
}

#[derive(Debug, Deserialize)]
pub struct CounterStep {
    pub step: u64,
    pub sync_window_title: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CounterOutput {
    pub value: u64,
    pub window_title: String,
}

#[derive(Debug, Deserialize)]
pub struct WindowActionReq {
    pub action: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HostStatus {
    pub app_name: String,
    pub version: String,
    pub runtime: String,
    pub pid: u32,
    pub os: String,
    pub arch: String,
    pub counter: u64,
}

async fn host_status(
    state: State<WorkbenchState>,
    cfg: AppConfig,
) -> Result<HostStatus, Infallible> {
    Ok(HostStatus {
        app_name: cfg.app.name.clone(),
        version: cfg.app.version.clone(),
        runtime: cfg.runtime.default.as_str().to_string(),
        pid: std::process::id(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        counter: state.counter.load(Ordering::Relaxed),
    })
}

async fn greet(
    state: State<WorkbenchState>,
    cfg: AppConfig,
    window: WindowHandle,
    input: GreetInput,
) -> Result<GreetOutput, ForgeError> {
    let trimmed = input.name.trim();
    if trimmed.is_empty() {
        return Err(ForgeError::bad_request(
            "NAME_REQUIRED",
            "Enter your name before sending a greeting.",
        )
        .with_details(json!({ "field": "name" })));
    }

    let count = state.invocations.fetch_add(1, Ordering::Relaxed) + 1;
    let runtime = cfg.runtime.default.as_str().to_string();
    let out = GreetOutput {
        greeting: format!("Hello, {trimmed}. Rust processed request #{count} on {runtime}."),
        runtime,
        invocations: count,
    };
    window.emit("bench.greeted", &out)?;
    Ok(out)
}

async fn step_counter(
    state: State<WorkbenchState>,
    cfg: AppConfig,
    window: WindowHandle,
    input: CounterStep,
) -> Result<CounterOutput, Infallible> {
    let next = state.counter.fetch_add(input.step, Ordering::Relaxed) + input.step;
    let title = if input.sync_window_title {
        let updated = format!("{} ({next})", cfg.window.title);
        window.set_title(&updated);
        updated
    } else {
        cfg.window.title.clone()
    };

    Ok(CounterOutput {
        value: next,
        window_title: title,
    })
}

async fn window_control(window: WindowHandle, req: WindowActionReq) -> Result<String, ForgeError> {
    match req.action.as_str() {
        "maximize" => window.set_maximized(true),
        "restore" => {
            window.set_maximized(false);
            window.set_fullscreen(false);
        }
        "fullscreen" => window.set_fullscreen(true),
        other => {
            return Err(ForgeError::bad_request(
                "UNKNOWN_WINDOW_ACTION",
                format!("unsupported window action '{other}'"),
            ));
        }
    }
    Ok(req.action)
}

pub fn build_app(root: Option<&Path>, fs_root_override: Option<&Path>) -> App {
    let base = match root {
        Some(p) => App::from_root(p),
        None => App::new(),
    };

    let fs_plugin = match fs_root_override {
        Some(p) => AppFsPlugin::with_root(p),
        None => AppFsPlugin::new(),
    };

    base.plugin(DialogPlugin::new())
        .plugin(ClipboardPlugin::new())
        .plugin(NotificationPlugin::new())
        .plugin(fs_plugin)
        .setup(|ctx| {
            ctx.insert_state(WorkbenchState::default());

            // Background Rust heartbeat pushing live events to JS every 2 seconds.
            let win = ctx.window();
            ctx.spawn(async move {
                let mut tick: u64 = 0;
                loop {
                    forge::tokio::time::sleep(Duration::from_secs(2)).await;
                    tick += 1;
                    let _ = win.emit("host.heartbeat", json!({ "tick": tick }));
                }
            });
            Ok(())
        })
        .command("host.status", host_status)
        .command("bench.greet", greet)
        .command("bench.increment", step_counter)
        .command("bench.window", window_control)
}

fn main() -> forge::Result<()> {
    build_app(None, None).run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn hello_forge_v0_2_commands_events_and_storage_work_headlessly() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let temp = tempfile::tempdir().unwrap();
        let app = build_app(Some(&manifest_dir), Some(temp.path()))
            .headless()
            .expect("hello-forge v0.2 should initialize headlessly");

        let index = app.get_asset("/").await;
        assert_eq!(index.status, forge::http::StatusCode::OK);

        let status: HostStatus = app.invoke_unit("host.status").await.unwrap();
        assert_eq!(status.app_name, "hello-forge");
        assert_eq!(status.version, "0.2.0");

        let out: GreetOutput = app
            .invoke("bench.greet", json!({ "name": "Pheem" }))
            .await
            .unwrap();
        assert!(out.greeting.contains("Pheem"));

        // Verify Rust -> JS event was emitted by `bench.greet`
        let events = app.take_emitted_events();
        assert!(events.iter().any(|(topic, _)| topic == "bench.greeted"));

        // Verify sandboxed AppFsPlugin note persistence
        let (): () = app
            .invoke(
                "fs.write_text",
                json!({ "path": "scratchpad.txt", "contents": "Saved via AppFsPlugin" }),
            )
            .await
            .unwrap();
        let saved: String = app
            .invoke("fs.read_text", json!({ "path": "scratchpad.txt" }))
            .await
            .unwrap();
        assert_eq!(saved, "Saved via AppFsPlugin");

        // Verify window control command
        let _: String = app
            .invoke("bench.window", json!({ "action": "maximize" }))
            .await
            .unwrap();
        assert!(app.is_maximized());
    }
}
