//! `MappedBuffer` for large files (large-files & search workstream).
//!
//! SPEC 8, ARCHITECTURE 2: `memmap2` map of the original file plus an
//! append-only add buffer, as a piece table in a balanced tree; background
//! line index with `memchr` (sparse checkpoint every 1024 lines);
//! `len_lines()` is `None` until indexing finishes; streaming save to a temp
//! file + rename + remap; change-on-disk detection. Implements
//! [`crate::TextBuffer`] and must give the same text as `RopeBuffer` for the
//! same edits (property test).
