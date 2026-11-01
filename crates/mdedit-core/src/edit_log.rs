//! The edit log: an ordered record of recently applied edits.
//!
//! Comment anchors, render caches, background workers and (later)
//! collaboration follow the document through this log instead of watching
//! the buffer. The log keeps the most recent [`DEFAULT_CAPACITY`] edits; a
//! consumer that falls further behind gets `None` from [`EditLog::since`]
//! and must resynchronise from the current text.

use std::collections::VecDeque;

use crate::edit::AppliedEdit;

/// Edits kept for consumers that catch up later.
pub const DEFAULT_CAPACITY: usize = 4096;

/// Something that keeps positions or caches in step with the text.
pub trait EditListener {
    fn on_edit(&mut self, applied: &AppliedEdit);
}

#[derive(Debug)]
pub struct EditLog {
    entries: VecDeque<AppliedEdit>,
    capacity: usize,
    /// Version of the newest entry ever pushed (survives trimming).
    last_version: u64,
}

impl Default for EditLog {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl EditLog {
    pub fn with_capacity(capacity: usize) -> Self {
        Self { entries: VecDeque::new(), capacity: capacity.max(1), last_version: 0 }
    }

    /// Start counting from `version` (a document whose buffer has already
    /// been edited before the log existed).
    pub fn starting_at(version: u64) -> Self {
        Self { last_version: version, ..Self::default() }
    }

    pub fn push(&mut self, applied: AppliedEdit) {
        self.last_version = applied.version;
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(applied);
    }

    /// Edits applied after `version`, oldest first, or `None` if some of
    /// them were already trimmed (the caller must resync from the text).
    pub fn since(&self, version: u64) -> Option<impl Iterator<Item = &AppliedEdit>> {
        if version >= self.last_version {
            return Some(self.entries.range(self.entries.len()..));
        }
        let oldest = self.entries.front()?.version;
        if version + 1 < oldest {
            return None;
        }
        let start = self.entries.partition_point(|e| e.version <= version);
        Some(self.entries.range(start..))
    }

    /// Feed every edit after `version` to `listener`; returns the version it
    /// is now at, or `None` if it fell too far behind.
    pub fn catch_up(&self, listener: &mut dyn EditListener, version: u64) -> Option<u64> {
        for applied in self.since(version)? {
            listener.on_edit(applied);
        }
        Some(self.last_version)
    }

    pub fn last_version(&self) -> u64 {
        self.last_version
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Edit, EditOrigin, TxnId};

    fn entry(version: u64) -> AppliedEdit {
        let e = Edit::insert(0, "x", EditOrigin::User, TxnId(0));
        AppliedEdit { inverse: Edit::delete(0..1, EditOrigin::Undo, TxnId(0)), edit: e, version }
    }

    #[test]
    fn since_and_trimming() {
        let mut log = EditLog::with_capacity(3);
        for v in 1..=5 {
            log.push(entry(v));
        }
        let versions = |from| log.since(from).map(|it| it.map(|e| e.version).collect::<Vec<_>>());
        assert_eq!(versions(5), Some(vec![]));
        assert_eq!(versions(3), Some(vec![4, 5]));
        assert_eq!(versions(2), Some(vec![3, 4, 5]));
        assert_eq!(versions(1), None);
    }
}
