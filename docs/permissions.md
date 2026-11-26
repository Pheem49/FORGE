# Capability Permission System (`[permissions]`)

Starting in **FORGE v0.3**, IPC command permissions are **strictly enforced by default** (`mode = "enforce"`). Every command invoked over `/__forge_ipc/*`—including user handlers, standard plugins, and built-in `window.*` / `event.*` commands—must be permitted by `forge.toml`.

---

## 1. Purpose & Security Model

Without capability rules, any XSS or untrusted script inside a WebView or Chromium frame can invoke every registered backend command. FORGE enforces a declarative, single-choke-point policy in `kernel/ipc.rs`:

- **Default-Deny:** When `[permissions]` is omitted from `forge.toml`, FORGE defaults to `mode = "enforce"` with `allow = []`. Unlisted commands fail with HTTP `403` and `code = "FORGE_PERMISSION_DENIED"`.
- **Deny Always Wins:** Any command matching `deny` is blocked even if matched by `"*"` or `"namespace.*"`.
- **Per-Window Isolation:** Secondary windows (such as an untrusted preview or settings dialog) can declare a restricted `[permissions.windows.<label>]` allow-list that **replaces** the global `allow` list for that window.
- **Startup Typo Guard:** In `enforce` mode, any non-`"*"` pattern that matches zero registered commands aborts startup immediately with `Error::Config` so typos like `"fs.red_text"` never fail silently in production.

---

## 2. Public Interface

### `forge.toml`

```toml
[permissions]
mode = "enforce" # "enforce" (default) | "permissive"
allow = [
  "greet",
  "dialog.*",
  "fs.read_text",
  "fs.write_text",
  "window.*",
  "event.*",
]
deny = ["fs.remove_file"]

[permissions.windows.settings]
allow = ["greet", "window.close"]
```

### One-Command Migration (`forge permissions init`)

Generate or refresh `[permissions]` automatically from your compiled `App` command registrations:

```bash
forge permissions init
```

Under the hood, `forge permissions init` runs your binary with `FORGE_EXPORT_COMMANDS=<path>`, collects every registered user/plugin command without opening a window, and writes a clean `mode = "enforce"` allow-list into `forge.toml`.

### Rust Test Helpers

```rust
let app = App::new()
    .configure(|cfg| {
        cfg.permissions.allow_all(); // Sets `allow = ["*"]` for compact unit tests
    })
    .command("ping", ping)
    .headless()?;
```

---

## 3. Rejection Payload Format

When JavaScript invokes a disallowed command, `window.forge.invoke(...)` rejects with:

```json
{
  "code": "FORGE_PERMISSION_DENIED",
  "message": "window 'settings' is not permitted to invoke command 'fs.remove_file'",
  "details": {
    "command": "fs.remove_file",
    "window": "settings",
    "hint": "command is explicitly blocked by [permissions].deny"
  }
}
```

---

## 4. Invariants

- Internal bootstrap handshake routes (`__forge_drain_events`, `__forge_ready`) bypass `[permissions]`; user registration of `__forge*` names is rejected at startup (`Error::ReservedCommand`).
- Switching to `mode = "permissive"` is intended strictly for rapid local prototyping; `deny` rules remain active even in `permissive` mode.
