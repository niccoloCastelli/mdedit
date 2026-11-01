//! Undo/redo history.
//!
//! SPEC 5: consecutive typing groups by word or a 1 s pause, all edits of one
//! `TxnId` form one step, history is unlimited within a memory budget,
//! survives mode switches, and is not persisted across restarts.
//!
//! The stack only stores [`AppliedEdit`]s; [`crate::Document`] applies the
//! inverses through its normal `apply` path (with `EditOrigin::Undo` /
//! `Redo`) so the edit log, block index and anchors see undo like any other
//! edit.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::edit::{AppliedEdit, Edit, EditOrigin};

/// Typing pauses longer than this start a new undo step.
pub const GROUP_PAUSE: Duration = Duration::from_secs(1);

/// Default cap on the text kept by the history (inserted + removed bytes).
pub const DEFAULT_MEMORY_BUDGET: usize = 64 * 1024 * 1024;

/// One undo step.
#[derive(Debug, Clone)]
pub struct Group {
    /// Unique per step; the document compares it with the id recorded at
    /// save time to know whether it is back at the saved text.
    pub id: u64,
    pub edits: Vec<AppliedEdit>,
    last_time: Instant,
    typing: Option<Typing>,
    /// Closed to further edits (set at save, so the next keystroke starts a
    /// new step and the document reads as dirty).
    sealed: bool,
}

impl Group {
    fn bytes(&self) -> usize {
        self.edits.iter().map(|a| a.edit.text.len() + a.inverse.text.len()).sum()
    }
}

/// What a typing step is doing, to decide whether the next keystroke joins
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Typing {
    /// Inserting single characters; `end` is the offset right after the last
    /// one, `last_char` the last char inserted.
    Insert { end: usize, last_char: char },
    /// Deleting with Backspace or Delete; the range of the last deletion.
    Delete { start: usize },
}

#[derive(Debug)]
pub struct UndoStack {
    undo: VecDeque<Group>,
    redo: Vec<Group>,
    next_id: u64,
    bytes: usize,
    budget: usize,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::with_budget(DEFAULT_MEMORY_BUDGET)
    }
}

impl UndoStack {
    pub fn with_budget(budget: usize) -> Self {
        Self { undo: VecDeque::new(), redo: Vec::new(), next_id: 1, bytes: 0, budget }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Id of the step that produced the current text, `None` at the start of
    /// history. Equal ids mean equal text.
    pub fn state_id(&self) -> Option<u64> {
        self.undo.back().map(|g| g.id)
    }

    /// Record a new edit (not one produced by undo/redo). Clears redo.
    pub fn record(&mut self, applied: &AppliedEdit, now: Instant) {
        debug_assert!(!matches!(applied.edit.origin, EditOrigin::Undo | EditOrigin::Redo));
        for group in self.redo.drain(..) {
            self.bytes -= group.bytes();
        }
        let typing = typing_of(&applied.edit);
        self.bytes += applied.edit.text.len() + applied.inverse.text.len();
        if let Some(last) = self.undo.back_mut()
            && joins(last, applied, typing, now)
        {
            last.edits.push(applied.clone());
            last.last_time = now;
            last.typing = typing;
        } else {
            let id = self.next_id;
            self.next_id += 1;
            self.undo.push_back(Group { id, edits: vec![applied.clone()], last_time: now, typing, sealed: false });
        }
        self.enforce_budget();
    }

    /// Take the step to undo. The caller applies the `inverse` of each edit
    /// in reverse order, then hands the results to [`Self::push_redo`].
    pub fn pop_undo(&mut self) -> Option<Group> {
        let group = self.undo.pop_back()?;
        self.bytes -= group.bytes();
        Some(group)
    }

    /// Take the step to redo; same protocol as [`Self::pop_undo`], followed
    /// by [`Self::push_undo`].
    pub fn pop_redo(&mut self) -> Option<Group> {
        let group = self.redo.pop()?;
        self.bytes -= group.bytes();
        Some(group)
    }

    /// Store the edits that performed an undo so redo can revert them. `id`
    /// is the undone step's id, kept so dirty tracking stays exact.
    pub fn push_redo(&mut self, id: u64, edits: Vec<AppliedEdit>, now: Instant) {
        let group = Group { id, edits, last_time: now, typing: None, sealed: true };
        self.bytes += group.bytes();
        self.redo.push(group);
        self.enforce_budget();
    }

    /// Store the edits that performed a redo.
    pub fn push_undo(&mut self, id: u64, edits: Vec<AppliedEdit>, now: Instant) {
        let group = Group { id, edits, last_time: now, typing: None, sealed: true };
        self.bytes += group.bytes();
        self.undo.push_back(group);
        self.enforce_budget();
    }

    /// Stop the current step from absorbing further edits.
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.back_mut() {
            last.sealed = true;
        }
    }

    /// Bytes of text currently held by the history.
    pub fn memory_bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
    }

    /// Drop the farthest redo steps, then the oldest undo steps, until the
    /// history fits the budget. The newest undo step is always kept.
    fn enforce_budget(&mut self) {
        while self.bytes > self.budget && !self.redo.is_empty() {
            let group = self.redo.remove(0);
            self.bytes -= group.bytes();
        }
        while self.bytes > self.budget && self.undo.len() > 1 {
            let group = self.undo.pop_front().expect("len > 1");
            self.bytes -= group.bytes();
        }
    }
}

fn typing_of(edit: &Edit) -> Option<Typing> {
    if edit.origin != EditOrigin::User {
        return None;
    }
    let mut chars = edit.text.chars();
    match (edit.range.is_empty(), chars.next(), chars.next()) {
        // One character typed.
        (true, Some(c), None) => Some(Typing::Insert { end: edit.range.start + c.len_utf8(), last_char: c }),
        // A deletion with nothing inserted (Backspace / Delete).
        (false, None, _) => Some(Typing::Delete { start: edit.range.start }),
        _ => None,
    }
}

fn joins(last: &Group, applied: &AppliedEdit, typing: Option<Typing>, now: Instant) -> bool {
    if last.sealed {
        return false;
    }
    let txn = applied.edit.txn;
    // An explicit transaction is always one step. TxnId(0) means "none".
    if txn.0 != 0 && last.edits.last().is_some_and(|prev| prev.edit.txn == txn) {
        return true;
    }
    if now.saturating_duration_since(last.last_time) > GROUP_PAUSE {
        return false;
    }
    match (last.typing, typing) {
        (Some(Typing::Insert { end, last_char }), Some(Typing::Insert { last_char: c, .. })) => {
            let contiguous = applied.edit.range.start == end;
            // A new word starts when a non-space follows whitespace; every
            // newline is its own step.
            let new_word = last_char.is_whitespace() && !c.is_whitespace();
            contiguous && !new_word && c != '\n' && last_char != '\n'
        }
        (Some(Typing::Delete { start }), Some(Typing::Delete { .. })) => {
            let r = &applied.edit.range;
            // Backspace run (each delete ends where the previous began) or
            // forward-delete run (each delete starts at the same offset).
            r.end == start || r.start == start
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::TxnId;

    fn typed(at: usize, c: &str) -> AppliedEdit {
        AppliedEdit {
            edit: Edit::insert(at, c, EditOrigin::User, TxnId(0)),
            inverse: Edit::delete(at..at + c.len(), EditOrigin::Undo, TxnId(0)),
            version: 0,
        }
    }

    fn deleted(range: std::ops::Range<usize>) -> AppliedEdit {
        AppliedEdit {
            edit: Edit::delete(range.clone(), EditOrigin::User, TxnId(0)),
            inverse: Edit::insert(range.start, "x", EditOrigin::Undo, TxnId(0)),
            version: 0,
        }
    }

    #[test]
    fn typing_groups_by_word() {
        let mut stack = UndoStack::default();
        let t0 = Instant::now();
        for (i, c) in "hello world".char_indices() {
            stack.record(&typed(i, &c.to_string()), t0);
        }
        // "hello " and "world"
        assert_eq!(stack.undo.len(), 2);
        assert_eq!(stack.undo[0].edits.len(), 6);
    }

    #[test]
    fn newline_is_its_own_step() {
        let mut stack = UndoStack::default();
        let t0 = Instant::now();
        for (i, c) in "ab\ncd".char_indices() {
            stack.record(&typed(i, &c.to_string()), t0);
        }
        assert_eq!(stack.undo.len(), 3);
    }

    #[test]
    fn pause_splits_step() {
        let mut stack = UndoStack::default();
        let t0 = Instant::now();
        stack.record(&typed(0, "a"), t0);
        stack.record(&typed(1, "b"), t0 + Duration::from_millis(500));
        stack.record(&typed(2, "c"), t0 + Duration::from_millis(2000));
        assert_eq!(stack.undo.len(), 2);
    }

    #[test]
    fn non_contiguous_typing_splits() {
        let mut stack = UndoStack::default();
        let t0 = Instant::now();
        stack.record(&typed(0, "a"), t0);
        stack.record(&typed(10, "b"), t0);
        assert_eq!(stack.undo.len(), 2);
    }

    #[test]
    fn backspace_run_is_one_step() {
        let mut stack = UndoStack::default();
        let t0 = Instant::now();
        stack.record(&deleted(9..10), t0);
        stack.record(&deleted(8..9), t0);
        stack.record(&deleted(7..8), t0);
        stack.record(&deleted(2..3), t0);
        assert_eq!(stack.undo.len(), 2);
    }

    #[test]
    fn same_txn_is_one_step() {
        let mut stack = UndoStack::default();
        let t0 = Instant::now();
        for (i, at) in [0, 10, 20].into_iter().enumerate() {
            let a = AppliedEdit {
                edit: Edit::insert(at, "**", EditOrigin::Command, TxnId(7)),
                inverse: Edit::delete(at..at + 2, EditOrigin::Undo, TxnId(7)),
                version: 0,
            };
            stack.record(&a, t0 + Duration::from_secs(5 * i as u64));
        }
        assert_eq!(stack.undo.len(), 1);
        assert_eq!(stack.undo[0].edits.len(), 3);
    }

    #[test]
    fn sealed_step_is_not_extended() {
        let mut stack = UndoStack::default();
        let t0 = Instant::now();
        stack.record(&typed(0, "a"), t0);
        stack.seal();
        stack.record(&typed(1, "b"), t0);
        assert_eq!(stack.undo.len(), 2);
    }

    #[test]
    fn budget_drops_oldest_but_keeps_newest() {
        let mut stack = UndoStack::with_budget(4);
        let t0 = Instant::now();
        for i in 0..5 {
            stack.record(&typed(i * 10, "x"), t0 + Duration::from_secs(i as u64 * 2));
        }
        // Each keystroke holds 1 byte (the inserted char; its inverse is a
        // pure delete).
        assert_eq!(stack.memory_bytes(), 4);
        assert_eq!(stack.undo.len(), 4);
        assert_eq!(stack.undo[0].id, 2);
    }
}
