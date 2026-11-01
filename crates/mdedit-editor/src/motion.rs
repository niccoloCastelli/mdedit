//! Logical cursor motions on the buffer text (graphemes, words, lines).
//! Visual motions (up/down through wrapped rows, page) live in the widget
//! because they need shaped lines.

use std::ops::Range;

use mdedit_core::TextBuffer;
use unicode_segmentation::UnicodeSegmentation;

/// Number of lines, treating "still indexing" as empty.
pub(crate) fn line_count(buf: &dyn TextBuffer) -> usize {
    buf.len_lines().unwrap_or(1).max(1)
}

/// Byte range of line `line` without its line ending.
pub(crate) fn line_range(buf: &dyn TextBuffer, line: usize) -> Range<usize> {
    let start = buf.line_to_byte(line);
    start..start + buf.line(line).len()
}

/// Line `line` including its line ending (for line selection and deletion).
pub(crate) fn line_range_full(buf: &dyn TextBuffer, line: usize) -> Range<usize> {
    let start = buf.line_to_byte(line);
    let end = if line + 1 < line_count(buf) { buf.line_to_byte(line + 1) } else { buf.len_bytes() };
    start..end
}

/// Line index and the column (bytes from line start) of `pos`.
pub(crate) fn line_col(buf: &dyn TextBuffer, pos: usize) -> (usize, usize) {
    let pos = pos.min(buf.len_bytes());
    let line = buf.byte_to_line(pos);
    let start = buf.line_to_byte(line);
    // A position inside a CRLF pair (or past a line's text) clamps to the
    // end of that line's text.
    let len = buf.line(line).len();
    (line, (pos - start).min(len))
}

pub(crate) fn next_grapheme(buf: &dyn TextBuffer, pos: usize) -> usize {
    let (line, col) = line_col(buf, pos);
    let text = buf.line(line);
    if col >= text.len() {
        return if line + 1 < line_count(buf) { buf.line_to_byte(line + 1) } else { buf.len_bytes() };
    }
    let next = text[col..].graphemes(true).next().map_or(0, str::len);
    buf.line_to_byte(line) + col + next
}

pub(crate) fn prev_grapheme(buf: &dyn TextBuffer, pos: usize) -> usize {
    let (line, col) = line_col(buf, pos);
    if col == 0 {
        return if line == 0 { 0 } else { line_range(buf, line - 1).end };
    }
    let text = buf.line(line);
    let prev = text[..col].graphemes(true).next_back().map_or(0, str::len);
    buf.line_to_byte(line) + col - prev
}

fn is_word(s: &str) -> bool {
    s.chars().any(char::is_alphanumeric)
}

/// Start of the word before `pos` (Ctrl+Left); at a line start, the end of
/// the previous line.
pub(crate) fn prev_word(buf: &dyn TextBuffer, pos: usize) -> usize {
    let (line, col) = line_col(buf, pos);
    if col == 0 {
        return prev_grapheme(buf, pos);
    }
    let text = buf.line(line);
    let start = text[..col]
        .split_word_bound_indices()
        .filter(|(_, w)| is_word(w))
        .last()
        .map_or(0, |(i, _)| i);
    buf.line_to_byte(line) + start
}

/// End of the word after `pos` (Ctrl+Right); at a line end, the start of
/// the next line.
pub(crate) fn next_word(buf: &dyn TextBuffer, pos: usize) -> usize {
    let (line, col) = line_col(buf, pos);
    let text = buf.line(line);
    if col >= text.len() {
        return next_grapheme(buf, pos);
    }
    let end = text[col..]
        .split_word_bound_indices()
        .find(|(_, w)| is_word(w))
        .map_or(text.len() - col, |(i, w)| i + w.len());
    buf.line_to_byte(line) + col + end
}

/// The word (or run of punctuation / whitespace) around `pos`, for double
/// click.
pub(crate) fn word_at(buf: &dyn TextBuffer, pos: usize) -> Range<usize> {
    let (line, col) = line_col(buf, pos);
    let text = buf.line(line);
    let base = buf.line_to_byte(line);
    let mut best = col..col;
    for (i, w) in text.split_word_bound_indices() {
        let r = i..i + w.len();
        // Prefer the word to the right of the click, but take a word that
        // ends exactly at the click if nothing starts there.
        if r.start <= col && col < r.end {
            best = r;
            break;
        }
        if r.end == col && is_word(w) {
            best = r;
        }
    }
    base + best.start..base + best.end
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdedit_core::RopeBuffer;

    #[test]
    fn graphemes_cross_lines_and_crlf() {
        let buf = RopeBuffer::from_text("aé\r\nb");
        assert_eq!(next_grapheme(&buf, 0), 1);
        assert_eq!(next_grapheme(&buf, 1), 3);
        assert_eq!(next_grapheme(&buf, 3), 5);
        assert_eq!(prev_grapheme(&buf, 5), 3);
        assert_eq!(prev_grapheme(&buf, 3), 1);
        assert_eq!(next_grapheme(&buf, 6), 6);
        assert_eq!(prev_grapheme(&buf, 0), 0);
    }

    #[test]
    fn words() {
        let buf = RopeBuffer::from_text("hello, big world\nnext");
        assert_eq!(next_word(&buf, 0), 5);
        assert_eq!(next_word(&buf, 5), 10);
        assert_eq!(next_word(&buf, 16), 17);
        assert_eq!(prev_word(&buf, 16), 11);
        assert_eq!(prev_word(&buf, 11), 7);
        assert_eq!(prev_word(&buf, 17), 16);
        assert_eq!(word_at(&buf, 8), 7..10);
        assert_eq!(word_at(&buf, 10), 10..11);
        assert_eq!(word_at(&buf, 16), 11..16);
    }

    #[test]
    fn line_ranges() {
        let buf = RopeBuffer::from_text("ab\r\ncd\n");
        assert_eq!(line_range(&buf, 0), 0..2);
        assert_eq!(line_range_full(&buf, 0), 0..4);
        assert_eq!(line_range_full(&buf, 1), 4..7);
        assert_eq!(line_range_full(&buf, 2), 7..7);
        assert_eq!(line_col(&buf, 3), (0, 2));
    }
}
