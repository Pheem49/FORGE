# Native Desktop Capability Plugins (`forge::plugins`)

FORGE ships modular, capability-gated desktop plugins under `forge::plugins`. Register all enabled plugins at once with `StandardPlugins`, or attach individual plugins with `App::plugin(...)`.

---

## 1. Purpose & Plugin Catalog

| Plugin | Cargo Feature | IPC Namespace | Description |
| :--- | :--- | :--- | :--- |
| `DialogPlugin` | `plugin-dialog` | `dialog.*` | Native OS file open/save pickers and message dialogs (`rfd`). |
| `ClipboardPlugin` | `plugin-clipboard` | `clipboard.*` | Read and write OS clipboard text (`arboard`). |
| `NotificationPlugin` | `plugin-notification` | `notification.*` | Desktop toast notifications (`notify-rust`). |
| `AppFsPlugin` | `plugin-fs` | `fs.*` | Sandboxed text & raw binary storage inside `<data_dir>/<identifier>/`. |
| `OpenerPlugin` | `plugin-opener` | `opener.*` | Open validated external `https://`, `http://`, or `mailto:` URLs in the OS default handler. |
| `GlobalShortcutPlugin` | `plugin-shortcut` | `shortcut.*` | System-wide keyboard hotkeys emitting `shortcut:triggered`. |
| `DeepLinkPlugin` | `plugin-deep-link` | `deep_link.*` | Custom `<scheme>://` URI handling across startup args and `SingleInstanceGuard`. |
| `UpdaterPlugin` | `plugin-updater` | `updater.*` | Ed25519-verified static OTA updates (see [Signing & Updater](signing-and-updater.md)). |

---

## 2. Public Interface

### `OpenerPlugin` (`opener.open_url`)
- `OpenerPlugin::new()` — Allows `["https", "http", "mailto"]` by default; customize via `.with_allowed_schemes(...)` or inject a mock `RecordedUrlOpener` via `.with_opener(...)`.
- JS: `await window.forge.invoke("opener.open_url", { url: "https://github.com/Pheem49/FORGE" })`

### `GlobalShortcutPlugin` (`shortcut.*`)
- `GlobalShortcutPlugin::new().with_shortcut("CmdOrCtrl+Shift+K").on_shortcut(|app, accel| { ... })`
- Commands: `shortcut.register` (`{ accelerator }`), `shortcut.unregister` (`{ accelerator }`), `shortcut.unregister_all`, `shortcut.list`.
- Push Event: `shortcut:triggered` (`{ accelerator }`), testable headlessly via `HeadlessApp::trigger_shortcut("CmdOrCtrl+Shift+K")`.

### `DeepLinkPlugin` (`deep_link.*` & `[app].schemes`)
- Declare custom URI schemes in `forge.toml`:
  ```toml
  [app]
  single_instance = true
  schemes = ["forge-notes"]
  ```
- `forge build` automatically registers `CFBundleURLTypes` in macOS `Info.plist`, `MimeType=x-scheme-handler/forge-notes;` + `%u` in Linux `.deb`/`.AppImage` `.desktop` entries, and `HKCU\Software\Classes\forge-notes` in Windows NSIS installers.
- Commands: `deep_link.get_current` (`string[]`).
- Push Event: `deep-link:open` (`{ urls: string[] }`), delivered on cold launch, macOS native `Event::Opened`, and when a second instance forwards CLI arguments via `SingleInstanceGuard`.

---

## 3. Minimal Example

```rust
use forge::App;
use forge::plugins::{DeepLinkPlugin, GlobalShortcutPlugin, OpenerPlugin};

fn main() -> forge::Result<()> {
    App::new()
        .plugin(OpenerPlugin::new())
        .plugin(DeepLinkPlugin::new())
        .plugin(GlobalShortcutPlugin::new().with_shortcut("CmdOrCtrl+Shift+Space"))
        .run()
}
```

```js
window.forge.listen("deep-link:open", ({ urls }) => {
  console.log("Opened via URI:", urls);
});

window.forge.listen("shortcut:triggered", ({ accelerator }) => {
  console.log("Global hotkey pressed:", accelerator);
});
```

---

## 4. Limitations & Security Invariants

- **Default-Deny Permissions:** Registering a plugin does **not** expose its commands to JavaScript until explicitly listed in `[permissions].allow` (for example, `allow = ["opener.open_url", "deep_link.*", "shortcut.*"]`).
- **`OpenerPlugin` Scheme & Argument Hardening:** `javascript:`, `vbscript:`, `data:`, `blob:`, `file:`, whitespace/control characters, and leading `-` flags are strictly rejected before spawning `xdg-open`, `open`, or `rundll32`.
- **Custom Scheme Validation:** `[app].schemes` enforces lowercase RFC 3986 scheme syntax and rejects reserved schemes (`http`, `https`, `file`, `javascript`, `forge`, `ws`, `wss`, etc.) at manifest load time.
- **Global Shortcuts Platform Scope:** `global-hotkey` supports Windows, macOS, and Linux X11 on `runtime = "webview"`, and returns a typed `501 FORGE_UNSUPPORTED` error on `runtime = "cef"`.
