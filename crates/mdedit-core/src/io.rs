//! Loading and saving files (core-buffer workstream).
//!
//! SPEC 3: UTF-8 with or without BOM (preserved), LF or CRLF (detected and
//! preserved), other encodings read-only; saves are atomic (temp file, fsync,
//! rename). Files above the large-file threshold open as a `MappedBuffer`.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

/// What must be preserved on save so an unedited file round-trips
/// byte-identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileFormat {
    pub bom: bool,
    pub line_ending: LineEnding,
    /// Not valid UTF-8: opened read-only with a banner.
    pub read_only: bool,
}

/// Default large-file threshold (SPEC 8), overridable in settings/project.
pub const DEFAULT_LARGE_FILE_THRESHOLD: u64 = 20 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

/// Open a file as a document, choosing rope or mapped buffer by size.
pub fn load(path: &Path, large_file_threshold: u64) -> Result<crate::Document, IoError> {
    // Placeholder until the core-buffer workstream lands BOM/line-ending
    // detection and the large-file workstream lands MappedBuffer.
    let _ = large_file_threshold;
    let text = std::fs::read_to_string(path)?;
    let mut doc = crate::Document::from_text(&text);
    doc.path = Some(path.to_path_buf());
    Ok(doc)
}

/// Atomically write the document to its path (or `path`).
pub fn save(doc: &mut crate::Document, path: Option<&Path>) -> Result<(), IoError> {
    let _ = (doc, path);
    Err(IoError::Unimplemented("io::save"))
}
