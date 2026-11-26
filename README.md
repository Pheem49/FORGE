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

* **Zero Macros, Plain Rust:** Write standard `async fn` handlers with `State<T>`, `WindowHandle`, `AppHandle`, and `AppConfig` extractors—fast compile times and full IDE autocomplete.
* **Safe by Default (`[permissions]`):** Strict capability allow-list (`mode = "enforce"`), Same-Origin `forge://localhost` protocol, 256-bit `X-Forge-Token` CSRF protection, and canonical symlink-safe sandboxing.
* **Sub-5ms Headless + Live Smoke Testing:** Verify 95%+ of your app in milliseconds via `App::headless()`, plus prove real native window & IPC execution in CI with `FORGE_SMOKE_TEST=1`.
* **Zero-Boilerplate Cross-OS Bundler:** `forge build` generates icons (`.png`, `.ico`, `.icns`), embeds UI assets into a single executable, produces `.deb`, `.AppImage`, `.tar.gz`, NSIS `.exe`, `.zip`, `.app`, and `.dmg`, and writes `SHA256SUMS`.

---

## What It Can Do

* **Dual Desktop Runtimes:** Switch between `webview` and `cef` via a single CLI flag (with automated `~/.cache/forge/cef` binary provisioning).
* **Multi-Window, System Tray & Menus:** Declarative `[windows.<label>]`, `[tray]`, `[menu]`, single-instance lock (`[app] single_instance = true`), and persistent window geometry (`remember_state = true`).
* **Starter Templates & Typed DX:** `forge new` for `vanilla`, `react`, `vue`, and `svelte`, `forge bindings` for TypeScript `.d.ts` command generation, `forge permissions init`, and `forge doctor`.
* **Native Desktop Plugins (`forge::plugins`):** File Dialogs, Clipboard, OS Notifications, and sandboxed App Storage.

---

## How to Use

### 1. Check Environment & Create a Project

```bash
# Verify compilers, WebView/CEF libraries, and packagers
cargo run -p forge-cli -- doctor

# Scaffold Vanilla, React, Vue 3, or Svelte 5
cargo run -p forge-cli -- new my-app --template react
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
cargo run -p forge-cli -- dev --manifest-path examples/hello-forge/forge.toml --runtime webview
cargo run -p forge-cli -- dev --manifest-path examples/hello-forge/forge.toml --runtime cef

# Build release installers + SHA256SUMS into dist/
cargo run -p forge-cli -- build --manifest-path examples/hello-forge/forge.toml --format all
```

---

## Documentation

* [Release Notes](RELEASE_NOTES.md) · [Architecture Guide](docs/architecture.md) · [`forge.toml` Reference](docs/forge-toml.md)
* [Multi-Window & Controls](docs/windows.md) · [System Tray & Menus](docs/tray-and-menu.md) · [Single-Instance Lock](docs/single-instance.md) · [Window State](docs/window-state.md)
* [Capability Permissions](docs/permissions.md) · [Cross-OS Bundling](docs/bundling.md) · [Chromium (`cef`) Runtime](docs/cef.md)
* [Typed TS Bindings](docs/typed-bindings.md) · [`forge doctor`](docs/doctor.md) · [Testing & Smoke Mode](docs/testing.md) · [Releasing](docs/releasing.md)
