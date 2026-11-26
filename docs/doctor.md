# Environment Diagnostics (`forge doctor`)

`forge doctor` verifies that your machine has the compilers, OS WebView libraries, Chromium CEF build tools, Node.js toolchain, and native packagers required for FORGE development and bundling.

---

## 1. CLI Interface

```bash
# Check environment against `./forge.toml` (if present) or defaults
forge doctor

# Check strictly for WebView or Chromium CEF workflows
forge doctor --runtime webview
forge doctor --runtime cef
```

---

## 2. What `forge doctor` Inspects

| Check Group | Tools / Libraries | Required When |
| :--- | :--- | :--- |
| **Rust Toolchain** | `rustc`, `cargo` | Always |
| **Linux WebView** | `webkit2gtk-4.1`, `gtk+-3.0` (`pkg-config`) | Linux + `webview` runtime |
| **Linux Desktop UI** | `ayatana-appindicator3-0.1` (`pkg-config`) | Optional (`desktop-ui` feature) |
| **Chromium CEF** | `cmake`, `ninja`, `clang`, `~/.cache/forge/cef` | `--runtime cef` (optional otherwise) |
| **Frontend HMR** | `node`, `npm` | `[dev]` commands configured in `forge.toml` |
| **OS Bundlers** | `dpkg-deb`, `patchelf`, `appimagetool`, `makensis`, `hdiutil` | Optional (per target format) |

---

## 3. Exit Codes & Output

- Prints `✔` for satisfied checks, `!` for optional tools not currently installed, and `✖` with the exact `apt` / `winget` / URL install command when a required dependency is missing.
- Returns exit code `0` when all required checks pass, or `1` when at least one required check fails (ideal as a CI preflight step).
