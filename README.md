# FORGE

**Write once. Choose your runtime.**

FORGE is a Rust-first desktop application platform that lets you write a single Rust + HTML/CSS/JS codebase and run it seamlessly on either the lightweight **OS System WebView** (`tao` + `wry`) or **Chromium** (`cef-rs`) with **zero code changes**.

---

## Architecture (`codebase-design`)

FORGE v0.1 is structured around two deep modules:

```text
FORGE/
├── crates/
│   ├── forge/        # Deep application module: config, State<T>, async IPC, and runtime adapters
│   └── forge-cli/    # Developer CLI (`forge new`, `forge dev`, automated CEF cache provisioning)
├── templates/
│   └── vanilla/      # Starter HTML/CSS/JS + Rust project template
└── examples/
    └── hello-forge/  # Interactive Runtime & IPC Workbench example
```

### Key Technical Highlights
- **Zero-Macro Rust Handlers**: Plain `async fn` handlers with sealed `FromContext` extractors (`State<T>`, `WindowHandle`, `AppConfig`) and startup `TypeId` verification.
- **Unified Custom-Scheme Exchange (`forge://app/...`)**: Both static UI assets (`GET /index.html`) and async RPC calls (`POST /__forge_ipc/<cmd>`) flow through a single custom-scheme protocol with per-launch 256-bit `X-Forge-Token` CSRF protection—eliminating custom C++ V8 FFI glue inside Chromium renderer subprocesses.
- **Transparent Multi-Process Subprocess Gate**: `App::run()` intercepts Chromium `--type=renderer` and `--type=gpu-process` child executions automatically before starting Tokio or running `.setup(...)` hooks.
- **Sub-5ms Headless Verification**: `App::headless()` exercises the complete asset + security + async RPC pipeline in `cargo test` without requiring X11, Wayland, or Chromium binaries.

---

## Quick Start

### 1. Run the Reference Workbench

Run on the OS System WebView (`WebKitGTK` / `WebView2` / `WKWebView`):

```bash
cargo run -p forge-cli -- dev --manifest-path examples/hello-forge/forge.toml --runtime webview
```

Switch the **exact same app** to Chromium (`CEF`):

```bash
cargo run -p forge-cli -- dev --manifest-path examples/hello-forge/forge.toml --runtime cef
```

*(On first run, `--runtime cef` automatically downloads and caches the pre-built CEF runtime into `~/.cache/forge/cef`.)*

### 2. Scaffold a New Project

```bash
cargo run -p forge-cli -- new my-app
cargo run -p forge-cli -- dev --manifest-path my-app/forge.toml
```

### 3. Run the Automated Test Suite

```bash
cargo test --workspace
```
