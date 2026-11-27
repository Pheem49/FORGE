# Code Signing, Notarization & Ed25519 Auto-Updater

FORGE v0.4 integrates OS-level code signing (`codesign` + `notarytool` on macOS, `signtool` on Windows) with static, Ed25519-signed Over-The-Air (OTA) release updates (`latest.json` + `UpdaterPlugin`).

---

## 1. Purpose & User-Facing Behavior

- **OS Trust:** Sign `.app` / `.dmg` bundles (including inside-out signing for Chromium CEF frameworks and helper apps) and Windows `.exe` / NSIS installers during `forge build`.
- **Serverless OTA Updates:** Host static `latest.json` and installer files on GitHub Releases, S3, or Cloudflare R2; `UpdaterPlugin` verifies HTTPS transport, monotonic SemVer versions, SHA-256 digests, and Ed25519 signatures before applying updates.

---

## 2. Public Interface

### CLI (`forge signer` & `forge build`)

```bash
# 1. Generate a 32-byte Ed25519 keypair (0600 private key + base64 pubkey)
forge signer keygen --out ~/.forge/updater.key

# 2. Build signed OS installers and emit dist/latest.json
forge build --format all \
  --updater-key ~/.forge/updater.key \
  --updater-base-url https://github.com/org/my-app/releases/download/v0.4.0 \
  --release-notes "Bug fixes and performance improvements"
```

### `forge.toml`

```toml
[bundle.macos]
signing_identity = "Developer ID Application: Example Corp (TEAM123)"
entitlements = "macos/entitlements.plist"
notary_profile = "forge-notary"

[bundle.windows]
certificate_thumbprint = "0123456789ABCDEF0123456789ABCDEF01234567"
timestamp_url = "http://timestamp.digicert.com"

[updater]
endpoints = ["https://github.com/org/my-app/releases/latest/download/latest.json"]
pubkey = "BASE64_ED25519_PUBLIC_KEY"
```

Environment overrides for CI: `FORGE_UPDATER_PRIVATE_KEY`, `FORGE_UPDATER_KEY_PATH`, `FORGE_UPDATER_BASE_URL`, `APPLE_SIGNING_IDENTITY`, `APPLE_NOTARY_PROFILE`, `WINDOWS_CERTIFICATE_THUMBPRINT`.

### Runtime Plugin (`forge::plugins::UpdaterPlugin`)

- Cargo feature: `forge/plugin-updater` (included in `forge/plugins`).
- Commands (subject to `[permissions]`):
  - `updater.check` → `Option<AvailableUpdate>` (`current_version`, `version`, `notes`, `pub_date`, `platform`, `url`, `sha256`).
  - `updater.download_and_install` → `ApplyOutcome` (`installed_needs_restart`, `installer_launched`, or `staged`), emitting `updater:progress` (`{ downloaded_bytes, total_bytes }`).
- Seams for headless testing: `UpdaterPlugin::with_transport(MemoryUpdateTransport)` and `with_applier(...)`.

---

## 3. Minimal Example

**Rust (`src/main.rs`)**
```rust
use forge::App;
use forge::plugins::UpdaterPlugin;

fn main() -> forge::Result<()> {
    App::new().plugin(UpdaterPlugin::new()).run()
}
```

**JavaScript (`ui/main.js`)**
```js
const update = await window.forge.invoke("updater.check");
if (update) {
  window.forge.listen("updater:progress", ({ downloaded_bytes, total_bytes }) => {
    console.log(`Downloading ${update.version}: ${downloaded_bytes}/${total_bytes}`);
  });
  const outcome = await window.forge.invoke("updater.download_and_install");
  console.log("Update applied:", outcome.status);
}
```

---

## 4. Security Invariants & Platform Notes

- **Metadata-Bound Ed25519 Signatures:** `forge signer` signs the canonical message `v1\n<version>\n<platform>\n<url>\n<sha256_hex>` and verifies it with `VerifyingKey::verify_strict` alongside constant-time `SHA-256` payload verification. An attacker who compromises a static bucket cannot replay an older signed binary under a newer version number, swap download URLs, or cross-swap platform artifacts.
- **HTTPS-Only Enforcement:** Remote endpoints and artifact URLs must use `https://` (`http://127.0.0.1` / `http://localhost` is allowed only in debug builds or explicit test configurations).
- **macOS CEF Inside-Out Signing:** When signing `--runtime cef` macOS bundles, `CodeSigner` signs nested `Libraries/*.dylib` → helper `.app` bundles → `Chromium Embedded Framework.framework` → `Contents/MacOS/<bin>` → outer `<Title>.app` before notarization and stapling.
