# Window-State Persistence

FORGE can automatically save and restore window size, screen position, and maximized state across launches (`remember_state = true`).

---

## 1. Purpose & User-Facing Behavior

When `remember_state = true` is enabled on `[window]` or `[windows.<label>]`, FORGE persists `{ x, y, width, height, maximized }` into `<data_dir>/<app.identifier>/window-state.json` whenever the window moves, resizes, maximizes, or closes, and restores those bounds on the next launch.

---

## 2. Public Interface

### `forge.toml`

```toml
[window]
title = "FORGE Studio"
width = 1200
height = 800
remember_state = true
```

### Rust API (`forge::window_state`)

- `Config::with_state_dir(&mut self, dir)` — Redirects state storage in tests.
- `SavedWindowGeometry::clamp_to_monitor(MonitorBounds)` — Clamps off-screen coordinates when an external monitor is unplugged.
- `WindowStateStore` — Sandboxed JSON persistence backed by FORGE's canonical `Sandbox`.

---

## 3. Minimal Example

```rust
use forge::App;

fn main() {
    App::new()
        .configure(|cfg| {
            cfg.window.remember_state = true;
        })
        .run()
        .unwrap();
}
```

---

## 4. Security & Multi-Monitor Invariants

- All reads and writes to `window-state.json` go through FORGE's canonical `Sandbox`, blocking symlink escapes.
- Before creating a native window, restored coordinates are clamped against the active monitor bounds so windows saved on a disconnected external display never open unreachable off-screen.
