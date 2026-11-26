# FORGE — Agent & Contributor Guide

This file defines the engineering conventions, architectural invariants, and workflow rules for AI coding agents and contributors working on **FORGE**.

---

## 1. Project Overview & Architecture

FORGE is a Rust-first desktop application platform (**"Write once. Choose your runtime."**) supporting both OS System WebViews (`tao` + `wry`) and Chromium (`cef-rs`) behind a single deep module interface.

### Workspace Structure
- `crates/forge/` — Core desktop module (`App`, `SetupContext`, `State<T>`, `WindowHandle`, `ForgeKernel`, `HeadlessApp`, `forge::plugins`, and internal `RuntimeHost` adapters for `webview` and `cef`).
- `crates/forge-cli/` — Developer CLI (`forge new`, `forge dev`, `forge build`, and automated CEF binary caching).
- `templates/` — Starter project templates (`vanilla`, `react`).
- `examples/hello-forge/` — Reference diagnostic workbench demonstrating commands, push events, window controls, and native plugins.
- `docs/` — Architecture and feature documentation.

---

## 2. Documentation Rule (Required for Every New Feature)

- **Whenever a new feature is added, you MUST write a dedicated `.md` documentation file inside `docs/`** (for example: `docs/hot-reload.md`, `docs/plugins.md`, `docs/bundler.md`).
- Each feature doc should cover:
  1. Purpose and user-facing behavior.
  2. Public interface (Rust types/methods, JS `window.forge.*` surface, `forge.toml` keys, or CLI flags).
  3. Minimal working usage example.
  4. Limitations, platform notes, or security invariants.
- When a change alters the high-level architecture, workspace layout, or quick-start workflow, also update `docs/architecture.md` and `README.md`.

---

## 3. Codebase Design Standards

Follow **Deep Module** principles:
- **Keep the external interface small and zero-macro.** Prefer plain `async fn` handlers with sealed `FromContext` extractors (`State<T>`, `WindowHandle`, `AppConfig`) over procedural macros.
- **Avoid shallow crates.** Keep internal seams (`RuntimeHost`, `CefProvisioner`, `ProcessRunner`) private or cleanly injected rather than splitting out thin pass-through crates.
- **Accept dependencies at seams, don't bypass them.** External OS/CLI tools (`cargo`, `patchelf`, background dev servers) must go through `ProcessRunner` / `CefProvisioner` seams so automated tests can verify them without network downloads or display servers.
- **The interface is the test surface.** Write integration tests against `App::headless()` and `forge-cli` entry points asserting observable outcomes—do not reach into private internal endpoints.

---

## 4. Critical Runtime & Security Invariants

1. **CEF Multi-Process Subprocess Gate:** `App::run()` must run `runtime::enter_process()` *before* initializing Tokio or `.setup(...)` hooks so Chromium child processes (`--type=renderer`, `--type=gpu-process`, etc.) exit cleanly with zero side effects.
2. **Main-Thread Ownership:** Native GUI event loops (`tao` and `cef`) must execute on the OS main thread (`std::thread::current().name() == Some("main")`). Never require `#[tokio::main]` in user applications.
3. **IPC & Asset Security:**
   - All `/__forge_ipc/*` calls must enforce `POST`, `Sec-Fetch-Site`, and constant-time `X-Forge-Token` verification.
   - All disk reads/writes (`ForgeKernel` static assets and `AppFsPlugin`) must verify canonical ancestor containment inside the root directory *before* creating directories or reading/writing files, blocking `..` and symlink escapes.
   - Async command handlers must run inside panic containment (`catch_unwind`) so panics never unwind across C/C++ GTK or Chromium FFI stacks.

---

## 5. Verification Checklist Before Committing

Run all checks and confirm zero warnings and zero test failures before creating a commit:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p forge --features webview,cef,cef/dox,plugins,dev-tools
```

### Git Hygiene
- Never commit build outputs (`/target`, `/dist`), local CEF caches, or `.agents/` / `skills-lock.json`.
- Keep commits focused with clear conventional commit messages (`feat:`, `fix:`, `docs:`, `refactor:`).
