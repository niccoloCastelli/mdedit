//! An open document: buffer + edit log + block index + file format.

use std::path::PathBuf;

use crate::buffer::{RopeBuffer, TextBuffer};
use crate::edit::{AppliedEdit, Edit};
use crate::edit_log::EditLog;
use crate::io::FileFormat;
use crate::parse::BlockIndex;

pub struct Document {
    buffer: Box<dyn TextBuffer>,
    log: EditLog,
    /// `None` in large-file mode (SPEC 8): no parse, line-local styling only.
    blocks: Option<BlockIndex>,
    pub path: Option<PathBuf>,
    pub format: FileFormat,
    /// Buffer version at the last save; dirty when it differs.
    saved_version: u64,
}

impl Document {
    /// A small in-memory document (rope buffer, parsed).
    pub fn from_text(text: &str) -> Self {
        Self::with_buffer(Box::new(RopeBuffer::from_text(text)), true)
    }

    /// Wrap any buffer. `parse = false` for large-file mode.
    pub fn with_buffer(buffer: Box<dyn TextBuffer>, parse: bool) -> Self {
        let blocks = parse.then(|| BlockIndex::parse(buffer.as_ref()));
        let saved_version = buffer.version();
        Self { buffer, log: EditLog::default(), blocks, path: None, format: FileFormat::default(), saved_version }
    }

    pub fn buffer(&self) -> &dyn TextBuffer {
        self.buffer.as_ref()
    }

    pub fn blocks(&self) -> Option<&BlockIndex> {
        self.blocks.as_ref()
    }

    pub fn log(&self) -> &EditLog {
        &self.log
    }

    pub fn is_large(&self) -> bool {
        self.blocks.is_none()
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.version() != self.saved_version
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.buffer.version();
    }

    /// The single entry point for changing text.
    pub fn apply(&mut self, edit: &Edit) -> AppliedEdit {
        let applied = self.buffer.apply(edit);
        if let Some(blocks) = &mut self.blocks {
            blocks.on_edit(&applied, self.buffer.as_ref());
        }
        self.log.push(applied.clone());
        applied
    }
}
