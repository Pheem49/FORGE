# Single-Instance Application Lock

FORGE supports single-instance enforcement (`[app] single_instance = true`) so launching a second copy of an installed application focuses the running window and forwards CLI arguments to it instead of opening a duplicate process.

---

## 1. Purpose & User-Facing Behavior

1. The primary process acquires an OS kernel advisory file lock (`fd-lock`) and binds an authenticated `127.0.0.1` loopback listener.
2. When a user launches the binary a second time (for example, double-clicking a file or running `my-app report.md`), the second process forwards `{ args, cwd }` over loopback and exits `0` before creating any window.
3. The primary process restores/focuses `"main"` and broadcasts `app:second-instance` to all windows.

---

## 2. Public Interface

### `forge.toml`

```toml
[app]
identifier = "dev.forge.notes"
single_instance = true
```

### JavaScript Listener

```js
window.forge.listen("app:second-instance", ({ args, cwd }) => {
  console.log("Opened from second launch:", args, cwd);
});
```

### Rust Types (`forge::single_instance`)

- `SingleInstanceGuard::acquire(lock_dir, identifier, args, cwd, on_second_instance) -> Result<AcquireOutcome>`
- `SecondInstancePayload { pub args: Vec<String>, pub cwd: PathBuf }`

---

## 3. Security & Crash Recovery Invariants

- **Crash-Safe Kernel Lock:** Advisory locks (`flock` / `LockFileEx`) are released automatically by the OS kernel if the primary process crashes or receives `SIGKILL`—stale lock files never brick future launches.
- **Capability Token Authentication:** The loopback endpoint file is created with mode `0600` on Unix and stores a 128-bit random token verified in constant time before accepting forwarded CLI arguments.
