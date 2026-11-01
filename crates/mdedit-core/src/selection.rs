//! Cursor and selection, as byte offsets into the document.

use std::ops::Range;

/// `anchor` is where the selection started, `head` is where the cursor is.
/// Equal values mean a plain cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn cursor(at: usize) -> Self {
        Self { anchor: at, head: at }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    /// Keep the selection valid across an applied edit.
    pub fn map(&self, applied: &crate::AppliedEdit) -> Self {
        Self { anchor: applied.map_offset(self.anchor, true), head: applied.map_offset(self.head, true) }
    }
}
