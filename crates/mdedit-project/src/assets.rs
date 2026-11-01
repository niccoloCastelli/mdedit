//! Asset import (SPEC 6.4): copy into `assets/` next to the markdown file,
//! sanitised name with `-2`, `-3` on collision, `pasted-YYYYMMDD-HHMMSS.png`
//! for pasted images, reuse of identical files by SHA-256.

use std::path::{Path, PathBuf};

use crate::ProjectError;

pub enum AssetSource<'a> {
    File(&'a Path),
    /// Pasted image bytes (PNG).
    Pasted(&'a [u8]),
}

/// Import an asset for `md_file`; returns the relative link to insert.
pub fn import(md_file: &Path, assets_dir: &str, source: AssetSource<'_>) -> Result<PathBuf, ProjectError> {
    let _ = (md_file, assets_dir, source);
    Err(ProjectError::Unimplemented("assets::import"))
}
