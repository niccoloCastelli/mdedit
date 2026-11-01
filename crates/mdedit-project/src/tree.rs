//! Lazy project file tree for the sidebar (hidden sidecars never listed).

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Direct children of `dir`, directories first, sidecars and `.mdedit/`
/// hidden. Placeholder: returns nothing.
pub fn list_dir(dir: &std::path::Path) -> std::io::Result<Vec<TreeEntry>> {
    let _ = dir;
    Ok(Vec::new())
}
