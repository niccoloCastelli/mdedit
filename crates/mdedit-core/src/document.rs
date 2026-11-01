//! An open document: buffer, edit log, undo history, block index and file
//! format.

use std::path::PathBuf;
use std::time::Instant;

use crate::buffer::{RopeBuffer, TextBuffer};
use crate::edit::{AppliedEdit, Edit, EditOrigin, TxnId};
use crate::edit_log::EditLog;
use crate::io::FileFormat;
use crate::parse::{BlockIndex, ParseJob, ParseResult};
use crate::selection::Selection;
use crate::undo::UndoStack;

/// The undo state of a document at some moment, used to mark it saved after
/// a background save of a snapshot taken at that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavePoint(Option<u64>);

pub struct Document {
    buffer: Box<dyn TextBuffer>,
    log: EditLog,
    undo: UndoStack,
    /// `None` in large-file mode (SPEC 8): no parse, line-local styling only.
    blocks: Option<BlockIndex>,
    pub path: Option<PathBuf>,
    pub format: FileFormat,
    /// Undo state at the last save; the document is dirty when it differs.
    saved: SavePoint,
    next_txn: u64,
}

impl Document {
    /// A small in-memory document (rope buffer, parsed).
    pub fn from_text(text: &str) -> Self {
        Self::with_buffer(Box::new(RopeBuffer::from_text(text)), true)
    }

    /// Wrap any buffer. `parse = false` for large-file mode.
    pub fn with_buffer(buffer: Box<dyn TextBuffer>, parse: bool) -> Self {
        let blocks = parse.then(|| BlockIndex::parse(buffer.as_ref()));
        let log = EditLog::starting_at(buffer.version());
        Self {
            buffer,
            log,
            undo: UndoStack::default(),
            blocks,
            path: None,
            format: FileFormat::default(),
            saved: SavePoint(None),
            next_txn: 0,
        }
    }

    pub fn buffer(&self) -> &dyn TextBuffer {
        self.buffer.as_ref()
    }

    pub fn blocks(&self) -> Option<&BlockIndex> {
        self.blocks.as_ref()
    }

    /// A large reparse to run on a worker (`spawn_blocking`), if the block
    /// index has one outstanding; hand its result to
    /// [`Self::finish_parse`]. Until then the index keeps the old blocks,
    /// shifted (ARCHITECTURE 3.2).
    pub fn take_parse_job(&mut self) -> Option<ParseJob> {
        self.blocks.as_mut()?.take_parse_job(self.buffer.as_ref())
    }

    /// Splice in a finished parse job; returns whether it was still current.
    pub fn finish_parse(&mut self, result: ParseResult) -> bool {
        let buffer = self.buffer.as_ref();
        self.blocks.as_mut().is_some_and(|b| b.finish_parse(result, buffer))
    }

    /// Finish any outstanding reparse on the calling thread.
    pub fn flush_parse(&mut self) {
        if let Some(blocks) = &mut self.blocks {
            blocks.flush(self.buffer.as_ref());
        }
    }

    pub fn log(&self) -> &EditLog {
        &self.log
    }

    pub fn is_large(&self) -> bool {
        self.blocks.is_none()
    }

    pub fn is_dirty(&self) -> bool {
        self.save_point() != self.saved
    }

    /// The current undo state (read-only; see [`Self::begin_save`]).
    pub fn save_point(&self) -> SavePoint {
        SavePoint(self.undo.state_id())
    }

    /// Call when taking the snapshot to save: closes the current undo step
    /// so later typing can't join it (which would make the saved state and
    /// newer text share one id), and returns the point to pass to
    /// [`Self::mark_saved_at`] once the write succeeded.
    pub fn begin_save(&mut self) -> SavePoint {
        self.undo.seal();
        self.save_point()
    }

    /// The current text is what is on disk.
    pub fn mark_saved(&mut self) {
        let point = self.begin_save();
        self.mark_saved_at(point);
    }

    /// The snapshot taken at `point` was written to disk.
    pub fn mark_saved_at(&mut self, point: SavePoint) {
        self.saved = point;
    }

    /// A fresh transaction id: edits sharing it undo as one step.
    pub fn next_txn(&mut self) -> TxnId {
        self.next_txn += 1;
        TxnId(self.next_txn)
    }

    /// The single entry point for changing text. Records the edit for undo
    /// unless it is itself an undo/redo or a remote edit.
    pub fn apply(&mut self, edit: &Edit) -> AppliedEdit {
        self.apply_at(edit, Instant::now())
    }

    /// [`Self::apply`] with an explicit time, for undo-grouping tests.
    pub fn apply_at(&mut self, edit: &Edit, now: Instant) -> AppliedEdit {
        let applied = self.apply_raw(edit);
        match edit.origin {
            EditOrigin::Undo | EditOrigin::Redo | EditOrigin::Remote => {}
            EditOrigin::User | EditOrigin::Command | EditOrigin::External => self.undo.record(&applied, now),
        }
        applied
    }

    /// Replace the whole text as one undoable step (reload after an external
    /// change, "restore this version").
    pub fn replace_all(&mut self, text: &str, origin: EditOrigin) -> AppliedEdit {
        let txn = self.next_txn();
        self.apply(&Edit { range: 0..self.buffer.len_bytes(), text: text.into(), origin, txn })
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Undo one step; returns where the cursor should go.
    pub fn undo(&mut self) -> Option<Selection> {
        let group = self.undo.pop_undo()?;
        let done = self.apply_inverses(&group.edits, EditOrigin::Undo);
        let cursor = cursor_after(&done);
        self.undo.push_redo(group.id, done, Instant::now());
        cursor
    }

    /// Redo one step; returns where the cursor should go.
    pub fn redo(&mut self) -> Option<Selection> {
        let group = self.undo.pop_redo()?;
        let done = self.apply_inverses(&group.edits, EditOrigin::Redo);
        let cursor = cursor_after(&done);
        self.undo.push_undo(group.id, done, Instant::now());
        cursor
    }

    /// Bytes of text held by the undo history.
    pub fn undo_memory(&self) -> usize {
        self.undo.memory_bytes()
    }

    fn apply_inverses(&mut self, edits: &[AppliedEdit], origin: EditOrigin) -> Vec<AppliedEdit> {
        edits
            .iter()
            .rev()
            .map(|a| {
                let inverse = Edit { origin, ..a.inverse.clone() };
                self.apply_raw(&inverse)
            })
            .collect()
    }

    fn apply_raw(&mut self, edit: &Edit) -> AppliedEdit {
        let applied = self.buffer.apply(edit);
        if let Some(blocks) = &mut self.blocks {
            blocks.on_edit(&applied, self.buffer.as_ref());
        }
        self.log.push(applied.clone());
        applied
    }
}

fn cursor_after(done: &[AppliedEdit]) -> Option<Selection> {
    done.last().map(|a| Selection::cursor(a.edit.range.start + a.edit.text.len()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn type_text(doc: &mut Document, at: usize, s: &str, now: Instant) {
        for (i, c) in s.char_indices() {
            doc.apply_at(&Edit::insert(at + i, c.to_string(), EditOrigin::User, TxnId(0)), now);
        }
    }

    #[test]
    fn undo_redo_restore_text_and_cursor() {
        let mut doc = Document::from_text("");
        let t0 = Instant::now();
        type_text(&mut doc, 0, "hello world", t0);
        assert_eq!(doc.undo(), Some(Selection::cursor(6)));
        assert_eq!(doc.buffer().text(), "hello ");
        assert_eq!(doc.undo(), Some(Selection::cursor(0)));
        assert_eq!(doc.buffer().text(), "");
        assert_eq!(doc.undo(), None);
        doc.redo();
        doc.redo();
        assert_eq!(doc.buffer().text(), "hello world");
        assert!(!doc.can_redo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut doc = Document::from_text("");
        type_text(&mut doc, 0, "ab", Instant::now());
        doc.undo();
        type_text(&mut doc, 0, "x", Instant::now());
        assert!(!doc.can_redo());
    }

    #[test]
    fn dirty_tracks_save_point_through_undo() {
        let mut doc = Document::from_text("a");
        let t0 = Instant::now();
        assert!(!doc.is_dirty());
        type_text(&mut doc, 1, "b", t0);
        assert!(doc.is_dirty());
        doc.mark_saved();
        assert!(!doc.is_dirty());
        // Typing right after saving, within the grouping pause, is dirty.
        type_text(&mut doc, 2, "c", t0);
        assert!(doc.is_dirty());
        doc.undo();
        assert!(!doc.is_dirty());
        doc.undo();
        assert!(doc.is_dirty());
        doc.redo();
        assert!(!doc.is_dirty());
    }

    #[test]
    fn command_txn_undoes_as_one_step() {
        let mut doc = Document::from_text("word");
        let txn = doc.next_txn();
        let later = Instant::now() + Duration::from_secs(10);
        doc.apply(&Edit::insert(4, "**", EditOrigin::Command, txn));
        doc.apply_at(&Edit::insert(0, "**", EditOrigin::Command, txn), later);
        assert_eq!(doc.buffer().text(), "**word**");
        doc.undo();
        assert_eq!(doc.buffer().text(), "word");
    }

    #[test]
    fn log_sees_undo() {
        let mut doc = Document::from_text("");
        type_text(&mut doc, 0, "ab", Instant::now());
        doc.undo();
        let origins: Vec<_> = doc.log().since(0).unwrap().map(|a| a.edit.origin).collect();
        assert_eq!(origins, [EditOrigin::User, EditOrigin::User, EditOrigin::Undo, EditOrigin::Undo]);
    }
}
