//! Text storage behind a [`Document`](crate::Document).
//!
//! [`TextBuffer`] is the interface; [`RopeBuffer`] (files up to the large-file
//! threshold) is implemented here; [`MappedBuffer`] (memory-mapped piece
//! table for large files, SPEC 8) in `mapped.rs`.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use ropey::Rope;

use crate::edit::{AppliedEdit, Edit, EditOrigin};
use crate::mapped::MappedBuffer;

/// Read and edit access to document text. All offsets are UTF-8 byte offsets
/// on char boundaries; line indices are 0-based.
pub trait TextBuffer: Send + Sync {
    fn len_bytes(&self) -> usize;

    /// Number of lines, or `None` while a large file is still being indexed.
    /// A buffer ending in a newline has a final empty line, as in ropey.
    fn len_lines(&self) -> Option<usize>;

    /// Line `idx` without its line ending.
    fn line(&self, idx: usize) -> Cow<'_, str>;

    fn slice(&self, range: Range<usize>) -> Cow<'_, str>;

    fn byte_to_line(&self, byte: usize) -> usize;

    fn line_to_byte(&self, line: usize) -> usize;

    /// The text in `range` as consecutive byte chunks, for streaming search
    /// and save without building one big string.
    fn chunks(&self, range: Range<usize>) -> Box<dyn Iterator<Item = &[u8]> + '_>;

    /// Apply an edit and return it with its inverse. The buffer bumps its
    /// version; callers should go through `Document::apply` so the edit log
    /// and block index see it.
    fn apply(&mut self, edit: &Edit) -> AppliedEdit;

    /// Version after the last applied edit (0 for a fresh buffer).
    fn version(&self) -> u64;

    /// A cheap immutable copy for background workers (parse, search, save).
    fn snapshot(&self) -> Arc<dyn TextBuffer>;

    /// The whole text. Avoid on large buffers.
    fn text(&self) -> Cow<'_, str> {
        self.slice(0..self.len_bytes())
    }

    /// The large-file buffer behind this one, for what only it has (index
    /// progress, disk-change checks, save with remap, page release).
    fn as_mapped(&self) -> Option<&MappedBuffer> {
        None
    }

    /// Mutable access to the large-file buffer. Never apply edits through
    /// it: they would bypass the document's edit log.
    fn as_mapped_mut(&mut self) -> Option<&mut MappedBuffer> {
        None
    }
}

/// In-memory rope buffer (ropey), used for files below the large-file
/// threshold.
#[derive(Debug, Clone, Default)]
pub struct RopeBuffer {
    rope: Rope,
    version: u64,
}

impl RopeBuffer {
    pub fn from_text(text: &str) -> Self {
        Self { rope: Rope::from_str(text), version: 0 }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }
}

fn slice_to_cow(slice: ropey::RopeSlice<'_>) -> Cow<'_, str> {
    match slice.as_str() {
        Some(s) => Cow::Borrowed(s),
        None => Cow::Owned(slice.to_string()),
    }
}

fn strip_line_ending(mut s: Cow<'_, str>) -> Cow<'_, str> {
    let trimmed = s.trim_end_matches(['\n', '\r']).len();
    match &mut s {
        Cow::Borrowed(b) => Cow::Borrowed(&b[..trimmed]),
        Cow::Owned(o) => {
            o.truncate(trimmed);
            s
        }
    }
}

impl TextBuffer for RopeBuffer {
    fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    fn len_lines(&self) -> Option<usize> {
        Some(self.rope.len_lines())
    }

    fn line(&self, idx: usize) -> Cow<'_, str> {
        strip_line_ending(slice_to_cow(self.rope.line(idx)))
    }

    fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        slice_to_cow(self.rope.byte_slice(range))
    }

    fn byte_to_line(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte)
    }

    fn line_to_byte(&self, line: usize) -> usize {
        self.rope.line_to_byte(line)
    }

    fn chunks(&self, range: Range<usize>) -> Box<dyn Iterator<Item = &[u8]> + '_> {
        Box::new(self.rope.byte_slice(range).chunks().map(str::as_bytes))
    }

    fn apply(&mut self, edit: &Edit) -> AppliedEdit {
        let start = self.rope.byte_to_char(edit.range.start);
        let end = self.rope.byte_to_char(edit.range.end);
        let removed = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        self.rope.insert(start, &edit.text);
        self.version += 1;
        AppliedEdit {
            edit: edit.clone(),
            inverse: Edit {
                range: edit.range.start..edit.range.start + edit.text.len(),
                text: removed,
                origin: EditOrigin::Undo,
                txn: edit.txn,
            },
            version: self.version,
        }
    }

    fn version(&self) -> u64 {
        self.version
    }

    fn snapshot(&self) -> Arc<dyn TextBuffer> {
        Arc::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::TxnId;

    #[test]
    fn apply_and_inverse_round_trip() {
        let mut buf = RopeBuffer::from_text("héllo world\n");
        let a = buf.apply(&Edit { range: 7..12, text: "rust".into(), origin: EditOrigin::User, txn: TxnId(1) });
        assert_eq!(buf.text(), "héllo rust\n");
        buf.apply(&a.inverse);
        assert_eq!(buf.text(), "héllo world\n");
        assert_eq!(buf.version(), 2);
    }

    #[test]
    fn lines_strip_endings() {
        let buf = RopeBuffer::from_text("a\r\nb\nc");
        assert_eq!(buf.len_lines(), Some(3));
        assert_eq!(buf.line(0), "a");
        assert_eq!(buf.line(1), "b");
        assert_eq!(buf.line(2), "c");
        assert_eq!(buf.line_to_byte(2), 5);
    }
}
