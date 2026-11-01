//! Undo/redo (core-buffer workstream).
//!
//! SPEC 5: grouping (consecutive typing groups by word or a 1 s pause, one
//! `TxnId` per command), unlimited within a memory budget, survives mode
//! switches, not persisted across restarts. Built on the inverses in
//! [`crate::AppliedEdit`]; applies them through `Document::apply` with
//! `EditOrigin::Undo` / `Redo` so every listener sees them.

/// Placeholder.
#[derive(Debug, Default)]
pub struct UndoStack {}
