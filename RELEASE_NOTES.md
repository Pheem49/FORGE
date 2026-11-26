# FORGE Release Notes

All notable features, architectural milestones, and developer tooling updates for **FORGE** are documented in this file.

---

## v0.2.0 — "From Dev Loop to Distributable App" (2026-11-26)

FORGE v0.2 transforms the core dual-runtime framework into an end-to-end Rust desktop application builder with live reloading, React/Vite support, bidirectional push events, single-binary packaging, and built-in native plugins.

### Highlights
- **Bidirectional Rust → JS Push Events**
  - Added `WindowHandle::emit(event, payload)` and `SetupContext::window()` in Rust so command handlers and background Tokio tasks can stream real-time events to JavaScript.
  - Added `window.forge.listen(event, handler)` and `window.forge.once(event, handler)` to `/__forge/bootstrap.js`.
  - Early events emitted during `.setup(...)` are buffered and drained automatically on `DOMContentLoaded`.
- **Extended Window Controls**
  - Added `set_size`, `set_minimized`, `set_maximized`, `set_fullscreen`, `drag_window`, and `[window].decorations` in `forge.toml`.
- **Live Dev Workflow (`Hot Reload` & Vite/React HMR)**
  - Built-in debounced `notify` file watcher (`FORGE_DEV_WATCH=1`): `.css` edits hot-swap stylesheets in-place without losing UI state (`__forge:css-reload`), while `.html`/`.js` edits trigger automatic reload (`__forge:reload`).
  - Added `[dev]` configuration (`url`, `before_dev_command`, `before_build_command`) to proxy external frontend dev servers (Vite/React/Vue) cleanly under Same-Origin `forge://app/`.
  - Added `--template react` (Vite + React 19 + TypeScript with typed `src-ui/forge.d.ts`) to `forge new`.
- **Single-Binary Asset Embedding & Desktop Bundler (`forge build`)**
  - Added zero-boilerplate compile-time embedding (`crates/forge/build.rs`): `forge build` compresses `ui/` into `.tar.gz` and bakes both `ui/` and `forge.toml` directly into the release executable with zero `build.rs` or proc-macros in user projects.
  - Added `forge build [--runtime webview|cef] [--format all|bin|tarball|deb]` producing stripped standalone executables, portable `.tar.gz` bundles (with Linux `patchelf --set-rpath '$ORIGIN'` for Chromium/CEF), and Debian/Ubuntu `.deb` packages (`.desktop` entry + SVG icon).
- **Standard Native Desktop Plugins (`forge::plugins`)**
  - `DialogPlugin` (`dialog.open`, `dialog.save`, `dialog.confirm`) backed by async `rfd` + mock test seam.
  - `ClipboardPlugin` (`clipboard.read_text`, `clipboard.write_text`) backed by `arboard` + in-memory test seam.
  - `NotificationPlugin` (`notification.send`) backed by `notify-rust` + recording test seam.
  - `AppFsPlugin` (`fs.read_text`, `fs.write_text`, `fs.list_dir`, `fs.remove_file`) strictly confined inside `<data_dir>/<app.identifier>/` with pre-mkdir canonical ancestor symlink checks.
- **Upgraded Reference Workbench (`examples/hello-forge`)**
  - Showcases live 2-second Rust heartbeat push events, window maximize/restore controls, clipboard copy, and persistent sandboxed notes.

---

## v0.1.0 — "Write Once. Choose Your Runtime." (2026-11-26)

Initial MVP establishing FORGE's 2-crate deep-module architecture (`crates/forge` + `crates/forge-cli`) and runtime portability between OS System WebViews and Chromium.

### Highlights
- **Core Rust Desktop Module (`crates/forge`)**
  - Single-source declarative configuration via `forge.toml` (`[app]`, `[window]`, `[runtime]`) with `FORGE_*` environment overrides and programmatic `App::configure(...)`.
  - Zero-macro async command registration (`App::command`) using sealed `FromContext` extractors (`State<T>`, `WindowHandle`, `AppConfig`) disjoint from `DeserializeOwned` payloads, with startup `TypeId` verification for missing state.
  - Subprocess-safe `App::setup(...)` hook and automatic Phase 1 subprocess gate (`runtime::enter_process`) preventing Chromium helper processes (`--type=renderer`, `--type=gpu-process`) from triggering duplicate host side effects.
  - Panic containment (`catch_unwind`) on async handlers so panics return structured `FORGE_HANDLER_PANIC` responses instead of unwinding across C/C++ FFI.
- **Unified Custom-Scheme Exchange (`forge://app/...`)**
  - Serves static UI assets (`/` → `/index.html`) with strict path traversal & symlink escape rejection (`403 Forbidden`), MIME detection, `Content-Security-Policy`, and automatic `<script src="/__forge/bootstrap.js"></script>` injection.
  - Routes `window.forge.invoke(cmd, payload)` over Same-Origin `POST /__forge_ipc/<cmd>` authenticated by a random per-launch 256-bit `X-Forge-Token` and `Sec-Fetch-Site` validation.
- **Dual Desktop Runtime Adapters & Headless Testing**
  - `WebViewHost`: OS System WebView adapter (`tao` + `wry` with `devtools` and `linux-body`).
  - `CefHost`: Chromium adapter (`cef-rs`) using `CefSchemeHandlerFactory` + `CefResourceHandler` in the browser process with zero custom renderer V8 FFI glue.
  - `HeadlessApp` (`App::headless()`): Sub-5ms zero-GUI test harness verifying assets, CSRF security, and async commands inside `cargo test`.
- **Developer CLI (`crates/forge-cli`) & Vanilla Template**
  - `forge new <dir>` scaffolding `templates/vanilla` (`Cargo.toml`, `forge.toml`, `src/main.rs`, `ui/`).
  - `forge dev [--runtime webview|cef]` with automated CEF binary caching (`~/.cache/forge/cef`) via `download-cef` and dynamic library search path configuration.
