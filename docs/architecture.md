# FORGE v0.3 Architecture Guide

FORGE follows **Deep Module** design principles (`codebase-design`): hiding dual-engine windowing, capability security, IPC serialization, asset sandboxing, and multi-OS packaging behind a compact, zero-macro Rust interface.

---

## 1. Two-Crate Workspace Topology

1. **`crates/forge` (Cargo package: `forge-app`, library: `forge`)**
   - **Public Surface:** `App`, `SetupContext`, `AppHandle`, `WindowHandle`, `WindowOptions`, `State<T>`, `AppConfig`, `Config`, `Manifest`, `Error`, `ErrorKind`, `ForgeError`, `Plugin`, `HeadlessApp`, `SingleInstanceGuard`, `WindowStateStore`, and `forge::plugins::*`.
   - **Shared Manifest Schema (`manifest.rs`):** Single source of truth for `forge.toml` (`[app]`, `[window]`, `[windows.*]`, `[runtime]`, `[dev]`, `[permissions]`, `[tray]`, `[menu]`, `[bundle]`), consumed by both `forge-app` and `forge-cli` (`default-features = false`).
   - **Private `kernel/` Submodules:**
     - `mod.rs` — Engine-neutral `ForgeKernel::handle_request` router and `dispatch_async`.
     - `ipc.rs` — POST/CSRF/token verification, single-choke-point `PermissionPolicy::check`, and `CatchUnwindFuture` panic containment.
     - `permissions.rs` — Default-enforce capability policy (`allow`, `deny`, `namespace.*`, per-window overrides, startup typo guard).
     - `builtins.rs` — Auto-registered `window.*` and `event.*` command namespaces.
     - `assets.rs` — In-memory / disk asset serving, `..` & symlink confinement, HTML `<script src="/__forge/bootstrap.js">` injection.
     - `bootstrap.rs` & `bootstrap.js` — Frozen `window.forge` client bridge.
     - `devtools.rs` — `notify` live-reload watcher and `ureq` upstream dev server proxy.
   - **Window Coordinator & `RuntimeHost` Seam (`runtime/`):**
     - `WindowRegistry` coordinates multi-window state, `HostCommand`s, `HostEvent`s, native `forge.*` menu/tray actions, and `WindowStateStore` geometry persistence.
     - `WebViewHost` (`tao` + `wry` + optional `tray-icon` / `muda` `desktop-ui`).
     - `CefHost` (`cef-rs` Chromium 156 **Views API**: `window_create_top_level` + `browser_view_create` + `Browser::identifier()` window mapping).

2. **`crates/forge-cli` (Cargo package: `forge-cli`, binary: `forge`)**
   - `forge new` — Scaffolds `vanilla`, `react`, `vue`, or `svelte` projects.
   - `forge dev` — Runs `[dev].before_dev_command`, enables live reload, provisions `~/.cache/forge/cef`, and launches the app.
   - `forge build` — Compiles release binaries with embedded UI archives and packages OS installers behind the **`BundleTarget` seam** (`deb`, `appimage`, `tarball`, `nsis`, `zip`, `app`, `dmg`) with multi-OS icon generation (`icons.rs`) and `SHA256SUMS`.
   - `forge permissions init` & `forge bindings` — Inspect registered commands via `FORGE_EXPORT_COMMANDS` to generate `forge.toml` allow-lists and `.d.ts` TypeScript bindings.
   - `forge doctor` — Verifies toolchains and system libraries behind `ProcessRunner`.

---

## 2. Multi-Window & Capability Request Flow

1. Page in window `"settings"` calls `window.forge.invoke("fs.read_text", { path: "note.md" })` (`POST /__forge_ipc/fs.read_text` over `forge://localhost`).
2. `WebViewHost` closure or `CefHost` `Browser::identifier()` stamps `ProtocolRequest { window: "settings", ... }`.
3. `ForgeKernel::handle_ipc` verifies `Sec-Fetch-Site`, constant-time `X-Forge-Token`, and `PermissionPolicy::check("settings", "fs.read_text")`.
4. On allow, the async handler executes inside `CatchUnwindFuture` with `WindowHandle` bound to `"settings"` and `AppHandle` bound to `WindowRegistry`.
