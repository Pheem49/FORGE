# Testing FORGE Applications (`HeadlessApp` & Smoke Mode)

FORGE provides two complementary testing surfaces so desktop applications can verify 95%+ of their behavior in milliseconds without a display server, while still proving real native window creation and custom-scheme IPC in CI.

---

## 1. Deterministic Headless Testing (`App::headless()`)

`App::headless()` initializes the exact same `forge.toml` configuration, `.setup(...)` hooks, `State<T>` startup `TypeId` verification, asset security rules, and async command router as `App::run()`, returning a [`HeadlessApp`] harness with zero GUI dependencies.

### Public Interface
- `app.get_asset(path) -> RawResponse` — Exercises `GET forge://localhost/<path>`, HTML `/__forge/bootstrap.js` injection, MIME detection, CSP headers, and `403` path traversal guards.
- `app.invoke::<Payload, Reply>(cmd, payload) -> Result<Reply, ForgeError>` — Exercises authenticated `POST /__forge_ipc/<cmd>` calls on Tokio worker threads.
- `app.invoke_unit::<Reply>(cmd) -> Result<Reply, ForgeError>` — Convenience helper for zero-argument commands.
- `app.take_emitted_events() -> Vec<(String, Value)>` — Drains Rust → JS push events emitted via `WindowHandle::emit`.
- `app.window_title()`, `app.window_size()`, `app.is_maximized()`, `app.is_fullscreen()`, `app.is_closed()` — Observes window control state.

### Minimal Example
```rust
#[tokio::test]
async fn verifies_commands_and_assets_headlessly() {
    let app = my_app::build_app()
        .headless()
        .expect("headless boot failed");

    let index = app.get_asset("/").await;
    assert_eq!(index.status, forge::http::StatusCode::OK);

    let reply: String = app.invoke("greet", serde_json::json!({ "name": "Ada" })).await.unwrap();
    assert_eq!(reply, "Hello, Ada!");
}
```

---

## 2. Live Window Smoke Mode (`FORGE_SMOKE_TEST=1`)

Headless tests intentionally skip native windowing (`tao`/`wry` and `cef`). To verify in CI or headless Xvfb environments that a real window opens, loads `forge://localhost/index.html`, executes `/__forge/bootstrap.js`, and completes a Same-Origin `POST /__forge_ipc/*` round-trip, set `FORGE_SMOKE_TEST=1`:

```bash
# On a desktop session:
FORGE_SMOKE_TEST=1 forge dev --runtime webview
FORGE_SMOKE_TEST=1 forge dev --runtime cef

# On headless Linux CI with Xvfb:
FORGE_SMOKE_TEST=1 xvfb-run -a forge dev --runtime webview
```

### Behavior & Invariants
1. On `DOMContentLoaded`, `/__forge/bootstrap.js` drains any early startup events and invokes the internal `__forge_ready` endpoint carrying `navigator.userAgent`.
2. `ForgeKernel` logs `[FORGE_SMOKE_TEST] ready runtime=<webview|cef> ua=<...>` to stdout and dispatches `QuitApp`, causing `App::run()` to shut down cleanly with exit code `0`.
3. A 30-second background watchdog terminates the process with exit code `124` if the window or IPC bridge hangs before reaching `DOMContentLoaded`.
