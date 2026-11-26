use std::path::{Path, PathBuf};

use super::staged::StagedApp;
use crate::Result;
use crate::dev::ProcessRunner;

/// Seam implemented by every OS package format (`deb`, `appimage`, `tarball`, `nsis`, `zip`, `app`, `dmg`).
pub trait BundleTarget {
    /// Canonical CLI format identifier (`"deb"`, `"appimage"`, `"nsis"`, ...).
    fn id(&self) -> &'static str;

    /// Returns `true` when this package format can be produced natively on the current host OS.
    fn supported_on_host(&self) -> bool;

    /// External CLI tool required on `PATH` (if any), checked via [`ProcessRunner::has_tool`].
    fn required_tool(&self) -> Option<&'static str> {
        None
    }

    /// Packages `app` into `out_dir`, invoking any external tools exclusively through `runner`.
    fn package(
        &self,
        app: &StagedApp,
        runner: &dyn ProcessRunner,
        out_dir: &Path,
    ) -> Result<PathBuf>;
}
