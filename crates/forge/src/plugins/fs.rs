use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

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

#[derive(Clone)]
struct AppFsRoot {
    canonical_root: PathBuf,
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
            std::fs::create_dir_all(&root).map_err(|e| {
                Error::SetupFailed(format!(
                    "failed to initialize AppFsPlugin root '{}': {e}",
                    root.display()
                ))
            })?;
            let canonical_root = root.canonicalize().map_err(|e| {
                Error::SetupFailed(format!(
                    "failed to canonicalize AppFsPlugin root '{}': {e}",
                    root.display()
                ))
            })?;
            ctx.insert_state(AppFsRoot { canonical_root });
            Ok(())
        })
        .command("fs.read_text", fs_read_text)
        .command("fs.write_text", fs_write_text)
        .command("fs.list_dir", fs_list_dir)
        .command("fs.remove_file", fs_remove_file)
    }
}

async fn fs_read_text(root: State<AppFsRoot>, req: FsPathReq) -> Result<String, ForgeError> {
    let target = resolve_confined_path(&root.canonical_root, &req.path, false)?;
    tokio::task::spawn_blocking(move || {
        let canon = target
            .canonicalize()
            .map_err(|e| ForgeError::new("FS_NOT_FOUND", e.to_string()))?;
        if !canon.starts_with(&root.canonical_root) {
            return Err(ForgeError::new(
                "FS_FORBIDDEN",
                "symlink target escapes application storage directory",
            ));
        }
        std::fs::read_to_string(&canon).map_err(ForgeError::from)
    })
    .await
    .map_err(|e| ForgeError::new("FS_TASK_FAILED", e.to_string()))?
}

async fn fs_write_text(root: State<AppFsRoot>, req: FsWriteReq) -> Result<(), ForgeError> {
    let target = resolve_confined_path(&root.canonical_root, &req.path, false)?;
    let contents = req.contents;
    tokio::task::spawn_blocking(move || {
        // Verify the deepest existing ancestor stays inside `canonical_root` BEFORE creating directories
        // so an intermediate symlink cannot redirect `create_dir_all` outside the sandbox.
        let mut ancestor = target.parent();
        while let Some(dir) = ancestor {
            if dir.exists() {
                let canon = dir.canonicalize().map_err(ForgeError::from)?;
                if !canon.starts_with(&root.canonical_root) {
                    return Err(ForgeError::new(
                        "FS_FORBIDDEN",
                        "ancestor directory symlink escapes application storage directory",
                    ));
                }
                break;
            }
            ancestor = dir.parent();
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(ForgeError::from)?;
            let canon_parent = parent.canonicalize().map_err(ForgeError::from)?;
            if !canon_parent.starts_with(&root.canonical_root) {
                return Err(ForgeError::new(
                    "FS_FORBIDDEN",
                    "parent directory escapes application storage directory",
                ));
            }
        }
        if target.exists() {
            let canon = target.canonicalize().map_err(ForgeError::from)?;
            if !canon.starts_with(&root.canonical_root) {
                return Err(ForgeError::new(
                    "FS_FORBIDDEN",
                    "existing file symlink escapes application storage directory",
                ));
            }
        }
        std::fs::write(&target, contents).map_err(ForgeError::from)
    })
    .await
    .map_err(|e| ForgeError::new("FS_TASK_FAILED", e.to_string()))?
}

async fn fs_list_dir(root: State<AppFsRoot>, req: FsListReq) -> Result<Vec<String>, ForgeError> {
    let rel = req.path.as_deref().unwrap_or("");
    let target = resolve_confined_path(&root.canonical_root, rel, true)?;
    tokio::task::spawn_blocking(move || {
        if !target.exists() {
            return Ok(Vec::new());
        }
        let canon = target.canonicalize().map_err(ForgeError::from)?;
        if !canon.starts_with(&root.canonical_root) {
            return Err(ForgeError::new(
                "FS_FORBIDDEN",
                "directory symlink escapes application storage directory",
            ));
        }
        let mut entries = Vec::new();
        for entry_res in std::fs::read_dir(&canon).map_err(ForgeError::from)? {
            let entry = entry_res.map_err(ForgeError::from)?;
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }
        entries.sort();
        Ok(entries)
    })
    .await
    .map_err(|e| ForgeError::new("FS_TASK_FAILED", e.to_string()))?
}

async fn fs_remove_file(root: State<AppFsRoot>, req: FsPathReq) -> Result<(), ForgeError> {
    let target = resolve_confined_path(&root.canonical_root, &req.path, false)?;
    tokio::task::spawn_blocking(move || {
        let canon = target
            .canonicalize()
            .map_err(|e| ForgeError::new("FS_NOT_FOUND", e.to_string()))?;
        if !canon.starts_with(&root.canonical_root) {
            return Err(ForgeError::new(
                "FS_FORBIDDEN",
                "symlink target escapes application storage directory",
            ));
        }
        std::fs::remove_file(&canon).map_err(ForgeError::from)
    })
    .await
    .map_err(|e| ForgeError::new("FS_TASK_FAILED", e.to_string()))?
}

fn resolve_confined_path(
    canon_root: &Path,
    raw_rel: &str,
    allow_empty: bool,
) -> Result<PathBuf, ForgeError> {
    if raw_rel.contains('\0') || raw_rel.contains('\\') {
        return Err(ForgeError::bad_request(
            "FS_FORBIDDEN",
            "path contains invalid characters",
        ));
    }
    let trimmed = raw_rel.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        return if allow_empty {
            Ok(canon_root.to_path_buf())
        } else {
            Err(ForgeError::bad_request(
                "FS_INVALID_PATH",
                "file path must not be empty",
            ))
        };
    }

    let mut joined = canon_root.to_path_buf();
    for comp in Path::new(trimmed).components() {
        match comp {
            Component::Normal(seg) => joined.push(seg),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ForgeError::new(
                    "FS_FORBIDDEN",
                    "relative path traversal ('..') is forbidden",
                ));
            }
        }
    }
    Ok(joined)
}
