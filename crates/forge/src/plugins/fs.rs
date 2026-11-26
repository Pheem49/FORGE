use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::sandbox::Sandbox;
use crate::{App, Error, ForgeError, Plugin, State};

#[derive(Debug, Deserialize, Serialize)]
pub struct FsPathReq {
    pub path: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsWriteReq {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FsListReq {
    pub path: Option<String>,
}

/// Sandboxed application data storage plugin (`fs.read_text`, `fs.write_text`,
/// `fs.list_dir`, `fs.remove_file`), confining all operations strictly inside
/// `<data_dir>/<app.identifier>/`.
#[derive(Clone, Default)]
pub struct AppFsPlugin {
    explicit_root: Option<PathBuf>,
}

impl AppFsPlugin {
    pub fn new() -> Self {
        Self {
            explicit_root: None,
        }
    }

    /// Anchors sandboxed storage to an explicit directory (ideal for `tempfile::TempDir` tests).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            explicit_root: Some(root.into()),
        }
    }
}

impl Plugin for AppFsPlugin {
    fn apply(self, app: App) -> App {
        let explicit_root = self.explicit_root;
        app.setup(move |ctx| {
            let root = match explicit_root {
                Some(p) => p,
                None => {
                    let base = dirs::data_local_dir()
                        .or_else(dirs::data_dir)
                        .unwrap_or_else(std::env::temp_dir);
                    base.join(&ctx.config().app.identifier)
                }
            };
            let sandbox = Sandbox::new(&root).map_err(|e| {
                Error::SetupFailed(format!(
                    "failed to initialize AppFsPlugin root '{}': {e}",
                    root.display()
                ))
            })?;
            ctx.insert_state(sandbox);
            Ok(())
        })
        .command("fs.read_text", fs_read_text)
        .command("fs.write_text", fs_write_text)
        .command("fs.list_dir", fs_list_dir)
        .command("fs.remove_file", fs_remove_file)
    }
}

async fn fs_read_text(sandbox: State<Sandbox>, req: FsPathReq) -> Result<String, ForgeError> {
    let path = req.path;
    tokio::task::spawn_blocking(move || {
        let canon = sandbox.resolve_existing_file(&path)?;
        std::fs::read_to_string(&canon).map_err(ForgeError::from)
    })
    .await
    .map_err(|e| ForgeError::internal("FS_TASK_FAILED", e.to_string()))?
}

async fn fs_write_text(sandbox: State<Sandbox>, req: FsWriteReq) -> Result<(), ForgeError> {
    let path = req.path;
    let contents = req.contents;
    tokio::task::spawn_blocking(move || {
        let target = sandbox.prepare_write_path(&path)?;
        std::fs::write(&target, contents).map_err(ForgeError::from)
    })
    .await
    .map_err(|e| ForgeError::internal("FS_TASK_FAILED", e.to_string()))?
}

async fn fs_list_dir(sandbox: State<Sandbox>, req: FsListReq) -> Result<Vec<String>, ForgeError> {
    let rel = req.path.unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        let Some(canon_dir) = sandbox.resolve_optional_dir(&rel)? else {
            return Ok(Vec::new());
        };
        let mut entries = Vec::new();
        for entry_res in std::fs::read_dir(&canon_dir).map_err(ForgeError::from)? {
            let entry = entry_res.map_err(ForgeError::from)?;
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }
        entries.sort();
        Ok(entries)
    })
    .await
    .map_err(|e| ForgeError::internal("FS_TASK_FAILED", e.to_string()))?
}

async fn fs_remove_file(sandbox: State<Sandbox>, req: FsPathReq) -> Result<(), ForgeError> {
    let path = req.path;
    tokio::task::spawn_blocking(move || {
        let canon = sandbox.resolve_existing_file(&path)?;
        std::fs::remove_file(&canon).map_err(ForgeError::from)
    })
    .await
    .map_err(|e| ForgeError::internal("FS_TASK_FAILED", e.to_string()))?
}
