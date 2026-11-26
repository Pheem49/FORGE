# FORGE v0.1 Architecture Guide

FORGE v0.1 follows **Deep Module** design principles (`codebase-design`): placing maximum cross-platform and dual-engine behavior behind a compact, zero-macro caller interface.

## 1. Module & Seam Topology

1. **`crates/forge` (Core Desktop Module)**
   - **External Interface**: `App`, `SetupContext`, `State<T>`, `WindowHandle`, `AppConfig`, `Config`, `Error`, `ForgeError`, `Plugin`, and `HeadlessApp`.
   - **Internal Seam (`pub(crate) trait RuntimeHost`)**: Separates the application kernel (`ForgeKernel`) from concrete browser/window adapters without leaking engine types into caller `main.rs`.
   - **Concrete Adapters**:
     - `WebViewHost` (`#[cfg(feature = "webview")]`): Uses `tao` + `wry` (`webkit2gtk-4.1` on Linux, `WebView2` on Windows, `WKWebView` on macOS).
     - `CefHost` (`#[cfg(feature = "cef")]`): Uses `cef-rs` (`Chromium 156`), handling multi-process child entry points and custom scheme serving.
     - `HeadlessApp`: Drives `ForgeKernel` directly in memory for `<5ms` zero-GUI verification in `cargo test`.

2. **`crates/forge-cli` (Developer Tooling Module)**
   - `forge new`: Scaffolds projects from `templates/vanilla`.
   - `forge dev`: Validates `forge.toml`, selects Cargo features (`webview` or `cef`), provisions cached CEF binaries into `~/.cache/forge/cef` via `download-cef`, configures dynamic loader variables (`CEF_PATH`, `LD_LIBRARY_PATH` / `DYLD_FALLBACK_LIBRARY_PATH` / `PATH`), and launches the application.

## 2. Unified Custom-Scheme Exchange (`forge://app/...`)

Rather than maintaining separate IPC bridges for WRY (`postMessage` + `evaluate_script`) and CEF (`CefV8Handler` + `CefProcessMessage` across Renderer/Browser OS processes), FORGE routes **both** static assets and async RPC invocations through a single custom-scheme protocol:

- `GET /index.html`, `/style.css`, `/main.js`: Canonicalized against path traversal (`..` and symlink escape blocked with `403 Forbidden`), served with `X-Content-Type-Options: nosniff` and a strict `Content-Security-Policy`, with `<script src="/__forge/bootstrap.js"></script>` injected at the top of `<head>`.
- `GET /__forge/bootstrap.js`: Defines `Object.freeze({ runtime, invoke })` on `window.forge`, closing over a random 256-bit per-launch token (`X-Forge-Token`).
- `POST /__forge_ipc/<command>`: Validates `Sec-Fetch-Site` and constant-time `X-Forge-Token` equality, extracts `State<T>` / `WindowHandle` / `AppConfig` and the JSON payload, runs the handler on Tokio inside `catch_unwind`, and returns JSON directly to the browser's `fetch()` Promise.

## 3. Multi-Process Subprocess Gate

Chromium spawns multiple child processes (`--type=renderer`, `--type=gpu-process`, `--type=utility`) by re-executing the application binary. `App::run()` executes `runtime::enter_process()` **before** starting Tokio or running `.setup(...)` closures, ensuring child processes exit cleanly without triggering duplicate database connections or side effects.
