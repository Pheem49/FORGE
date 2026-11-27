# Releasing FORGE (`forge-app` & `forge-app-cli`)

FORGE ships as two coordinated crates from a single workspace:
1. **`forge-app`** (`[lib] name = "forge"`) — Core runtime, multi-window kernel, permission gate, and native plugins.
2. **`forge-app-cli`** (`[[bin]] name = "forge"`) — Project scaffolder, live dev runner, doctor, signer, bindings generator, and cross-OS bundler.

---

## 1. Pre-Release Verification Checklist

Run the full local suite and dry-run packaging checks before tagging a release:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features forge-app/plugins,forge-app/desktop-ui
cargo check -p forge-app --features webview,cef,cef/dox,plugins,dev-tools,desktop-ui

# Verify live window & IPC smoke tests on both engines:
FORGE_SMOKE_TEST=1 cargo run -p forge-app-cli -- dev --manifest-path examples/hello-forge/forge.toml --runtime webview
FORGE_SMOKE_TEST=1 cargo run -p forge-app-cli -- dev --manifest-path examples/hello-forge/forge.toml --runtime cef

# Verify crates.io tarballs:
cargo publish --dry-run -p forge-app
cargo package -p forge-app-cli --no-verify
```

---

## 2. Publishing Order on crates.io

Because `forge-app-cli` depends on `forge-app` (for the shared `forge::manifest` schema), publish **`forge-app` first**:

```bash
cargo publish -p forge-app
cargo publish -p forge-app-cli
```

Once published, anyone can install the `forge` CLI globally via:

```bash
cargo install forge-app-cli
```

---

## 3. Tagging & GitHub Release Automation

Pushing a version tag (`v0.4.0`) triggers [`.github/workflows/release.yml`](../.github/workflows/release.yml), which:
1. Runs `cargo publish --dry-run -p forge-app` and `cargo package --list -p forge-app-cli`.
2. Builds release artifacts natively across `ubuntu-24.04`, `windows-latest`, and `macos-latest`.
3. Generates `SHA256SUMS` and attaches installers (`.deb`, `.tar.gz`, `.exe` / `.zip`, `.dmg`) to the GitHub Release.

```bash
git tag -a v0.4.0 -m "FORGE v0.4.0 — Production Desktop Parity & Trusted Distribution"
git push origin main --tags
```
