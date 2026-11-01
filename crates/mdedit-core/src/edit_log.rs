//! The edit log: an ordered record of every applied edit.
//!
//! Undo/redo, comment anchors, the block index, render caches and (later)
//! collaboration follow the document through this log instead of watching
//! the buffer. Undo grouping and the memory cap are the core-buffer
//! workstream's job; this is the minimal shape they build on.

use crate::edit::AppliedEdit;

/// Something that keeps positions or caches in step with the text.
pub trait EditListener {
    fn on_edit(&mut self, applied: &AppliedEdit);
}

#[derive(Debug, Default)]
pub struct EditLog {
    entries: Vec<AppliedEdit>,
}

impl EditLog {
    pub fn push(&mut self, applied: AppliedEdit) {
        self.entries.push(applied);
    }

    /// Edits applied after `version`, oldest first. Lets a consumer that was
    /// not subscribed (a background worker, a reopened panel) catch up.
    pub fn since(&self, version: u64) -> impl Iterator<Item = &AppliedEdit> {
        let start = self.entries.partition_point(|e| e.version <= version);
        self.entries[start..].iter()
    }

    pub fn last_version(&self) -> u64 {
        self.entries.last().map_or(0, |e| e.version)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
