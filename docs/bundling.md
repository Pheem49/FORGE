# Cross-OS Application Bundler (`forge build`)

`forge build` compiles a release executable with single-file embedded UI assets, generates multi-OS icons, packages OS-native installers through the `BundleTarget` seam, and writes deterministic `SHA256SUMS` checksums into `dist/`.

---

## 1. Supported Bundle Formats

```bash
forge build --format all|bin|tarball|deb|appimage|nsis|zip|app|dmg
```

| Format | Host OS | External Tool | Output Artifact |
| :--- | :--- | :--- | :--- |
| `bin` | Any | — | `dist/<name>/<bin>` (+ staged CEF runtime files) |
| `tarball` | Linux / macOS / Any | — | `dist/<name>-<version>-<os>-<arch>.tar.gz` |
| `deb` | Linux | `dpkg-deb` | `dist/<name>_<version>_<arch>.deb` |
| `appimage` | Linux | `appimagetool` | `dist/<name>-<version>-<arch>.AppImage` |
| `nsis` | Windows | `makensis` | `dist/<name>-<version>-<arch>-setup.exe` |
| `zip` | Windows / Any | — | `dist/<name>-<version>-<os>-<arch>.zip` |
| `app` | macOS | `bundle-cef-app` (optional, CEF) | `dist/<Title>.app` |
| `dmg` | macOS | `hdiutil` | `dist/<name>-<version>-<arch>.dmg` |

`--format all` builds every format native to the current host OS whose external CLI tool is available on `PATH`. Requesting an incompatible host format (for example, `--format dmg` on Linux) fails fast with a descriptive error naming the current OS.

---

## 2. `forge.toml` Bundle & Icon Metadata

```toml
[app]
name = "forge-notes"
version = "0.3.0"
identifier = "dev.forge.notes"
icon = "icons/icon.png" # Single master PNG or SVG

[bundle]
publisher = "FORGE Labs"
category = "Productivity"
copyright = "Copyright (c) 2026 FORGE Labs"
homepage = "https://github.com/Pheem49/FORGE"
```

### Icon Pipeline
During staging, `IconSet::generate` converts `[app].icon` (or FORGE's built-in badge when omitted) into:
- `icon.svg` and `128x128` / `256x256` / `512x512` PNGs (Linux `.deb` & `.AppImage`)
- Multi-resolution Windows `.ico` (`32x32` + `256x256` PNG frames)
- Apple `.icns` (`ic07`, `ic08`, `ic09` chunks for macOS `.app` & `.dmg`)

---

## 3. Minimal Example

```bash
# Build native WebView bundles
forge build --runtime webview --format all

# Build self-contained Chromium AppImage + .deb + tarball
forge build --runtime cef --format all
```

---

## 4. Platform & Runtime Notes

- **Linux AppImage (`webview` vs `cef`):** A `--runtime cef` AppImage bundles `libcef.so` and `AppRun` `LD_LIBRARY_PATH` self-contained. A `--runtime webview` AppImage remains lightweight (~4 MB) and relies on the host distribution's installed `libwebkit2gtk-4.1`; `.deb` is recommended when distributing `webview` builds on Debian/Ubuntu so `apt` resolves `libwebkit2gtk-4.1-0` automatically.
- **Windows NSIS (`webview` vs `cef`):** WebView installers check the Windows Registry for the Microsoft Edge WebView2 Evergreen Runtime during installation; CEF installers bundle Chromium DLLs inside `$INSTDIR` with zero host prerequisites. When `[bundle.windows].certificate_thumbprint` (or `WINDOWS_CERTIFICATE_THUMBPRINT`) is configured, `forge build` signs both the inner `.exe` and outer `*-setup.exe` with `signtool`.
- **macOS Signing & Notarization:** Configure `[bundle.macos]` (`signing_identity`, `entitlements`, `notary_profile`) or `APPLE_SIGNING_IDENTITY` / `APPLE_NOTARY_PROFILE` to sign `.app` bundles inside-out (`dylib` → CEF helpers → framework → executable → `.app`) and notarize/staple `.dmg` images via `xcrun notarytool` + `stapler` (see [Code Signing & Updater](signing-and-updater.md)).
- **Deep-Link Registration & Signed OTA Manifest:** Setting `[app].schemes = ["myapp"]` registers custom URI handlers across `.app`, `.deb`, `.AppImage`, and `NSIS`. Passing `--updater-key <key>` emits a cryptographically signed `dist/latest.json` manifest alongside `SHA256SUMS`.
