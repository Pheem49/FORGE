# FORGE

**Write once. Choose your runtime. Ship anywhere.**

![FORGE Workbench](docs/assets/forge-workbench.png)

---

## What is FORGE?

**FORGE** is a Rust-first desktop application platform and builder. Build cross-platform desktop apps using **Rust** for application logic and **Vanilla JS**, **React**, **Vue 3**, or **Svelte 5** for the UI—then switch between the lightweight **OS System WebView** (`WRY`) and **Chromium 156** (`CEF`) without changing a single line of application code.

---

## Why FORGE?

| | System WebView (`--runtime webview`) | Chromium (`--runtime cef`) |
| :--- | :--- | :--- |
| **Engine** | WebKitGTK / WebView2 / WKWebView | Embedded Chromium 156 (CEF Views API) |
| **Rendering** | Native OS WebView | Identical Blink + V8 on Linux, Windows & macOS |
| **Bundle Size** | ~2–6 MB standalone / `.deb` / NSIS | Self-contained Chromium bundle / AppImage |
| **Code Changes** | None | None |

* **Zero Macros, JSON & Raw Binary IPC:** Write standard `async fn` handlers with `State<T>`, `Meta<T>`, `RawBytes`, `WindowHandle`, `AppHandle`, and `AppConfig` extractors—fast compile times, full IDE autocomplete, and zero-overhead `Uint8Array` ↔ `RawBytes` streaming.
* **Safe by Default (`[permissions]`):** Strict capability allow-list (`mode = "enforce"`), Same-Origin `forge://localhost` protocol, 256-bit `X-Forge-Token` CSRF protection, and canonical symlink-safe sandboxing.
* **Sub-5ms Headless + Live Smoke Testing:** Verify 95%+ of your app in milliseconds via `App::headless()`, plus prove real native window & IPC execution in CI with `FORGE_SMOKE_TEST=1`.
* **Trusted Cross-OS Bundler & OTA Updater:** `forge build` generates icons, embeds UI assets, packages `.deb`, `.AppImage`, `.tar.gz`, NSIS `.exe`, `.zip`, `.app`, and `.dmg`, code-signs & notarizes macOS/Windows builds, and emits Ed25519-signed `latest.json` manifests for `UpdaterPlugin`.

---

## What It Can Do

* **Dual Desktop Runtimes:** Switch between `webview` and `cef` via a single CLI flag (with automated `~/.cache/forge/cef` binary provisioning and unified `[data-forge-drag-region]` custom titlebars).
* **Multi-Window, Lifecycle, System Tray & Menus:** Declarative `[windows.<label>]`, close interception (`on_close = "close" | "hide" | "emit"`), `always_on_top`, `[tray]`, `[menu]`, single-instance lock (`[app] single_instance = true`), and persistent window geometry (`remember_state = true`).
* **Starter Templates & Typed DX:** `forge new` for `vanilla`, `react`, `vue`, and `svelte`, `forge bindings` for TypeScript `.d.ts` command generation, `forge signer keygen`, `forge permissions init`, and `forge doctor`.
* **Native Desktop Plugins (`forge::plugins`):** File Dialogs, Clipboard, OS Notifications, Sandboxed Text/Binary App Storage, Safe URL Opener, Global Shortcuts, Custom URI Deep Links (`myapp://`), and Signed Auto-Updater.

---

## Installation

### 1. System Prerequisites

* **All Platforms:** [Rust 1.92+](https://rustup.rs/) (`rustc` & `cargo`). [Node.js 18+](https://nodejs.org/) is only needed when using the `react`, `vue`, or `svelte` templates.
* **Linux (Ubuntu / Debian / Pop!\_OS):**
  ```bash
  sudo apt update && sudo apt install -y \
    libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
    build-essential pkg-config patchelf
  ```
* **Windows:** [Visual Studio C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (WebView2 is included with Windows 10/11; [NSIS](https://nsis.sourceforge.io/) is optional for `.exe` installers).
* **macOS:** Xcode Command Line Tools (`xcode-select --install`).

### 2. Install the `forge` CLI

Choose any of the following installation methods (all install the `forge` binary into `~/.cargo/bin`):

**Option A — From crates.io (Recommended):**
```bash
cargo install forge-app-cli
```

**Option B — Directly from GitHub (latest `main`):**
```bash
cargo install --git https://github.com/Pheem49/FORGE forge-app-cli
```

**Option C — From Local Source:**
```bash
git clone https://github.com/Pheem49/FORGE.git
cd FORGE
cargo install --path crates/forge-cli
```

Verify your environment at any time with:
```bash
forge doctor
```

### 3. Add Library to an Existing Cargo Project (Optional)

If you prefer wiring FORGE manually without `forge new`, add `forge-app` to your `Cargo.toml`:

```toml
[dependencies]
forge = { package = "forge-app", version = "0.4", features = ["plugins", "desktop-ui"] }
```

---

## Quick Start

### 1. Scaffold a New Project

```bash
# Templates: vanilla | react | vue | svelte
forge new my-app --template react
cd my-app
npm install
```

### 2. Write Rust Commands & Declare Permissions

**`src/main.rs`**
```rust
use forge::{App, ForgeError};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct GreetReq { name: String }

#[derive(Serialize)]
struct GreetRes { message: String }

async fn greet(req: GreetReq) -> Result<GreetRes, ForgeError> {
    Ok(GreetRes {
        message: format!("Hello, {}!", req.name),
    })
}

fn main() -> forge::Result<()> {
    App::new().command("greet", greet).run()
}
```

**`forge.toml`**
```toml
[app]
name = "my-app"
identifier = "dev.forge.myapp"

[permissions]
mode = "enforce"
allow = ["greet", "window.*", "event.*"]
```

**`src-ui/App.tsx`**
```ts
const res = await window.forge.invoke<{ message: string }>("greet", { name: "Pheem" });
```

### 3. Run & Package

```bash
# Live dev mode (OS System WebView or Chromium CEF)
forge dev --runtime webview
forge dev --runtime cef

# Build release installers + SHA256SUMS into dist/
forge build --format all
```

---

## Documentation

* [Release Notes](RELEASE_NOTES.md) · [Architecture Guide](docs/architecture.md) · [`forge.toml` Reference](docs/forge-toml.md)
* [Multi-Window & Lifecycle](docs/windows.md) · [Raw Binary IPC](docs/binary-ipc.md) · [Native Plugins](docs/plugins.md) · [Code Signing & Updater](docs/signing-and-updater.md)
* [System Tray & Menus](docs/tray-and-menu.md) · [Single-Instance Lock](docs/single-instance.md) · [Window State](docs/window-state.md)
* [Capability Permissions](docs/permissions.md) · [Cross-OS Bundling](docs/bundling.md) · [Chromium (`cef`) Runtime](docs/cef.md)
* [Typed TS Bindings](docs/typed-bindings.md) · [`forge doctor`](docs/doctor.md) · [Testing & Smoke Mode](docs/testing.md) · [Releasing](docs/releasing.md)
