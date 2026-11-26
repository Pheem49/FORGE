# System Tray & Native Application Menu (`desktop-ui`)

FORGE provides declarative `forge.toml` and runtime Rust APIs for OS system tray icons and native application menu bars via the opt-in `desktop-ui` Cargo feature (`tray-icon` + `muda`).

---

## 1. Purpose & User-Facing Behavior

- Render native menu bars (macOS global menu bar, Windows Win32 menu bar, Linux GTK3 menu bar) and OS tray icons with context menus.
- Built-in `forge.*` item IDs (`forge.quit`, `forge.show-main`, `forge.hide-main`, `forge.fullscreen`) are handled automatically by FORGE; custom item IDs broadcast `menu:click` or `tray:click` push events (`{ id }`) to open windows.

---

## 2. Public Interface

### Cargo Feature

```toml
[features]
webview = ["forge/webview", "forge/desktop-ui"]
```

### `forge.toml`

```toml
[tray]
icon = "icons/tray.png"
tooltip = "FORGE Notes"
menu = [
  { id = "forge.show-main", label = "Show Window" },
  { separator = true },
  { id = "forge.quit", label = "Quit" }
]

[menu]
[[menu.submenu]]
label = "File"
items = [
  { id = "file.new", label = "New Note", accelerator = "CmdOrCtrl+N" },
  { separator = true },
  { id = "forge.quit", label = "Quit", accelerator = "CmdOrCtrl+Q" }
]
```

### Rust & Headless Testing Interface

- `AppHandle::set_tray(TrayConfig) -> Result<(), ForgeError>`
- `AppHandle::set_menu(MenuConfig) -> Result<(), ForgeError>`
- `HeadlessApp::tray()`, `HeadlessApp::menu()`, `HeadlessApp::click_menu(id)`, `HeadlessApp::click_tray(id)`

---

## 3. Minimal Example

```js
window.forge.listen("menu:click", ({ id }) => {
  if (id === "file.new") {
    createBlankDocument();
  }
});
```

---

## 4. Limitations & Platform Notes

- **Linux System Package:** Building and running `desktop-ui` on Linux requires `libgtk-3-dev` and `libayatana-appindicator3-dev` (verified automatically by `forge doctor`).
- **CEF Runtime Behavior:** Because `tray-icon` and `muda` require a GTK main loop on Linux while `CefHost` runs Chromium's Views message pump, `AppHandle::set_tray` and `AppHandle::set_menu` return a typed `FORGE_UNSUPPORTED` (`501`) error on `cef` rather than silently no-oping.
