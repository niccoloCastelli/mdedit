//! Text motions and smart Enter on the source, for View-mode editing.

use std::ops::Range;

use mdedit_core::TextBuffer;
use unicode_segmentation::UnicodeSegmentation;

/// Offset of the grapheme boundary before `pos` (a CRLF counts as one).
pub(crate) fn prev_grapheme(buf: &dyn TextBuffer, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let line = buf.byte_to_line(pos);
    let start = buf.line_to_byte(line);
    if pos == start {
        // Step over the previous line ending.
        let prev = buf.line_to_byte(line - 1);
        return prev + buf.line(line - 1).len();
    }
    let text = buf.slice(start..pos);
    let last = text.grapheme_indices(true).next_back().map_or(0, |(i, _)| i);
    start + last
}

/// Offset of the grapheme boundary after `pos`.
pub(crate) fn next_grapheme(buf: &dyn TextBuffer, pos: usize) -> usize {
    let len = buf.len_bytes();
    if pos >= len {
        return len;
    }
    let line = buf.byte_to_line(pos);
    let start = buf.line_to_byte(line);
    let content = buf.line(line);
    let end = start + content.len();
    if pos >= end {
        // At the line ending: the next line's start.
        let lines = buf.len_lines().unwrap_or(line + 1);
        return if line + 1 < lines { buf.line_to_byte(line + 1) } else { len };
    }
    let rest = &content[pos - start..];
    pos + rest.graphemes(true).next().map_or(rest.len(), str::len)
}

fn is_word(s: &str) -> bool {
    s.chars().any(|c| c.is_alphanumeric() || c == '_')
}

/// Start of the word before `pos` (Ctrl+Left, Ctrl+Backspace).
pub(crate) fn word_left(buf: &dyn TextBuffer, pos: usize) -> usize {
    let line = buf.byte_to_line(pos);
    let start = buf.line_to_byte(line);
    if pos == start {
        return prev_grapheme(buf, pos);
    }
    let text = buf.slice(start..pos);
    let words: Vec<(usize, &str)> = text.split_word_bound_indices().collect();
    let mut i = words.len();
    while i > 0 && !is_word(words[i - 1].1) {
        i -= 1;
    }
    if i == 0 {
        return start;
    }
    start + words[i - 1].0
}

/// End of the word after `pos` (Ctrl+Right, Ctrl+Delete).
pub(crate) fn word_right(buf: &dyn TextBuffer, pos: usize) -> usize {
    let line = buf.byte_to_line(pos);
    let start = buf.line_to_byte(line);
    let content = buf.line(line);
    if pos >= start + content.len() {
        return next_grapheme(buf, pos);
    }
    let rest = &content[pos - start..];
    let mut offset = 0;
    let mut seen_word = false;
    for w in rest.split_word_bounds() {
        if is_word(w) {
            seen_word = true;
        } else if seen_word {
            break;
        }
        offset += w.len();
    }
    pos + offset
}

/// The word (or run of non-word characters) around `pos`.
pub(crate) fn word_at(buf: &dyn TextBuffer, pos: usize) -> Range<usize> {
    let line = buf.byte_to_line(pos);
    let start = buf.line_to_byte(line);
    let content = buf.line(line);
    let mut at = start;
    for w in content.split_word_bounds() {
        let end = at + w.len();
        if pos < end || end == start + content.len() {
            return at..end;
        }
        at = end;
    }
    pos..pos
}

/// The line holding `pos`, without its ending.
pub(crate) fn line_range(buf: &dyn TextBuffer, pos: usize) -> Range<usize> {
    let line = buf.byte_to_line(pos);
    let start = buf.line_to_byte(line);
    start..start + buf.line(line).len()
}

/// Container markers at the start of a line: indentation, `>` quotes, a
/// list marker and a task box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prefix {
    /// Bytes of the line taken by markers.
    pub len: usize,
    /// What the next line starts with to stay in the same containers.
    pub continuation: String,
    /// Whether there is a list marker or quote at all.
    pub container: bool,
}

pub(crate) fn line_prefix(line: &str) -> Prefix {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut cont = String::new();
    let mut container = false;
    loop {
        let indent = bytes[i..].iter().take_while(|&&b| b == b' ').count();
        let after = i + indent;
        if bytes.get(after) == Some(&b'>') {
            cont.push_str(&line[i..=after]);
            i = after + 1;
            if bytes.get(i) == Some(&b' ') {
                cont.push(' ');
                i += 1;
            }
            container = true;
            continue;
        }
        // Bullet marker.
        if let Some(&m) = bytes.get(after)
            && matches!(m, b'-' | b'*' | b'+')
            && matches!(bytes.get(after + 1), Some(b' ') | None)
        {
            let spaces = bytes[after + 1..].iter().take_while(|&&b| b == b' ').count().max(1);
            cont.push_str(&line[i..after]);
            cont.push(m as char);
            cont.push_str(&" ".repeat(spaces));
            i = (after + 1 + spaces).min(line.len());
            container = true;
            task(line, &mut i, &mut cont);
            break;
        }
        // Ordered marker.
        let digits = bytes[after..].iter().take_while(|b| b.is_ascii_digit()).count();
        if (1..=9).contains(&digits)
            && let Some(&d) = bytes.get(after + digits)
            && matches!(d, b'.' | b')')
            && matches!(bytes.get(after + digits + 1), Some(b' ') | None)
        {
            let n: u64 = line[after..after + digits].parse().unwrap_or(0);
            let spaces = bytes[after + digits + 1..].iter().take_while(|&&b| b == b' ').count().max(1);
            cont.push_str(&line[i..after]);
            cont.push_str(&(n + 1).to_string());
            cont.push(d as char);
            cont.push_str(&" ".repeat(spaces));
            i = (after + digits + 1 + spaces).min(line.len());
            container = true;
            task(line, &mut i, &mut cont);
            break;
        }
        // Plain indentation inside a container keeps going.
        if container && indent > 0 {
            cont.push_str(&line[i..after]);
            i = after;
        }
        break;
    }
    Prefix { len: i, continuation: cont, container }
}

fn task(line: &str, i: &mut usize, cont: &mut String) {
    let rest = &line[*i..];
    if rest.len() >= 3
        && matches!(&rest[..3], "[ ]" | "[x]" | "[X]")
        && matches!(rest.as_bytes().get(3), Some(b' ') | None)
    {
        cont.push_str("[ ] ");
        *i = (*i + 4).min(line.len());
    }
}

/// What Enter does at a cursor in a paragraph-like block: continue a list
/// or quote, end it on an empty item, or start a new paragraph.
/// Returns the range to replace, the text and the caret after.
pub(crate) fn enter(
    buf: &dyn TextBuffer,
    sel: Range<usize>,
    eol: &str,
    in_code: bool,
) -> (Range<usize>, String, usize) {
    let line = line_range(buf, sel.start);
    let text = buf.slice(line.clone());
    if in_code {
        let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let insert = format!("{eol}{indent}");
        return (sel.clone(), insert.clone(), sel.start + insert.len());
    }
    let prefix = line_prefix(&text);
    if prefix.container {
        if text[prefix.len..].trim().is_empty() && sel.end <= line.end {
            // Empty item: end the list (or quote) with a blank line and
            // start a paragraph after it.
            return (line.start..line.end, eol.to_owned(), line.start + eol.len());
        }
        let insert = format!("{eol}{}", prefix.continuation);
        return (sel.clone(), insert.clone(), sel.start + insert.len());
    }
    let insert = format!("{eol}{eol}");
    (sel.clone(), insert.clone(), sel.start + insert.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdedit_core::RopeBuffer;

    #[test]
    fn graphemes_and_line_endings() {
        let buf = RopeBuffer::from_text("aé\r\nb");
        assert_eq!(next_grapheme(&buf, 1), 3);
        assert_eq!(next_grapheme(&buf, 3), 5, "CRLF is one step");
        assert_eq!(prev_grapheme(&buf, 5), 3);
        assert_eq!(prev_grapheme(&buf, 3), 1);
    }

    #[test]
    fn words() {
        let buf = RopeBuffer::from_text("hello, big world");
        assert_eq!(word_left(&buf, 16), 11);
        assert_eq!(word_left(&buf, 11), 7);
        assert_eq!(word_right(&buf, 0), 5);
        assert_eq!(word_right(&buf, 5), 10);
        assert_eq!(word_at(&buf, 8), 7..10);
    }

    #[test]
    fn prefixes() {
        assert_eq!(line_prefix("- item").continuation, "- ");
        assert_eq!(line_prefix("  9. x").continuation, "  10. ");
        assert_eq!(line_prefix("> - [x] done").continuation, "> - [ ] ");
        assert_eq!(line_prefix("> quote").continuation, "> ");
        assert!(!line_prefix("plain").container);
        assert!(!line_prefix("-not a list").container);
    }

    #[test]
    fn enter_continues_and_ends_lists() {
        let buf = RopeBuffer::from_text("- one\n- ");
        assert_eq!(enter(&buf, 5..5, "\n", false), (5..5, "\n- ".into(), 8));
        assert_eq!(enter(&buf, 8..8, "\n", false), (6..8, "\n".into(), 7));
        let buf = RopeBuffer::from_text("para");
        assert_eq!(enter(&buf, 4..4, "\n", false), (4..4, "\n\n".into(), 6));
        let buf = RopeBuffer::from_text("    code");
        assert_eq!(enter(&buf, 8..8, "\n", true), (8..8, "\n    ".into(), 13));
    }
}
