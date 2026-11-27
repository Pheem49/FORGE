# FORGE Release Notes

All notable features, architectural milestones, and developer tooling updates for **FORGE** are documented in this file.

---

## v0.4.0 — "Production Desktop Parity & Trusted Distribution" (2026-11-27)

FORGE v0.4 closes the core production desktop gaps relative to Tauri and Electron while keeping FORGE's zero-macro, dual-runtime, and sub-5ms headless-testable design: adding declarative/imperative window close interception, unified custom titlebar drag regions across WebView & CEF Views, zero-overhead raw binary IPC, macOS/Windows code signing + notarization, an Ed25519-verified static OTA auto-updater, and core OS integration plugins (`opener`, `shortcut`, `deep-link`).

### Highlights
- **Window Lifecycle & CEF Views Parity**
  - Added `[window] on_close = "close" | "hide" | "emit"` (`CloseBehavior`), `window:close-requested` interception, `WindowHandle::destroy()` / `window.forge.window.destroy()`, and `always_on_top` across `WebViewHost`, `CefHost` (`WindowDelegate::can_close` + `set_always_on_top`), and `HeadlessApp::request_close`.
  - Added unified frameless titlebar drag regions via `[data-forge-drag-region]` / `[data-forge-no-drag]`: delegating `mousedown` on `webview` and wiring Blink CSS `-webkit-app-region: drag` into `CefDragHandler::on_draggable_regions_changed` → `Window::set_draggable_regions` on `cef`.
- **Zero-Macro Raw Binary IPC (`RawBytes` & `Meta<T>`)**
  - Added `forge::RawBytes` (`application/octet-stream`) and `forge::Meta<T>` (`X-Forge-Meta` JSON header extractor) supporting all 6 payload/reply combinations alongside standard `FromContext` extractors with zero procedural macros.
  - Updated `window.forge.invoke` / `invokeBytes` to automatically send and receive `Uint8Array` / `ArrayBuffer`, and added `fs.read_bytes` / `fs.write_bytes` to `AppFsPlugin`.
- **Code Signing, Notarization & Ed25519 Auto-Updater**
  - Added `CodeSigner` (`bundle/sign.rs`) behind `ProcessRunner`: inside-out macOS `codesign` (`dylib` → CEF helper `.app`s → `CEF.framework` → binary → `.app`) + `xcrun notarytool` + `xcrun stapler`, and Windows pre/post-NSIS `signtool`.
  - Added `forge signer keygen` / `forge signer sign`, `forge build --updater-key` emitting `dist/latest.json`, and `UpdaterPlugin` (`updater.check`, `updater.download_and_install`) enforcing HTTPS, monotonic SemVer, SHA-256, and metadata-bound Ed25519 verification (`v1\n<version>\n<platform>\n<url>\n<sha256>`).
- **Core OS Integration Plugins (`opener`, `shortcut`, `deep-link`)**
  - Added `OpenerPlugin` (`opener.open_url` with strict scheme allow-list blocking `javascript:`, `file:`, `data:`, and CLI flag injection).
  - Added `GlobalShortcutPlugin` (`shortcut.register` / `unregister` / `unregister_all` / `list` + `shortcut:triggered` + `HeadlessApp::trigger_shortcut`).
  - Added `DeepLinkPlugin` (`[app] schemes = [...]`, `deep_link.get_current`, `deep-link:open`) integrated with `SingleInstanceGuard` and OS installer manifests (`CFBundleURLTypes` in `Info.plist`, `MimeType=x-scheme-handler/...` in `.desktop`, and `HKCU\Software\Classes\...` in NSIS).

---

## v0.3.0 — "Trustworthy & Shippable" (2026-11-26)

FORGE v0.3 proves both runtimes (`webview` and real Chromium 156 `cef`) live end-to-end, enforces capability permissions by default, adds full desktop application features (multi-window, CEF Views API, system tray, native menus, single-instance lock, window-state persistence), expands `forge build` into a cross-OS installer pipeline, and rounds out developer tooling (`forge doctor`, `forge bindings`, Vue 3 & Svelte 5 templates, 3-OS CI & release workflows).

### Breaking Changes & Migration from v0.2
1. **Strict Permissions by Default (`[permissions]`)**
   - Omitting `[permissions]` in `forge.toml` now defaults to `mode = "enforce"` with an empty `allow = []` list (rejecting unlisted commands with HTTP `403` `FORGE_PERMISSION_DENIED`).
   - **Migration:** Run `forge permissions init` to auto-generate a strict `[permissions]` block from your registered commands, or explicitly opt out during prototyping with `allow = ["*"]` / `mode = "permissive"`.
2. **Cargo Package Renamed to `forge-app` (`[lib] name = "forge"`)**
   - Rust `use forge::*` imports are unchanged; in external `Cargo.toml` manifests declare `forge = { package = "forge-app", version = "0.3", default-features = false }`.
3. **`ForgeError::bad_request` HTTP Status Fix**
   - `ForgeError::bad_request(...)` now returns HTTP `400 Bad Request` (`ErrorKind::BadRequest`) instead of `422`; wire JSON `{ code, message, details }` is unchanged.

### Highlights
- **Live-Verified Chromium 156 (`CefHost` Views API) & 3-OS CI**
  - Rewrote `CefHost` onto CEF's native **Views API** (`window_create_top_level` + `browser_view_create` + `ForgeWindowDelegate`), enabling real `maximize`, `minimize`, `restore`, `set_fullscreen`, `set_size`, `set_position`, `show`, `hide`, and multi-window on Chromium.
  - Unified custom-scheme authority onto `forge://localhost` (`https://forge.localhost` on Windows WebView2) and configured CEF `DISPLAY_ISOLATED | STANDARD | SECURE | CORS_ENABLED | FETCH_ENABLED` scheme options.
  - Added `FORGE_SMOKE_TEST=1` (`__forge_ready` IPC round-trip + 30s watchdog) and `.github/workflows/ci.yml` (`ubuntu-24.04`, `windows-latest`, `macos-latest` + Linux `xvfb-run` live window smoke test + template build matrix).
- **Multi-Window, System Tray, Menus, Single-Instance & Window State**
  - Added `AppHandle` (`create_window`, `window`, `windows`, `emit_all`, `emit_to`, `set_tray`, `set_menu`, `quit`), `WindowOptions`, `[windows.<label>]`, built-in `window.*` & `event.*` IPC namespaces, and `window.forge.window` / `emit` / `emitTo` in JS.
  - Added opt-in `desktop-ui` feature (`tray-icon` + `muda`) supporting declarative `[tray]` and `[menu]` with native `forge.*` actions (`forge.quit`, `forge.show-main`, etc.) and headless test hooks (`HeadlessApp::click_menu`, `click_tray`).
  - Added `[app] single_instance = true` (`fd-lock` + authenticated loopback CLI argument forwarding via `app:second-instance`) and `[window] remember_state = true` (sandboxed `window-state.json` persistence with multi-monitor coordinate clamping).
- **Capability Permission System (`forge::kernel::permissions` & `forge permissions init`)**
  - Single-choke-point IPC enforcement supporting exact names, `namespace.*`, `*`, `deny` precedence, per-window `[permissions.windows.<label>]` overrides, and startup typo validation.
  - Added `FORGE_EXPORT_COMMANDS=<path>` and `forge permissions init`.
- **Cross-OS Bundler (`BundleTarget` Seam, Icon Pipeline & `SHA256SUMS`)**
  - Refactored `forge-cli` packaging into `bundle/` (`StagedApp` + `BundleTarget` trait) supporting `deb`, `appimage`, `tarball`, `nsis`, `zip`, `app`, and `dmg`, plus deterministic `SHA256SUMS` in `dist/`.
  - Added automatic icon pipeline (`icons.rs`) generating `.svg`, `128`/`256`/`512` `.png`, Windows `.ico`, and Apple `.icns` from a single source icon or FORGE's built-in badge.
- **Developer Experience & Deepened Architecture**
  - Added `forge bindings` (`.d.ts` generator with `ForgeCommandName` union + augmentable `ForgeCommandTypes`), `forge doctor` (environment diagnostics), and `--template vue` (Vue 3 + Vite + TS) & `--template svelte` (Svelte 5 + Vite + TS).
  - Split `kernel.rs` into focused private submodules (`kernel/{mod,assets,bootstrap,ipc,permissions,builtins,devtools,http}`), unified `forge::manifest::Manifest` across both crates, typed `ErrorKind`, and added `.github/workflows/release.yml`.

---

## v0.2.0 — "From Dev Loop to Distributable App" (2026-11-26)

FORGE v0.2 transforms the core dual-runtime framework into an end-to-end Rust desktop application builder with live reloading, React/Vite support, bidirectional push events, single-binary packaging, and built-in native plugins.

### Highlights
- **Bidirectional Rust → JS Push Events**
  - Added `WindowHandle::emit(event, payload)` and `SetupContext::window()` in Rust paired with `window.forge.listen` / `once` in JS, buffering early `.setup(...)` events until `DOMContentLoaded`.
- **Extended Window Controls & Live Dev Workflow**
  - Added `set_size`, `set_minimized`, `set_maximized`, `set_fullscreen`, `drag_window`, debounced `notify` CSS/HTML Hot Reload (`FORGE_DEV_WATCH=1`), `[dev]` Vite proxy, and `--template react`.
- **Single-Binary Asset Embedding & Initial Linux Bundler**
  - Zero-boilerplate `FORGE_EMBED_ARCHIVE` + `FORGE_EMBED_MANIFEST` compile-time embedding and initial `.tar.gz` / `.deb` packaging.
- **Standard Native Desktop Plugins (`forge::plugins`)**
  - `DialogPlugin`, `ClipboardPlugin`, `NotificationPlugin`, and sandboxed `AppFsPlugin`.

---

## v0.1.0 — "Write Once. Choose Your Runtime." (2026-11-26)

Initial MVP establishing FORGE's 2-crate deep-module architecture (`crates/forge` + `crates/forge-cli`) and runtime portability between OS System WebViews (`tao` + `wry`) and Chromium (`cef-rs`), plus `HeadlessApp` zero-GUI testing.
