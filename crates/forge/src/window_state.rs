use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sandbox::Sandbox;

const STATE_FILE_NAME: &str = "window-state.json";

/// Persisted geometry for a single window (`[window] remember_state = true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedWindowGeometry {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// Monitor rectangle `(origin_x, origin_y, width, height)` used to clamp restored windows onto visible displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl SavedWindowGeometry {
    /// Clamps `self` so a window saved on a disconnected external monitor remains reachable on `monitor`.
    pub fn clamp_to_monitor(mut self, monitor: MonitorBounds) -> Self {
        let mon_w = monitor.width.max(320);
        let mon_h = monitor.height.max(240);
        self.width = self.width.clamp(200, mon_w);
        self.height = self.height.clamp(150, mon_h);

        if let (Some(x), Some(y)) = (self.x, self.y) {
            let max_x = monitor.x + (mon_w as i32) - (self.width as i32).min(mon_w as i32);
            let max_y = monitor.y + (mon_h as i32) - (self.height as i32).min(mon_h as i32);
            self.x = Some(x.clamp(monitor.x, max_x.max(monitor.x)));
            self.y = Some(y.clamp(monitor.y, max_y.max(monitor.y)));
        }
        self
    }
}

/// Sandboxed JSON store for persisting window bounds across launches.
#[derive(Clone, Debug)]
pub struct WindowStateStore {
    sandbox: Sandbox,
}

impl WindowStateStore {
    pub fn new(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let sandbox = Sandbox::new(dir)?;
        Ok(Self { sandbox })
    }

    pub(crate) fn for_identifier(identifier: &str, explicit_dir: Option<&Path>) -> Option<Self> {
        let dir = match explicit_dir {
            Some(p) => p.to_path_buf(),
            None => default_state_dir(identifier)?,
        };
        Self::new(dir).ok()
    }

    pub fn load_all(&self) -> BTreeMap<String, SavedWindowGeometry> {
        let Ok(canon) = self.sandbox.resolve_existing_file(STATE_FILE_NAME) else {
            return BTreeMap::new();
        };
        let Ok(raw) = std::fs::read_to_string(canon) else {
            return BTreeMap::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn get(&self, label: &str) -> Option<SavedWindowGeometry> {
        self.load_all().remove(label)
    }

    pub fn save(&self, label: &str, geom: SavedWindowGeometry) -> std::io::Result<()> {
        let mut all = self.load_all();
        all.insert(label.to_string(), geom);
        let target = self
            .sandbox
            .prepare_write_path(STATE_FILE_NAME)
            .map_err(|e| std::io::Error::other(e.message))?;
        let serialized =
            serde_json::to_vec_pretty(&all).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(target, serialized)
    }
}

fn default_state_dir(identifier: &str) -> Option<PathBuf> {
    let safe_id: String = identifier
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    #[cfg(any(feature = "webview", feature = "cef", feature = "plugin-fs"))]
    {
        let base = dirs::data_local_dir()
            .or_else(dirs::data_dir)
            .unwrap_or_else(std::env::temp_dir);
        Some(base.join(safe_id))
    }
    #[cfg(not(any(feature = "webview", feature = "cef", feature = "plugin-fs")))]
    {
        Some(std::env::temp_dir().join("forge-state").join(safe_id))
    }
}
