# FORGE v0.2 Architecture Guide

FORGE follows **Deep Module** design principles (`codebase-design`): placing maximum cross-platform, dual-engine, and packaging behavior behind a compact, zero-macro caller interface.

## 1. Two-Crate Deep Module Topology

1. **`crates/forge` (Core Desktop & Plugin Module)**
   - **External Interface**: `App`, `SetupContext`, `State<T>`, `WindowHandle`, `AppConfig`, `Config`, `Error`, `ForgeError`, `Plugin`, `HeadlessApp`, and `forge::plugins::*`.
   - **Internal Seam (`pub(crate) trait RuntimeHost`)**: Separates `ForgeKernel` from concrete desktop adapters (`WebViewHost`, `CefHost`, and `HeadlessApp`).
   - **Compile-Time Asset Embedding (`crates/forge/build.rs`)**: Reads `FORGE_EMBED_ARCHIVE` during `forge build` and embeds compressed UI assets directly into `ForgeKernel` via `include_bytes!`, keeping user projects free of `build.rs` boilerplate.

2. **`crates/forge-cli` (Scaffolder, Live Runner & Bundler Module)**
   - `forge new`: Scaffolds `vanilla` or `react` (Vite + TypeScript) starter applications.
   - `forge dev`: Enables live asset watching (`FORGE_DEV_WATCH=1`), spawns `[dev].before_dev_command` when configured, provisions cached CEF binaries (`~/.cache/forge/cef`), and launches the app.
   - `forge build`: Runs `[dev].before_build_command`, compresses `ui/` into `.tar.gz`, compiles a `--release` binary with `FORGE_EMBED_ARCHIVE`, strips symbols, patches ELF `$ORIGIN` rpath for CEF bundles, and builds `.tar.gz` and `.deb` packages.

## 2. Unified Custom-Scheme Exchange & Push Events

- **Client → Host (`window.forge.invoke`)**: Uses `fetch('/__forge_ipc/<cmd>')` over Same-Origin `forge://app/` with a closure-scoped 256-bit `X-Forge-Token`.
- **Host → Client (`WindowHandle::emit` → `window.forge.listen`)**: Dispatches `HostDirective::EmitEvent` onto the GUI thread (`webview.evaluate_script` in WRY; `Frame::execute_java_script` in CEF) while buffering early `.setup(...)` events until `DOMContentLoaded` drains `__forge_drain_events`.
- **Live Reload**: Uses `notify` in `ForgeKernel` to emit `__forge:css-reload` (in-place stylesheet swap) and `__forge:reload` (full page reload).
