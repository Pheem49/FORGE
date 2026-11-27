# `forge.toml` Manifest Reference

Both the `forge-app` runtime crate (`forge::Manifest`) and `forge-cli` share a single declarative schema for `forge.toml`.

---

## Complete Example

```toml
[app]
name = "forge-notes"
version = "0.4.0"
description = "Fast cross-runtime desktop notes"
identifier = "dev.forge.notes"
assets = "dist-ui"
icon = "icons/icon.png"
single_instance = true
exit_on_last_window_close = true
schemes = ["forge-notes"]

[window] # Primary "main" window
url = "/index.html"
title = "FORGE Notes"
width = 1100
height = 740
min_width = 720
min_height = 480
resizable = true
decorations = true
visible = true
always_on_top = false
on_close = "emit" # "close" (default) | "hide" | "emit"
devtools = true
remember_state = true
create_on_startup = true

[windows.settings] # Additional named window
url = "/settings.html"
title = "Preferences"
width = 520
height = 420
create_on_startup = false

[runtime]
default = "webview" # "webview" | "cef"

[dev]
url = "http://127.0.0.1:5173"
before_dev_command = "npm run dev"
before_build_command = "npm run build"

[permissions]
mode = "enforce" # "enforce" (default) | "permissive"
allow = ["greet", "notes.*", "dialog.*", "fs.*", "window.*", "event.*"]
deny = ["fs.remove_file"]

[permissions.windows.settings]
allow = ["greet", "window.close"]

[tray]
icon = "icons/tray.png"
tooltip = "FORGE Notes"
menu = [
  { id = "forge.show-main", label = "Show Notes" },
  { separator = true },
  { id = "forge.quit", label = "Quit" },
]

[menu]
[[menu.submenu]]
label = "File"
items = [
  { id = "file.new", label = "New Note", accelerator = "CmdOrCtrl+N" },
  { separator = true },
  { id = "forge.quit", label = "Quit", accelerator = "CmdOrCtrl+Q" },
]

[bundle]
publisher = "FORGE Labs"
category = "Productivity"
copyright = "Copyright (c) 2026 FORGE Labs"
homepage = "https://github.com/Pheem49/FORGE"

[bundle.macos]
signing_identity = "Developer ID Application: FORGE Labs (TEAM123)"
entitlements = "macos/entitlements.plist"
notary_profile = "forge-notary"

[bundle.windows]
certificate_thumbprint = "0123456789ABCDEF0123456789ABCDEF01234567"
timestamp_url = "http://timestamp.digicert.com"

[updater]
endpoints = ["https://github.com/Pheem49/FORGE/releases/latest/download/latest.json"]
pubkey = "BASE64_ED25519_PUBLIC_KEY"
```

---

## Environment Variable Overrides

| Variable | Effect |
| :--- | :--- |
| `FORGE_MANIFEST_DIR` | Directory containing `forge.toml` (defaults to `.`). |
| `FORGE_RUNTIME` | Overrides `[runtime].default` (`webview` or `cef`). |
| `FORGE_ASSETS_DIR` | Overrides `[app].assets`. |
| `FORGE_DEV_URL` | Overrides `[dev].url` and enables the HTTP dev proxy. |
| `FORGE_DEV_WATCH=1` | Enables local static file live reload / CSS hot swap. |
| `FORGE_EXPORT_COMMANDS=<path>` | Writes registered command metadata JSON and exits `0` before opening a window. |
| `FORGE_SMOKE_TEST=1` | Verifies full window + `DOMContentLoaded` + IPC round-trip and exits `0`. |
