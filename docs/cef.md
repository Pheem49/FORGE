# Chromium Embedded Framework (`cef`) Runtime

FORGE supports running identical application code on either the OS system WebView (`tao` + `wry`) or a bundled **Chromium 156** engine via **`cef-rs`** (`--runtime cef`).

---

## 1. Purpose & User-Facing Behavior

Use `--runtime cef` when you require:
- Identical Blink rendering, CSS engine behavior, and V8 JavaScript execution across Linux, Windows, and macOS.
- Self-contained Linux distributions (such as AppImage or `.tar.gz`) that do not depend on the end user's installed `libwebkit2gtk` version.

Switch runtimes without modifying application Rust or JavaScript code:

```bash
# Run in development with Chromium
forge dev --runtime cef

# Package a self-contained Chromium release bundle
forge build --runtime cef
```

---

## 2. Public Interface & Configuration

### `forge.toml`

```toml
[runtime]
default = "cef" # "webview" | "cef"
```

### Cargo Feature Flags

```toml
[dependencies]
forge = { package = "forge-app", version = "0.3", default-features = false }

[features]
default = ["webview"]
webview = ["forge/webview"]
cef = ["forge/cef"]
```

### Environment Variables

| Variable | Description |
| :--- | :--- |
| `FORGE_RUNTIME=cef` | Overrides `[runtime].default` at launch. |
| `CEF_PATH` | Path to extracted Chromium CEF binary distribution (automatically provisioned and cached by `forge dev` / `forge build` under `~/.cache/forge/cef/<version>/`). |
| `FORGE_SMOKE_TEST=1` | Boots Chromium, verifies full `forge://localhost` + `POST /__forge_ipc/*` round-trip, prints User-Agent, and exits `0`. |

---

## 3. Minimal Example

```rust
use forge::{App, ForgeError};
use serde_json::{Value, json};

async fn ping(_payload: Value) -> Result<Value, ForgeError> {
    Ok(json!({ "engine": "ready" }))
}

fn main() {
    App::new()
        .command("ping", ping)
        .run()
        .expect("FORGE runtime failed");
}
```

Verify headless and live Chromium execution:

```bash
FORGE_SMOKE_TEST=1 forge dev --runtime cef
# Output:
# [FORGE_SMOKE_TEST] ready runtime=cef ua=Mozilla/5.0 (X11; Linux x86_64) ... Chrome/156.0.0.0 Safari/537.36
```

---

## 4. Architecture, Spike Findings & Security Invariants

### Multi-Process Subprocess Gate
Chromium spawns helper processes (`--type=renderer`, `--type=gpu-process`, `--type=utility`) using the **same executable**. `App::run()` executes `CefHost::enter_process()` (`cef::execute_process`) **before** starting Tokio, parsing state, or running `.setup(...)` hooks so child processes exit cleanly with zero host side effects.

### Custom Scheme (`forge://localhost`) & Origin Rules
- Both `WebViewHost` and `CefHost` serve application assets and IPC over the unified **`forge://localhost`** authority.
- In `CefHost`, `forge` is registered with:
  `STANDARD | DISPLAY_ISOLATED | SECURE | CORS_ENABLED | FETCH_ENABLED`
  - **Why `DISPLAY_ISOLATED` instead of `LOCAL`:** In Chromium/CEF, `CEF_SCHEME_OPTION_LOCAL` applies `file://` semantics, turning the page origin into an opaque `'null'` origin and blocking same-origin `fetch('/__forge_ipc/...')` calls under CORS. Combining `STANDARD | SECURE | DISPLAY_ISOLATED | CORS_ENABLED | FETCH_ENABLED` preserves a genuine `forge://localhost` tuple origin for Fetch/IPC while preventing external web origins from framing or linking to `forge://` resources.
- `PostData::elements` in `cef-rs` requires a pre-sized `vec![None; count]` buffer (`elements.len()` determines the maximum element count copied across C FFI).
- On Linux, `Settings { no_sandbox: 1 }` is paired with `--disable-gpu-sandbox` while keeping Chromium's zygote intact (`--no-zygote` is intentionally avoided).

### Window Identification & Native Desktop UI
- `SchemeHandlerFactory::create` receives `browser: Option<&mut Browser>` with a stable `Browser::identifier()` (`Some(1)`, `Some(2)`, ...) for every asset and `fetch()` request, allowing multi-window routing to map browser IDs to FORGE window labels (falling back to `"main"` when `browser` is `None`).
- System tray (`tray-icon`) and native menu bars (`muda`) on Linux rely on a GTK main loop, whereas `CefHost` runs Chromium's Aura/Views message pump. Tray and menu operations are first-class on `WebViewHost` and return a typed `FORGE_UNSUPPORTED` (`501`) error on `CefHost` rather than silently no-oping.
