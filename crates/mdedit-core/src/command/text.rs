//! Helpers shared by the commands: markdown line prefixes, the lines a
//! selection touches, and an edit collector that maps offsets through the
//! edits it holds.

use std::ops::Range;

use super::CommandResult;
use crate::buffer::TextBuffer;
use crate::edit::{Edit, EditOrigin, TxnId};
use crate::selection::Selection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListKind {
    /// `-`, `*` or `+`.
    Bullet(u8),
    /// `1.` or `1)`.
    Ordered { number: u64, delim: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListMarker {
    pub kind: ListKind,
    /// The marker plus the one space after it, as byte offsets in the line.
    pub range: Range<usize>,
    /// `[ ] ` / `[x] ` after the marker: checked, and its range.
    pub task: Option<(bool, Range<usize>)>,
}

impl ListMarker {
    /// End of the marker and task box: where the item's text starts.
    pub fn end(&self) -> usize {
        self.task.as_ref().map_or(self.range.end, |t| t.1.end)
    }

    pub fn number_digits(&self) -> usize {
        match self.kind {
            ListKind::Bullet(_) => 0,
            ListKind::Ordered { number, .. } => number.to_string().len(),
        }
    }
}

/// The block-level prefix of one source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineInfo {
    /// End of the blockquote prefix (`> > `), 0 when not quoted.
    pub quote: usize,
    /// End of the indentation after the quote prefix.
    pub indent: usize,
    pub list: Option<ListMarker>,
    /// ATX heading level and the range of `## `.
    pub heading: Option<(u8, Range<usize>)>,
    /// Where the line's inline text starts.
    pub content: usize,
}

impl LineInfo {
    pub fn parse(line: &str) -> Self {
        let b = line.as_bytes();
        let mut quote = 0;
        loop {
            let mut i = quote;
            while i < b.len() && b[i] == b' ' && i - quote < 3 {
                i += 1;
            }
            if b.get(i) != Some(&b'>') {
                break;
            }
            i += 1;
            if matches!(b.get(i), Some(b' ' | b'\t')) {
                i += 1;
            }
            quote = i;
        }
        let mut indent = quote;
        while matches!(b.get(indent), Some(b' ' | b'\t')) {
            indent += 1;
        }
        let list = if is_thematic_break(&line[indent..]) { None } else { parse_list_marker(line, indent) };
        let after_list = list.as_ref().map_or(indent, ListMarker::end);
        let heading = parse_heading(line, after_list);
        let content = heading.as_ref().map_or(after_list, |h| h.1.end);
        Self { quote, indent, list, heading, content }
    }

    /// Indentation width in columns (tabs to the next multiple of 4).
    pub fn indent_cols(&self, line: &str) -> usize {
        columns(&line[self.quote..self.indent])
    }
}

pub(crate) fn columns(ws: &str) -> usize {
    ws.bytes().fold(0, |c, b| if b == b'\t' { c + 4 - c % 4 } else { c + 1 })
}

pub(crate) fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_thematic_break(rest: &str) -> bool {
    let mut chars = rest.chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else { return false };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut n = 1;
    for c in chars {
        if c != first {
            return false;
        }
        n += 1;
    }
    n >= 3
}

fn parse_list_marker(line: &str, at: usize) -> Option<ListMarker> {
    let b = line.as_bytes();
    let mut i = at;
    let kind = match *b.get(i)? {
        c @ (b'-' | b'*' | b'+') => {
            i += 1;
            ListKind::Bullet(c)
        }
        b'0'..=b'9' => {
            while i < b.len() && b[i].is_ascii_digit() && i - at < 9 {
                i += 1;
            }
            let delim = *b.get(i)?;
            if delim != b'.' && delim != b')' {
                return None;
            }
            let number = line[at..i].parse().ok()?;
            i += 1;
            ListKind::Ordered { number, delim }
        }
        _ => return None,
    };
    match b.get(i) {
        None => {}
        Some(b' ' | b'\t') => i += 1,
        Some(_) => return None,
    }
    let range = at..i;
    let task = (b.len() >= i + 3
        && b[i] == b'['
        && b[i + 2] == b']'
        && matches!(b[i + 1], b' ' | b'x' | b'X')
        && matches!(b.get(i + 3), None | Some(b' ' | b'\t')))
    .then(|| (b[i + 1] != b' ', i..(i + 4).min(b.len())));
    Some(ListMarker { kind, range, task })
}

fn parse_heading(line: &str, at: usize) -> Option<(u8, Range<usize>)> {
    let b = line.as_bytes();
    let n = b[at..].iter().take_while(|&&c| c == b'#').count();
    if n == 0 || n > 6 {
        return None;
    }
    match b.get(at + n) {
        None => Some((n as u8, at..at + n)),
        Some(b' ' | b'\t') => Some((n as u8, at..at + n + 1)),
        Some(_) => None,
    }
}

/// A setext underline (`===` or `---`), ignoring the quote prefix.
pub(crate) fn setext_level(line: &str) -> Option<u8> {
    let info = LineInfo::parse(line);
    let rest = line[info.quote..].trim();
    if info.indent_cols(line) > 3 || rest.is_empty() {
        return None;
    }
    if rest.bytes().all(|c| c == b'=') {
        Some(1)
    } else if rest.bytes().all(|c| c == b'-') {
        Some(2)
    } else {
        None
    }
}

/// First and last line a selection touches. A non-empty selection ending at
/// the very start of a line does not include that line.
pub(crate) fn touched_lines(buf: &dyn TextBuffer, range: &Range<usize>) -> (usize, usize) {
    let first = buf.byte_to_line(range.start);
    let mut last = buf.byte_to_line(range.end);
    if last > first && buf.line_to_byte(last) == range.end {
        last -= 1;
    }
    (first, last)
}

pub(crate) fn line_count(buf: &dyn TextBuffer) -> usize {
    buf.len_lines().unwrap_or_else(|| buf.byte_to_line(buf.len_bytes()) + 1)
}

/// Byte range of line `idx` without its line ending.
pub(crate) fn line_range(buf: &dyn TextBuffer, idx: usize) -> Range<usize> {
    let start = buf.line_to_byte(idx);
    start..start + buf.line(idx).len()
}

/// Whether `s` looks like a URL worth turning into a link.
pub(crate) fn is_url(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && !s.contains(char::is_whitespace)
        && ["http://", "https://", "mailto:", "ftp://", "www."].iter().any(|p| s.starts_with(p))
}

/// A link or image destination, in angle brackets when it has spaces.
pub(crate) fn link_dest(url: &str) -> String {
    if url.contains(' ') || url.contains(['(', ')']) { format!("<{url}>") } else { url.to_string() }
}

/// Edits collected against the *original* text. They must not overlap; two
/// insertions at the same offset are merged in the order given.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    edits: Vec<(Range<usize>, String)>,
}

impl Changes {
    pub fn replace(&mut self, range: Range<usize>, text: impl Into<String>) {
        let text = text.into();
        if range.is_empty() && text.is_empty() {
            return;
        }
        if range.is_empty()
            && let Some(prev) = self.edits.iter_mut().find(|(r, _)| r.is_empty() && r.start == range.start)
        {
            prev.1.push_str(&text);
            return;
        }
        self.edits.push((range, text));
    }

    pub fn insert(&mut self, at: usize, text: impl Into<String>) {
        self.replace(at..at, text);
    }

    pub fn delete(&mut self, range: Range<usize>) {
        self.replace(range, String::new());
    }

    /// Replace `range` (currently holding `old`) with `new`, trimming the
    /// common prefix and suffix so unchanged text keeps its anchors.
    pub fn replace_minimal(&mut self, range: Range<usize>, old: &str, new: &str) {
        if old == new {
            return;
        }
        let mut pre = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(pre) || !new.is_char_boundary(pre) {
            pre -= 1;
        }
        let max_suf = old.len().min(new.len()) - pre;
        let mut suf = old.bytes().rev().zip(new.bytes().rev()).take(max_suf).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(old.len() - suf) || !new.is_char_boundary(new.len() - suf) {
            suf -= 1;
        }
        self.replace(range.start + pre..range.end - suf, &new[pre..new.len() - suf]);
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    fn sorted(&self) -> Vec<&(Range<usize>, String)> {
        let mut v: Vec<_> = self.edits.iter().collect();
        v.sort_by_key(|(r, _)| (r.start, r.end));
        v
    }

    /// Map an offset in the original text to the text after all edits.
    /// Offsets at an insertion point or inside a replaced range go to the
    /// end of the new text if `right`, else its start.
    pub fn map(&self, offset: usize, right: bool) -> usize {
        let mut delta: isize = 0;
        for (r, t) in self.sorted() {
            if offset < r.start {
                break;
            }
            if offset > r.end || (offset == r.end && !r.is_empty()) {
                delta += t.len() as isize - r.len() as isize;
                continue;
            }
            let start = (r.start as isize + delta) as usize;
            return if right { start + t.len() } else { start };
        }
        (offset as isize + delta) as usize
    }

    /// Map a selection: its start sticks right and its end sticks left, so
    /// text wrapped around it stays outside it.
    pub fn map_selection(&self, sel: Selection) -> Selection {
        if sel.is_empty() {
            return Selection::cursor(self.map(sel.head, true));
        }
        let r = sel.range();
        let start = self.map(r.start, true);
        let end = self.map(r.end, false).max(start);
        if sel.anchor <= sel.head {
            Selection { anchor: start, head: end }
        } else {
            Selection { anchor: end, head: start }
        }
    }

    /// The edits, last offset first, so applying them one after another
    /// (each to the result of the previous) gives the same text as applying
    /// them all to the original.
    pub fn finish(self, selection: Selection) -> CommandResult {
        let mut edits = self.edits;
        edits.sort_by_key(|(r, _)| std::cmp::Reverse((r.start, r.end)));
        let edits = edits
            .into_iter()
            .map(|(range, text)| Edit { range, text, origin: EditOrigin::Command, txn: TxnId::default() })
            .collect();
        CommandResult { edits, selection }
    }

    /// [`Self::finish`] with the selection mapped through the edits.
    pub fn finish_mapped(self, selection: Selection) -> CommandResult {
        let sel = self.map_selection(selection);
        self.finish(sel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_info_prefixes() {
        let i = LineInfo::parse("> - [x] # hi");
        assert_eq!(i.quote, 2);
        assert_eq!(i.indent, 2);
        let m = i.list.unwrap();
        assert_eq!(m.kind, ListKind::Bullet(b'-'));
        assert_eq!(m.task, Some((true, 4..8)));
        assert_eq!(i.heading, Some((1, 8..10)));
        assert_eq!(i.content, 10);

        let i = LineInfo::parse("   12) item");
        assert_eq!(i.list.unwrap().kind, ListKind::Ordered { number: 12, delim: b')' });
        assert_eq!(i.content, 7);

        assert!(LineInfo::parse("**bold**").list.is_none());
        assert!(LineInfo::parse("- - -").list.is_none());
        assert!(LineInfo::parse("#hashtag").heading.is_none());
        assert_eq!(LineInfo::parse(">>quote").quote, 2);
    }

    #[test]
    fn changes_map_and_order() {
        let mut c = Changes::default();
        c.insert(0, "**");
        c.insert(3, "**");
        assert_eq!(c.map(0, true), 2);
        assert_eq!(c.map(3, false), 5);
        assert_eq!(c.map(4, false), 8);
        let r = c.finish(Selection::cursor(0));
        assert_eq!(r.edits[0].range, 3..3);
        assert_eq!(r.edits[1].range, 0..0);
    }

    #[test]
    fn replace_minimal_trims() {
        let mut c = Changes::default();
        c.replace_minimal(10..17, "| a | b", "| a  | b");
        assert_eq!(c.edits, vec![(14..14, " ".to_string())]);
    }
}
