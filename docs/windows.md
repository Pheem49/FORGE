# Multi-Window Management, Lifecycle & Window Controls

FORGE v0.4 supports multiple native windows, declarative/imperative close interception, `always_on_top`, and unified frameless drag regions across both `webview` (`tao` + `wry`) and `cef` (Chromium Views API), coordinated through a label-keyed registry with zero procedural macros.

---

## 1. Purpose & User-Facing Behavior

Every FORGE application starts with a primary `"main"` window configured by `[window]`, and can declare additional windows in `[windows.<label>]` or spawn windows dynamically at runtime from Rust (`AppHandle::create_window`) or JavaScript (`forge.window.create`).

By default (`[app] exit_on_last_window_close = true`), the application stays running until the last open window closes.

---

## 2. Public Interface

### `forge.toml`

```toml
[app]
exit_on_last_window_close = true

[window] # label = "main"
url = "/index.html"
title = "Notes"
width = 1024
height = 768
always_on_top = false
on_close = "emit" # "close" (default) | "hide" | "emit"

[windows.settings]
url = "/settings.html"
title = "Preferences"
width = 520
height = 420
create_on_startup = false
```

### Rust Extractors (`AppHandle`, `WindowHandle`, `WindowOptions`)

- **`WindowHandle`** — Injected into command handlers as the **calling window** (`win.label()`, `set_title`, `set_size`, `set_position`, `set_minimized`, `set_maximized`, `set_fullscreen`, `set_always_on_top`, `set_on_close`, `show`, `hide`, `focus`, `drag_window`, `close_window`, `destroy`, `emit`).
- **`AppHandle`** — Application-wide capability extractor (`create_window`, `window(label)`, `windows()`, `emit_all`, `emit_to`, `quit`).

### Close Interception (`on_close`) & Custom Titlebar Drag Regions

- `on_close = "close"` *(default)*: Destroys the window immediately when `[X]` or `window.close()` is invoked.
- `on_close = "hide"`: Cancels destruction and hides the window (ideal for background tray apps).
- `on_close = "emit"`: Cancels destruction and emits `window:close-requested` (`{ label }`) so your UI can prompt to save unsaved work before calling `await window.forge.window.destroy()`.
- **Unified Frameless Dragging (`data-forge-drag-region`):** Mark any custom header element with `<header data-forge-drag-region>` (and interactive child controls with `data-forge-no-drag`). On `webview`, FORGE delegates primary mouse-down to `startDragging()`; on `cef`, FORGE wires CSS `-webkit-app-region: drag` into `CefDragHandler::on_draggable_regions_changed` → `Window::set_draggable_regions`.

### JavaScript Bridge (`window.forge.window` & Inter-Window Events)

```js
console.log(window.forge.window.label); // e.g. "main" or "settings"
await window.forge.window.setAlwaysOnTop(true);
window.forge.listen("window:close-requested", async () => {
  if (confirm("Discard unsaved changes?")) {
    await window.forge.window.destroy();
  }
});
await window.forge.window.create({ label: "settings" });
await window.forge.emitTo("settings", "prefs.load", { section: "theme" });
await window.forge.emit("theme.changed", { dark: true });
```

Built-in IPC commands (subject to `[permissions]`):
`window.close`, `window.destroy`, `window.minimize`, `window.maximize`, `window.toggle_fullscreen`, `window.set_always_on_top`, `window.set_on_close`, `window.set_title`, `window.show`, `window.hide`, `window.focus`, `window.start_dragging`, `window.create`, `window.list`, `event.emit_all`, `event.emit_to`.

---

## 3. Minimal Example

```rust
use forge::{App, AppHandle, ForgeError, WindowHandle, WindowOptions};

async fn open_about(caller: WindowHandle, app: AppHandle) -> Result<String, ForgeError> {
    let win = app.create_window(
        "about",
        WindowOptions::new("/about.html")
            .title(format!("About (opened from {})", caller.label()))
            .size(420, 300),
    )?;
    Ok(win.label().to_string())
}
```

---

## 4. Platform & Engine Notes

- **CEF Views API:** On `--runtime cef`, FORGE uses `cef::window_create_top_level` + `cef::browser_view_create` so `maximize`, `minimize`, `restore`, `set_fullscreen`, `set_title`, `set_size`, `set_position`, `show`, `hide`, and multi-window work natively across platforms.
- **Window Identification:** Each WebView protocol closure and CEF `Browser::identifier()` stamps the originating window label directly onto `ProtocolRequest` on the Rust side—pages cannot spoof another window's identity.
