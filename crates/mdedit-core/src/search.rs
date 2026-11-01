//! Find and replace (large-files & search workstream).
//!
//! SPEC 5 and 8: plain text, case, whole word and regex. Every search runs
//! over [`TextBuffer::chunks`] in windows of [`WINDOW`] bytes, so a 1 GB
//! mapped file never becomes one string; [`spawn_search`] runs it on a
//! background thread over a snapshot and streams the results; and
//! [`replace_all`] applies every replacement under one `TxnId`, so it undoes
//! as one step.
//!
//! Matching is `regex-automata`'s meta regex (plain text is escaped into a
//! pattern), multi-line (`^`/`$` match at line breaks, CRLF aware). Windows
//! end at a line break where one is near, and each window searches
//! [`OVERLAP`] bytes past its end, so a match is only cut short if it is
//! longer than that. Empty matches are skipped. Whole word means the match is
//! not preceded or followed by a letter, digit or `_`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::{ControlFlow, Range};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use regex_automata::meta::Regex;
use regex_automata::util::captures::Captures;
use regex_automata::util::syntax;
use regex_automata::Input;

use crate::buffer::TextBuffer;
use crate::document::Document;
use crate::edit::{Edit, EditOrigin};

/// Bytes searched per window (one progress step for streamed searches).
pub const WINDOW: usize = 1 << 20;
/// How far past a window's end a match that started inside it may reach.
pub const OVERLAP: usize = 256 * 1024;
/// How far a window end may move forward to land on a line break.
const EXTEND: usize = 64 * 1024;
/// Context bytes around the searched span, for `\b` and `^`/`$`.
const CONTEXT: usize = 4;

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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    #[error("invalid regex: {0}")]
    InvalidRegex(String),
}

/// A compiled query. Build it once (this is where an invalid regex is
/// reported) and run it over any buffer.
#[derive(Debug, Clone)]
pub struct Searcher {
    re: Option<Regex>,
    whole_word: bool,
}

/// One match as seen by [`Searcher::run`]: its range in the buffer, plus the
/// window it was found in and its capture groups (when asked for), to expand
/// a replacement.
pub struct Found<'a> {
    pub range: Range<usize>,
    hay: &'a [u8],
    caps: Option<&'a Captures>,
}

impl Found<'_> {
    /// `replacement` with `$1`, `${name}` expanded (regex captures were
    /// requested) or as is.
    pub fn expand(&self, replacement: &str) -> String {
        match self.caps {
            Some(caps) => {
                let mut out = Vec::new();
                caps.interpolate_bytes_into(self.hay, replacement.as_bytes(), &mut out);
                String::from_utf8_lossy(&out).into_owned()
            }
            None => replacement.to_owned(),
        }
    }
}

impl Searcher {
    pub fn new(query: &SearchQuery) -> Result<Self, SearchError> {
        if query.pattern.is_empty() {
            return Ok(Self { re: None, whole_word: query.whole_word });
        }
        let pattern = if query.regex { Cow::Borrowed(query.pattern.as_str()) } else { Cow::Owned(escape(&query.pattern)) };
        let re = Regex::builder()
            .syntax(syntax::Config::new().case_insensitive(!query.case_sensitive).multi_line(true).crlf(true))
            .build(&pattern)
            .map_err(|e| SearchError::InvalidRegex(e.to_string()))?;
        Ok(Self { re: Some(re), whole_word: query.whole_word })
    }

    /// Report matches starting in `range`, in order, to `on_match`; after
    /// each window, report how far the search got to `on_progress`. Either
    /// can stop the search. `captures` makes [`Found::expand`] expand groups
    /// (a little slower).
    pub fn run(
        &self,
        buf: &dyn TextBuffer,
        range: Range<usize>,
        captures: bool,
        mut on_match: impl FnMut(Found<'_>) -> ControlFlow<()>,
        mut on_progress: impl FnMut(usize) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        let Some(re) = &self.re else { return ControlFlow::Continue(()) };
        let len = buf.len_bytes();
        let end = range.end.min(len);
        let mut pos = range.start.min(end);
        let mut caps = re.create_captures();
        while pos < end {
            let h0 = pos.saturating_sub(CONTEXT);
            let hay = read_bytes(buf, h0..(pos + WINDOW + OVERLAP + 2 * EXTEND + CONTEXT).min(len));
            let window_end = if pos + WINDOW >= end { end } else { (h0 + boundary_after(&hay, pos + WINDOW - h0)).min(end) };
            let span_end = if window_end >= end {
                end
            } else {
                (h0 + boundary_after(&hay, window_end + OVERLAP - h0)).min(end)
            };
            let mut at = pos;
            let mut last_end = pos;
            while at < window_end {
                let input = Input::new(&*hay).span(at - h0..span_end - h0);
                let m = if captures {
                    re.search_captures(&input, &mut caps);
                    caps.get_match()
                } else {
                    re.search(&input)
                };
                let Some(m) = m else { break };
                if h0 + m.start() >= window_end {
                    break;
                }
                if m.is_empty() || (self.whole_word && !is_whole_word(&hay, m.start(), m.end())) {
                    at = h0 + next_char(&hay, m.start());
                    continue;
                }
                let found = Found { range: h0 + m.start()..h0 + m.end(), hay: &hay, caps: captures.then_some(&caps) };
                if on_match(found).is_break() {
                    return ControlFlow::Break(());
                }
                at = h0 + m.end();
                last_end = at;
            }
            if let Some(mapped) = buf.as_mapped() {
                mapped.release_pages(pos..window_end);
            }
            if on_progress(window_end).is_break() {
                return ControlFlow::Break(());
            }
            pos = window_end.max(last_end);
        }
        ControlFlow::Continue(())
    }
}

/// Report matches in document order to `sink` until it breaks. Runs on a
/// snapshot, so it is safe to call from a worker.
pub fn search(
    buf: &dyn TextBuffer,
    query: &SearchQuery,
    mut sink: impl FnMut(SearchMatch) -> ControlFlow<()>,
) -> Result<(), SearchError> {
    let searcher = Searcher::new(query)?;
    let _ = searcher.run(
        buf,
        0..buf.len_bytes(),
        false,
        |f| sink(SearchMatch { range: f.range }),
        |_| ControlFlow::Continue(()),
    );
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

/// What a background search sends, in order: batches of matches and
/// progress after each window, then `Done`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    Matches(Vec<SearchMatch>),
    Progress { searched: usize, total: usize },
    Done { matches: usize, cancelled: bool },
}

/// A search running on its own thread. Dropping it cancels the search.
#[derive(Debug)]
pub struct SearchJob {
    pub events: mpsc::Receiver<SearchEvent>,
    cancel: Arc<AtomicBool>,
}

impl SearchJob {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for SearchJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Matches sent per batch at most (a window with more sends several).
const BATCH: usize = 4096;

/// Search `buf` (a snapshot) on a background thread, streaming results as
/// they are found (SPEC 8). An invalid regex is reported here, before any
/// thread starts. The app forwards [`SearchJob::events`] to the UI (an iced
/// subscription) and drops the job to cancel.
pub fn spawn_search(buf: Arc<dyn TextBuffer>, query: &SearchQuery) -> Result<SearchJob, SearchError> {
    let searcher = Searcher::new(query)?;
    let (tx, events) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    std::thread::Builder::new()
        .name("mdedit-search".into())
        .spawn(move || {
            let total = buf.len_bytes();
            // Both callbacks flush it.
            let batch = RefCell::new(Vec::new());
            let flush = || {
                let matches = std::mem::take(&mut *batch.borrow_mut());
                matches.is_empty() || tx.send(SearchEvent::Matches(matches)).is_ok()
            };
            let mut matches = 0;
            let flow = searcher.run(
                buf.as_ref(),
                0..total,
                false,
                |f| {
                    matches += 1;
                    batch.borrow_mut().push(SearchMatch { range: f.range });
                    if batch.borrow().len() >= BATCH && !flush() {
                        return ControlFlow::Break(());
                    }
                    ControlFlow::Continue(())
                },
                |searched| {
                    if stop.load(Ordering::Relaxed) {
                        return ControlFlow::Break(());
                    }
                    if !flush() {
                        return ControlFlow::Break(());
                    }
                    match tx.send(SearchEvent::Progress { searched, total }) {
                        Ok(()) => ControlFlow::Continue(()),
                        Err(_) => ControlFlow::Break(()),
                    }
                },
            );
            let _ = tx.send(SearchEvent::Done { matches, cancelled: flow.is_break() });
        })
        .expect("spawn search thread");
    Ok(SearchJob { events, cancel })
}

/// Replace every match with `replacement` (`$1`, `${name}` expand in regex
/// mode) as one undo step. Returns the number of replacements.
pub fn replace_all(doc: &mut Document, query: &SearchQuery, replacement: &str) -> Result<usize, SearchError> {
    let searcher = Searcher::new(query)?;
    let captures = query.regex && replacement.contains('$');
    let mut edits = Vec::new();
    let _ = searcher.run(
        doc.buffer(),
        0..doc.buffer().len_bytes(),
        captures,
        |f| {
            edits.push((f.range.clone(), f.expand(replacement)));
            ControlFlow::Continue(())
        },
        |_| ControlFlow::Continue(()),
    );
    if edits.is_empty() {
        return Ok(0);
    }
    let txn = doc.next_txn();
    // Back to front, so earlier ranges stay valid.
    for (range, text) in edits.iter().rev() {
        doc.apply(&Edit { range: range.clone(), text: text.clone(), origin: EditOrigin::Command, txn });
    }
    Ok(edits.len())
}

/// `range` as one slice, borrowed when it lies in one chunk.
fn read_bytes(buf: &dyn TextBuffer, range: Range<usize>) -> Cow<'_, [u8]> {
    let mut chunks = buf.chunks(range.clone());
    let Some(first) = chunks.next() else { return Cow::Borrowed(&[]) };
    match chunks.next() {
        None => Cow::Borrowed(first),
        Some(second) => {
            let mut out = Vec::with_capacity(range.len());
            out.extend_from_slice(first);
            out.extend_from_slice(second);
            chunks.for_each(|c| out.extend_from_slice(c));
            Cow::Owned(out)
        }
    }
}

/// A position at or after `i`: just past a line break within [`EXTEND`]
/// bytes if there is one, else the next char boundary.
fn boundary_after(hay: &[u8], i: usize) -> usize {
    if i >= hay.len() {
        return hay.len();
    }
    if let Some(j) = memchr::memchr(b'\n', &hay[i..(i + EXTEND).min(hay.len())]) {
        return i + j + 1;
    }
    let mut i = i;
    while i < hay.len() && is_continuation(hay[i]) {
        i += 1;
    }
    i
}

fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

fn next_char(hay: &[u8], i: usize) -> usize {
    let mut i = i + 1;
    while i < hay.len() && is_continuation(hay[i]) {
        i += 1;
    }
    i
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_whole_word(hay: &[u8], start: usize, end: usize) -> bool {
    let mut j = start;
    while j > start.saturating_sub(4) {
        j -= 1;
        if !is_continuation(hay[j]) {
            break;
        }
    }
    let before = std::str::from_utf8(&hay[j..start]).ok().and_then(|s| s.chars().next_back());
    let after = hay[end..(end + 4).min(hay.len())].utf8_chunks().next().and_then(|c| c.valid().chars().next());
    !before.is_some_and(is_word) && !after.is_some_and(is_word)
}

/// Escape regex metacharacters (the set `regex-syntax` escapes).
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '#' | '&' | '-' | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MappedBuffer, RopeBuffer};

    fn q(pattern: &str) -> SearchQuery {
        SearchQuery { pattern: pattern.into(), case_sensitive: true, ..Default::default() }
    }

    fn ranges(buf: &dyn TextBuffer, query: &SearchQuery) -> Vec<Range<usize>> {
        find_all(buf, query).unwrap().into_iter().map(|m| m.range).collect()
    }

    #[test]
    fn plain_case_and_whole_word() {
        let buf = RopeBuffer::from_text("Foo foo food a.b (x) foo_bar\nfoo");
        assert_eq!(ranges(&buf, &q("foo")), [4..7, 8..11, 21..24, 29..32]);
        let ci = SearchQuery { case_sensitive: false, ..q("foo") };
        assert_eq!(ranges(&buf, &ci).len(), 5);
        let ww = SearchQuery { whole_word: true, ..q("foo") };
        assert_eq!(ranges(&buf, &ww), [4..7, 29..32]);
        assert_eq!(ranges(&buf, &q("a.b")), [13..16]);
        assert_eq!(ranges(&buf, &q("(x)")), [17..20]);
        assert!(ranges(&buf, &q("")).is_empty());
    }

    #[test]
    fn unicode_case_folding_and_word_boundaries() {
        let buf = RopeBuffer::from_text("Straße STRASSE ÉTÉ été étés");
        let ci = SearchQuery { case_sensitive: false, whole_word: true, ..q("été") };
        assert_eq!(ranges(&buf, &ci), [16..21, 22..27]);
    }

    #[test]
    fn regex_lines_and_errors() {
        let buf = RopeBuffer::from_text("# One\r\ntext\n## Two\n");
        let re = SearchQuery { regex: true, ..q(r"^#+ (\w+)$") };
        assert_eq!(ranges(&buf, &re), [0..5, 12..18]);
        let empty = SearchQuery { regex: true, ..q("x*") };
        assert_eq!(ranges(&buf, &empty), [9..10]);
        let bad = SearchQuery { regex: true, ..q("(") };
        assert!(matches!(find_all(&buf, &bad), Err(SearchError::InvalidRegex(_))));
    }

    #[test]
    fn replace_all_is_one_undo_step() {
        let mut doc = Document::from_text("a1 b22 c333");
        let re = SearchQuery { regex: true, ..q(r"([a-z])(\d+)") };
        assert_eq!(replace_all(&mut doc, &re, "$2$1").unwrap(), 3);
        assert_eq!(doc.buffer().text(), "1a 22b 333c");
        assert_eq!(replace_all(&mut doc, &q("2"), "$").unwrap(), 2);
        assert_eq!(doc.buffer().text(), "1a $$b 333c");
        doc.undo();
        assert_eq!(doc.buffer().text(), "1a 22b 333c");
        doc.undo();
        assert_eq!(doc.buffer().text(), "a1 b22 c333");
        assert_eq!(replace_all(&mut doc, &q("zzz"), "y").unwrap(), 0);
    }

    /// Several windows, matches across window ends and piece boundaries,
    /// and the same results on a mapped buffer as on a rope.
    #[test]
    fn windows_give_the_same_matches_as_one_pass() {
        let line = "lorem ipsum needle dolor sit amet, consectetur needle\n";
        let mut text = line.repeat(3 * WINDOW / line.len());
        // A very long line, so a window end falls mid-line.
        text.push_str(&"needle ".repeat(2 * EXTEND / 7));
        text.push_str("\nend needle");
        let rope = RopeBuffer::from_text(&text);
        let mut mapped = MappedBuffer::from_text(&text);
        let mid = text.len() / 2;
        mapped.apply(&Edit::insert(mid, "X", EditOrigin::User, Default::default()));
        mapped.apply(&Edit::delete(mid..mid + 1, EditOrigin::User, Default::default()));
        let expected: Vec<_> = text.match_indices("needle").map(|(i, m)| i..i + m.len()).collect();
        assert_eq!(ranges(&rope, &q("needle")), expected);
        assert_eq!(ranges(&mapped, &q("needle")), expected);
        let re = SearchQuery { regex: true, ..q(r"ne+dle\n") };
        assert_eq!(ranges(&mapped, &re).len(), text.matches("needle\n").count());
    }

    #[test]
    fn spawned_search_streams_and_cancels() {
        let text = "x needle\n".repeat(WINDOW / 4);
        let buf: Arc<dyn TextBuffer> = Arc::new(RopeBuffer::from_text(&text));
        let job = spawn_search(buf.clone(), &q("needle")).unwrap();
        let (mut found, mut progress, mut done) = (0, 0, None);
        for event in job.events.iter() {
            match event {
                SearchEvent::Matches(m) => found += m.len(),
                SearchEvent::Progress { searched, total } => {
                    assert!(searched > progress && total == text.len());
                    progress = searched;
                }
                SearchEvent::Done { matches, cancelled } => done = Some((matches, cancelled)),
            }
        }
        assert_eq!(found, WINDOW / 4);
        assert_eq!(progress, text.len());
        assert_eq!(done, Some((WINDOW / 4, false)));

        let job = spawn_search(buf, &q("needle")).unwrap();
        job.cancel();
        let last = job.events.iter().last();
        assert!(matches!(last, Some(SearchEvent::Done { .. })));

        assert!(spawn_search(Arc::new(RopeBuffer::default()), &SearchQuery { regex: true, ..q("[") }).is_err());
    }
}
