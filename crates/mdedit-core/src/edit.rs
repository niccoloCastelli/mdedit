//! Edits: the only way text changes.
//!
//! Every mutation (typing, undo, commands, external reloads, later remote
//! collaborators) is an [`Edit`] applied through [`crate::Document::apply`],
//! which records it in the [`crate::EditLog`]. Offsets are **byte** offsets
//! into the UTF-8 text and must fall on char boundaries.

use std::ops::Range;

/// Groups edits that undo/redo as one step (typing a word, replace-all, a
/// formatting command that touches two places).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct TxnId(pub u64);

/// Where an edit came from. Listeners use it, e.g. undo grouping only merges
/// `User` typing, and comment anchors treat `External` reloads differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditOrigin {
    /// Typing, paste, delete from the editor widgets.
    User,
    /// A document command (formatting, table edit, replace-all).
    Command,
    Undo,
    Redo,
    /// Reload after the file changed on disk.
    External,
    /// Reserved for collaborative editing (v2).
    Remote,
}

/// Replace `range` with `text`. Insertion is an empty range, deletion is
/// empty text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub origin: EditOrigin,
    pub txn: TxnId,
}

impl Edit {
    pub fn insert(at: usize, text: impl Into<String>, origin: EditOrigin, txn: TxnId) -> Self {
        Self { range: at..at, text: text.into(), origin, txn }
    }

    pub fn delete(range: Range<usize>, origin: EditOrigin, txn: TxnId) -> Self {
        Self { range, text: String::new(), origin, txn }
    }

    /// Byte length change caused by this edit.
    pub fn delta(&self) -> isize {
        self.text.len() as isize - (self.range.end - self.range.start) as isize
    }
}

/// An edit after it was applied, with what is needed to undo it and to shift
/// positions held elsewhere (block index, anchors, cursors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEdit {
    pub edit: Edit,
    /// The edit that restores the previous text.
    pub inverse: Edit,
    /// Document version after this edit (monotonic, starts at 0).
    pub version: u64,
}

impl AppliedEdit {
    /// Map a byte offset from before this edit to after it. Offsets inside
    /// the replaced range move to its end if `stick_right`, else its start.
    pub fn map_offset(&self, offset: usize, stick_right: bool) -> usize {
        let r = &self.edit.range;
        if offset < r.start {
            offset
        } else if offset > r.end || (offset == r.end && r.start != r.end) {
            (offset as isize + self.edit.delta()) as usize
        } else if stick_right {
            r.start + self.edit.text.len()
        } else {
            r.start
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(range: Range<usize>, text: &str) -> AppliedEdit {
        let edit = Edit { range: range.clone(), text: text.into(), origin: EditOrigin::User, txn: TxnId(0) };
        let inverse = Edit::delete(range.start..range.start + text.len(), EditOrigin::Undo, TxnId(0));
        AppliedEdit { edit, inverse, version: 1 }
    }

    #[test]
    fn map_offset_shifts_after_and_keeps_before() {
        let a = applied(5..8, "abcdef");
        assert_eq!(a.map_offset(2, false), 2);
        assert_eq!(a.map_offset(10, false), 13);
        assert_eq!(a.map_offset(6, false), 5);
        assert_eq!(a.map_offset(6, true), 11);
    }

    #[test]
    fn map_offset_at_insertion_point() {
        let a = applied(4..4, "xy");
        assert_eq!(a.map_offset(4, false), 4);
        assert_eq!(a.map_offset(4, true), 6);
    }
}
