//! Find and replace (large-files & search workstream).
//!
//! SPEC 5 and 8: plain text, case, whole word and regex; streamed over chunks
//! on a background thread for large files, with results delivered as they
//! are found; replace-all as one undo step. Placeholder: case-sensitive plain
//! text only, whole buffer in memory.

use std::ops::{ControlFlow, Range};

use crate::buffer::TextBuffer;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchQuery {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub range: Range<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("invalid regex: {0}")]
    InvalidRegex(String),
}

/// Report matches in document order to `sink` until it breaks. Runs on a
/// snapshot, so it is safe to call from a worker.
pub fn search(
    buf: &dyn TextBuffer,
    query: &SearchQuery,
    mut sink: impl FnMut(SearchMatch) -> ControlFlow<()>,
) -> Result<(), SearchError> {
    if query.pattern.is_empty() {
        return Ok(());
    }
    let text = buf.text();
    for (start, m) in text.match_indices(query.pattern.as_str()) {
        if sink(SearchMatch { range: start..start + m.len() }).is_break() {
            break;
        }
    }
    Ok(())
}

/// Convenience for small buffers.
pub fn find_all(buf: &dyn TextBuffer, query: &SearchQuery) -> Result<Vec<SearchMatch>, SearchError> {
    let mut out = Vec::new();
    search(buf, query, |m| {
        out.push(m);
        ControlFlow::Continue(())
    })?;
    Ok(out)
}
