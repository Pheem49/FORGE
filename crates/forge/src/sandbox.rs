use std::path::{Component, Path, PathBuf};

use crate::error::ForgeError;

/// Canonical directory confinement primitive enforcing pre-creation ancestor checks
/// and post-resolution symlink containment inside an explicit root directory.
#[derive(Clone, Debug)]
pub(crate) struct Sandbox {
    canonical_root: PathBuf,
}

#[allow(dead_code)]
impl Sandbox {
    /// Initializes (creating if absent) and canonicalizes `root`.
    pub(crate) fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        let canonical_root = root.canonicalize()?;
        Ok(Self { canonical_root })
    }

    #[allow(dead_code)]
    pub(crate) fn root(&self) -> &Path {
        &self.canonical_root
    }

    /// Resolves an existing file inside the sandbox, blocking `..` traversal and symlink escapes.
    pub(crate) fn resolve_existing_file(&self, raw_rel: &str) -> Result<PathBuf, ForgeError> {
        let candidate = self.join_lexical(raw_rel, false)?;
        let canon = candidate
            .canonicalize()
            .map_err(|e| ForgeError::not_found("FS_NOT_FOUND", e.to_string()))?;
        if !canon.starts_with(&self.canonical_root) {
            return Err(ForgeError::forbidden(
                "FS_FORBIDDEN",
                "symlink target escapes application storage directory",
            ));
        }
        Ok(canon)
    }

    /// Resolves an optional directory inside the sandbox (`Ok(None)` if it does not yet exist).
    #[allow(dead_code)]
    pub(crate) fn resolve_optional_dir(
        &self,
        raw_rel: &str,
    ) -> Result<Option<PathBuf>, ForgeError> {
        let candidate = self.join_lexical(raw_rel, true)?;
        if !candidate.exists() {
            return Ok(None);
        }
        let canon = candidate.canonicalize().map_err(ForgeError::from)?;
        if !canon.starts_with(&self.canonical_root) {
            return Err(ForgeError::forbidden(
                "FS_FORBIDDEN",
                "directory symlink escapes application storage directory",
            ));
        }
        Ok(Some(canon))
    }

    /// Safely prepares parent directories and resolves a file destination for writing,
    /// verifying that the deepest existing ancestor stays inside `canonical_root` *before*
    /// calling `create_dir_all`.
    pub(crate) fn prepare_write_path(&self, raw_rel: &str) -> Result<PathBuf, ForgeError> {
        let target = self.join_lexical(raw_rel, false)?;

        let mut ancestor = target.parent();
        while let Some(dir) = ancestor {
            if dir.exists() {
                let canon = dir.canonicalize().map_err(ForgeError::from)?;
                if !canon.starts_with(&self.canonical_root) {
                    return Err(ForgeError::forbidden(
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
            if !canon_parent.starts_with(&self.canonical_root) {
                return Err(ForgeError::forbidden(
                    "FS_FORBIDDEN",
                    "parent directory escapes application storage directory",
                ));
            }
        }

        if target.exists() {
            let canon = target.canonicalize().map_err(ForgeError::from)?;
            if !canon.starts_with(&self.canonical_root) {
                return Err(ForgeError::forbidden(
                    "FS_FORBIDDEN",
                    "existing file symlink escapes application storage directory",
                ));
            }
        }

        Ok(target)
    }

    fn join_lexical(&self, raw_rel: &str, allow_empty: bool) -> Result<PathBuf, ForgeError> {
        if raw_rel.contains('\0') || raw_rel.contains('\\') {
            return Err(ForgeError::bad_request(
                "FS_FORBIDDEN",
                "path contains invalid characters",
            ));
        }
        let trimmed = raw_rel.trim().trim_start_matches('/');
        if trimmed.is_empty() {
            return if allow_empty {
                Ok(self.canonical_root.clone())
            } else {
                Err(ForgeError::bad_request(
                    "FS_INVALID_PATH",
                    "file path must not be empty",
                ))
            };
        }

        let mut joined = self.canonical_root.clone();
        for comp in Path::new(trimmed).components() {
            match comp {
                Component::Normal(seg) => joined.push(seg),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(ForgeError::forbidden(
                        "FS_FORBIDDEN",
                        "relative path traversal ('..') is forbidden",
                    ));
                }
            }
        }
        Ok(joined)
    }
}
