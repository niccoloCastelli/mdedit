//! Export (SPEC 10; ARCHITECTURE 8).
//!
//! Input is the markdown source (parsed here with
//! `mdedit_core::parse::markdown_options()`, a full parse is fine for
//! export) plus a resolved [`Template`]. PDF: markdown → Typst markup →
//! `typst` compiled through a minimal `World` (bundled fonts, project files
//! for images) → `typst-pdf`. DOCX: `docx-rs` with named styles from the
//! template TOML. Blocking; the app runs it on a worker.

pub mod template;

use std::path::Path;

pub use template::Template;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Pdf,
    Docx,
}

/// Per-export overrides from the export dialog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportOptions {
    pub page_size: Option<String>,
    pub toc: Option<bool>,
    pub heading_numbers: Option<bool>,
    pub include_comments: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("template: {0}")]
    Template(String),
    #[error("typst: {0}")]
    Typst(String),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

/// Render `markdown` to bytes. `base_dir` resolves relative image paths.
pub fn export(
    markdown: &str,
    base_dir: &Path,
    template: &Template,
    options: &ExportOptions,
    format: ExportFormat,
) -> Result<Vec<u8>, ExportError> {
    let _ = (markdown, base_dir, template, options);
    Err(ExportError::Unimplemented(match format {
        ExportFormat::Pdf => "pdf export",
        ExportFormat::Docx => "docx export",
    }))
}
