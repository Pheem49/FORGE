# FORGE — Agent & Contributor Guide

This file defines the engineering conventions, architectural invariants, and workflow rules for AI coding agents and contributors working on **FORGE**.

---

## 1. Project Overview & Architecture

FORGE is a Rust-first desktop application platform (**"Write once. Choose your runtime. Ship anywhere."**) supporting both OS System WebViews (`tao` + `wry`) and Chromium (`cef-rs` Views API) behind a single deep module interface.

### Workspace Structure
- `crates/forge/` (`package = "forge-app"`, `[lib] name = "forge"`) — Core desktop module (`App`, `SetupContext`, `AppHandle`, `WindowHandle`, `State<T>`, `Manifest`, `ForgeKernel`, `HeadlessApp`, `forge::plugins`, `single_instance`, `window_state`, and internal `RuntimeHost` adapters for `webview` and `cef`).
- `crates/forge-cli/` (`package = "forge-cli"`, `[[bin]] name = "forge"`) — Developer CLI (`forge new`, `forge dev`, `forge build`, `forge permissions init`, `forge bindings`, `forge doctor`, and automated CEF binary caching).
- `templates/` — Starter project templates (`vanilla`, `react`, `vue`, `svelte`).
- `examples/hello-forge/` — Reference diagnostic workbench demonstrating commands, push events, window controls, permissions, and native plugins.
- `docs/` — Architecture and per-feature documentation.

---

## 2. Documentation Rule (Required for Every New Feature)

- **Whenever a new feature is added, you MUST write a dedicated `.md` documentation file inside `docs/`** (for example: `docs/windows.md`, `docs/permissions.md`, `docs/bundling.md`).
- Each feature doc should cover:
  1. Purpose and user-facing behavior.
  2. Public interface (Rust types/methods, JS `window.forge.*` surface, `forge.toml` keys, or CLI flags).
  3. Minimal working usage example.
  4. Limitations, platform notes, or security invariants.
- When a change alters the high-level architecture, workspace layout, or quick-start workflow, also update `docs/architecture.md` and `README.md`.

---

## 3. Codebase Design Standards

Follow **Deep Module** principles:
- **Keep the external interface small and zero-macro.** Prefer plain `async fn` handlers with sealed `FromContext` extractors (`State<T>`, `WindowHandle`, `AppHandle`, `AppConfig`) over procedural macros.
- **One shared manifest schema.** `forge::manifest::Manifest` is shared by `forge-app` and `forge-cli` (`default-features = false`); never duplicate `forge.toml` structs across crates.
- **Accept dependencies at seams, don't bypass them.** External OS/CLI tools (`cargo`, `patchelf`, `dpkg-deb`, `appimagetool`, `makensis`, `hdiutil`, `pkg-config`, dev servers) must go through `ProcessRunner` / `CefProvisioner` / `BundleTarget` seams so automated tests verify them without network downloads or display servers.
- **The interface is the test surface.** Write integration tests against `App::headless()`, `FORGE_SMOKE_TEST=1`, and `forge-cli` entry points asserting observable outcomes.

---

## 4. Critical Runtime & Security Invariants

1. **CEF Multi-Process Subprocess Gate:** `App::run()` must run `runtime::enter_process()` *before* initializing Tokio or `.setup(...)` hooks so Chromium child processes (`--type=renderer`, `--type=gpu-process`, etc.) exit cleanly with zero side effects.
2. **Main-Thread Ownership:** Native GUI event loops (`tao` and `cef`) must execute on the OS main thread (`std::thread::current().name() == Some("main")`). Never require `#[tokio::main]` in user applications.
3. **IPC, Permission & Asset Security:**
   - All `/__forge_ipc/*` calls must enforce `POST`, `Sec-Fetch-Site`, constant-time `X-Forge-Token` verification, and single-choke-point `PermissionPolicy::check` (`mode = "enforce"` by default, `deny` always wins).
   - All disk reads/writes (`ForgeKernel` static assets, `AppFsPlugin`, `WindowStateStore`) must verify canonical ancestor containment inside the root directory *before* creating directories or reading/writing files, blocking `..` and symlink escapes.
   - Async command handlers must run inside panic containment (`catch_unwind`) so panics never unwind across C/C++ GTK or Chromium FFI stacks.

---

## 5. Verification Checklist Before Committing

Run all checks and confirm zero warnings and zero test failures before creating a commit:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p forge-app --features webview,cef,cef/dox,plugins,dev-tools,desktop-ui
```

### Git Hygiene
- Never commit build outputs (`/target`, `/dist`), local CEF caches, or `.agents/` / `skills-lock.json`.
- Keep commits focused with clear conventional commit messages (`feat:`, `fix:`, `docs:`, `refactor:`).
