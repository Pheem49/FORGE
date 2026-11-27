# FORGE v0.4 Architecture Guide

FORGE follows **Deep Module** design principles (`codebase-design`): hiding dual-engine windowing, capability security, JSON & raw binary IPC, asset sandboxing, code signing, and multi-OS packaging behind a compact, zero-macro Rust interface.

---

## 1. Two-Crate Workspace Topology

1. **`crates/forge` (Cargo package: `forge-app`, library: `forge`)**
   - **Public Surface:** `App`, `SetupContext`, `AppHandle`, `WindowHandle`, `WindowOptions`, `CloseBehavior`, `State<T>`, `Meta<T>`, `RawBytes`, `AppConfig`, `Config`, `Manifest`, `Error`, `ErrorKind`, `ForgeError`, `Plugin`, `HeadlessApp`, `SingleInstanceGuard`, `WindowStateStore`, and `forge::plugins::*`.
   - **Shared Manifest Schema (`manifest.rs`):** Single source of truth for `forge.toml` (`[app]`, `[window]`, `[windows.*]`, `[runtime]`, `[dev]`, `[permissions]`, `[tray]`, `[menu]`, `[bundle]`, `[bundle.macos]`, `[bundle.windows]`, `[updater]`) plus `UpdateReleaseManifest` (`latest.json`), consumed by both `forge-app` and `forge-app-cli` (`default-features = false`).
   - **Private `kernel/` Submodules:**
     - `mod.rs` — Engine-neutral `ForgeKernel::handle_request` router and `dispatch_async`.
     - `ipc.rs` — POST/CSRF/token verification, single-choke-point `PermissionPolicy::check`, `application/json` & `application/octet-stream` (`RawBytes` + `X-Forge-Meta`) routing, and `CatchUnwindFuture` panic containment.
     - `permissions.rs` — Default-enforce capability policy (`allow`, `deny`, `namespace.*`, per-window overrides, startup typo guard).
     - `builtins.rs` — Auto-registered `window.*` (`close`, `destroy`, `set_always_on_top`, `set_on_close`, etc.) and `event.*` command namespaces.
     - `assets.rs` — In-memory / disk asset serving, `..` & symlink confinement, HTML `<script src="/__forge/bootstrap.js">` injection.
     - `bootstrap.rs` & `bootstrap.js` — Frozen `window.forge` client bridge (`invoke`, `invokeBytes`, unified `[data-forge-drag-region]` delegation).
     - `devtools.rs` — `notify` live-reload watcher and `ureq` upstream dev server proxy.
   - **Window Coordinator & `RuntimeHost` Seam (`runtime/`):**
     - `WindowRegistry` coordinates multi-window state, `CloseDisposition` (`close` | `hide` | `emit`), `HostCommand`s, `HostEvent`s, native `forge.*` menu/tray/shortcut actions, `SingleInstanceGuard` hooks, and `WindowStateStore` geometry persistence.
     - `WebViewHost` (`tao` + `wry` + optional `tray-icon` / `muda` `desktop-ui` + `global-hotkey` `plugin-shortcut`).
     - `CefHost` (`cef-rs` Chromium 156 **Views API**: `window_create_top_level` + `browser_view_create` + `ForgeDragHandler` + `can_close` gate + `Browser::identifier()` window mapping).

2. **`crates/forge-cli` (Cargo package: `forge-app-cli`, binary: `forge`)**
   - `forge new` — Scaffolds `vanilla`, `react`, `vue`, or `svelte` projects.
   - `forge dev` — Runs `[dev].before_dev_command`, enables live reload, provisions `~/.cache/forge/cef`, and launches the app.
   - `forge build` — Compiles release binaries with embedded UI archives, packages OS installers (`deb`, `appimage`, `tarball`, `nsis`, `zip`, `app`, `dmg`) with custom URI scheme registration, runs `CodeSigner` (`codesign` + `notarytool` + `stapler` / `signtool`), and writes `dist/latest.json` + `SHA256SUMS`.
   - `forge signer` — Generates Ed25519 updater keypairs (`keygen`) and detached release signatures (`sign`).
   - `forge permissions init` & `forge bindings` — Inspect registered commands via `FORGE_EXPORT_COMMANDS` to generate `forge.toml` allow-lists and `.d.ts` TypeScript bindings.
   - `forge doctor` — Verifies toolchains and system libraries behind `ProcessRunner`.

---

## 2. Multi-Window, Capability & Binary IPC Request Flow

1. Page in window `"settings"` calls `window.forge.invoke("fs.read_bytes", { path: "avatar.png" })` (or `invokeBytes("fs.write_bytes", uint8, { path: "avatar.png" })`) over `forge://localhost`.
2. `WebViewHost` closure or `CefHost` `Browser::identifier()` stamps `ProtocolRequest { window: "settings", ... }`.
3. `ForgeKernel::handle_ipc` verifies `Sec-Fetch-Site`, constant-time `X-Forge-Token`, and `PermissionPolicy::check("settings", cmd)`.
4. On allow, the async handler executes inside `CatchUnwindFuture` with `WindowHandle`, `AppHandle`, `State<T>`, `Meta<T>`, and `RawBytes` / JSON deserialization, returning `application/json` or `application/octet-stream` directly.
