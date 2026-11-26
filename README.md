# FORGE

**Write once. Choose your runtime. Ship anywhere.**

FORGE is a Rust-first desktop application platform that lets you write a single Rust + Web UI codebase (Vanilla HTML/CSS/JS or React/Vite) and run or package it seamlessly on either the lightweight **OS System WebView** (`tao` + `wry`) or **Chromium** (`cef-rs`) with **zero code changes**.

---

## Features (`v0.2`)

- **Dual Runtime Engine (`webview` & `cef`)**: Instant, tiny ~5 MB builds with OS System WebViews (`WebKitGTK` / `WebView2` / `WKWebView`), or pixel-identical rendering across every OS with Chromium (`CEF`).
- **Zero-Macro Rust Handlers**: Standard `async fn` handlers with sealed `FromContext` extractors (`State<T>`, `WindowHandle`, `AppConfig`) and startup `TypeId` verification.
- **Bidirectional IPC + Push Events**:
  - **JS → Rust**: `await window.forge.invoke(cmd, payload)` over a unified Same-Origin `forge://app/` exchange protected by a per-launch 256-bit `X-Forge-Token`.
  - **Rust → JS**: `window.emit(event, payload)` paired with `window.forge.listen(event, fn)` and `window.forge.once(event, fn)`.
- **Live Dev Workflow (`Hot Reload` & Vite/React HMR)**:
  - In-place CSS hot-swap and automatic HTML/JS reload on save during `forge dev`.
  - Native `[dev].url` proxy & `before_dev_command` lifecycle for Vite / React / Vue HMR (`forge new my-app --template react`).
- **Single-Binary Asset Embedding & Desktop Bundler (`forge build`)**:
  - Automatically compresses and embeds `ui/` into the release binary with zero `build.rs` or proc-macros in user projects.
  - Generates standalone executables, portable `.tar.gz` bundles (with `$ORIGIN` rpath patching for CEF), and Debian/Ubuntu `.deb` installers (`dpkg-deb`).
- **Standard Native Desktop Plugins (`forge::plugins`)**:
  - `DialogPlugin` (`dialog.open`, `dialog.save`, `dialog.confirm`)
  - `ClipboardPlugin` (`clipboard.read_text`, `clipboard.write_text`)
  - `NotificationPlugin` (`notification.send`)
  - `AppFsPlugin` (`fs.read_text`, `fs.write_text`, `fs.list_dir`, `fs.remove_file` — confined inside `<data_dir>/<app.identifier>/`)
- **Sub-5ms Headless Testing (`App::headless()`)**: Exercise static/embedded assets, CSRF guards, async commands, push events, and mockable native plugins inside `cargo test` with no display server.

---

## Quick Start

### 1. Run the Reference Workbench (`Hot Reload` Enabled)

```bash
# Run on OS System WebView:
cargo run -p forge-cli -- dev --manifest-path examples/hello-forge/forge.toml --runtime webview

# Switch the exact same app to Chromium (CEF):
cargo run -p forge-cli -- dev --manifest-path examples/hello-forge/forge.toml --runtime cef
```

### 2. Package a Distributable Binary & `.deb` Installer

```bash
cargo run -p forge-cli -- build --manifest-path examples/hello-forge/forge.toml --runtime webview
```
Outputs are written to `examples/hello-forge/dist/`:
- `dist/hello-forge/hello-forge` (standalone single-file executable with UI assets embedded inside!)
- `dist/hello-forge-0.2.0-linux-x86_64.tar.gz`
- `dist/hello-forge_0.2.0_amd64.deb`

### 3. Scaffold a New Project (`vanilla` or `react`)

```bash
# Vanilla HTML/CSS/JS + Rust:
cargo run -p forge-cli -- new my-app

# React + Vite + TypeScript + Rust:
cargo run -p forge-cli -- new my-react-app --template react
```

### 4. Run the Automated Test Suite

```bash
cargo test --workspace
```
