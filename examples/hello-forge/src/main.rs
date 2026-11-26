use std::convert::Infallible;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

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

#[derive(Debug, Serialize, Deserialize)]
pub struct HostStatus {
    pub app_name: String,
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

    Ok(GreetOutput {
        greeting: format!("Hello, {trimmed}. Rust processed request #{count} on {runtime}."),
        runtime,
        invocations: count,
    })
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

pub fn build_app(root: Option<&Path>) -> App {
    let base = match root {
        Some(p) => App::from_root(p),
        None => App::new(),
    };
    base.setup(|ctx| {
        ctx.insert_state(WorkbenchState::default());
        Ok(())
    })
    .command("host.status", host_status)
    .command("bench.greet", greet)
    .command("bench.increment", step_counter)
}

fn main() -> forge::Result<()> {
    build_app(None).run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn hello_forge_commands_and_ui_work_headlessly() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let app = build_app(Some(&manifest_dir))
            .headless()
            .expect("hello-forge should initialize headlessly");

        let index = app.get_asset("/").await;
        assert_eq!(index.status, forge::http::StatusCode::OK);

        let status: HostStatus = app.invoke_unit("host.status").await.unwrap();
        assert_eq!(status.app_name, "hello-forge");

        let out: GreetOutput = app
            .invoke("bench.greet", json!({ "name": "Pheem" }))
            .await
            .unwrap();
        assert!(out.greeting.contains("Pheem"));
        assert_eq!(out.invocations, 1);

        let err = app
            .invoke::<_, GreetOutput>("bench.greet", json!({ "name": "  " }))
            .await
            .unwrap_err();
        assert_eq!(err.code, "NAME_REQUIRED");

        let stepped: CounterOutput = app
            .invoke(
                "bench.increment",
                json!({ "step": 5, "sync_window_title": true }),
            )
            .await
            .unwrap();
        assert_eq!(stepped.value, 5);
        assert_eq!(app.window_title(), "FORGE Workbench (5)");
    }
}
