//! Line-local markdown styling for Edit mode (parsing workstream).
//!
//! The Edit-mode widget asks for the styled ranges of each *visible* line.
//! Two entry points:
//!
//! - [`highlight_lines`] for parsed documents: the block index says which
//!   block each line belongs to (fenced or indented code, HTML block, setext
//!   heading, table), so styling is right even when the viewport starts in
//!   the middle of a code block.
//! - [`highlight_line`] for large files (no block index, ARCHITECTURE 3.3):
//!   the line-local tokenizer, with the fence and HTML-block state carried
//!   from the previous visible line in a [`LineState`].
//!
//! Inline styling (emphasis, code spans, links, inline HTML) is line-local in
//! both cases. Token names map one-to-one to the `syntax-*` design tokens
//! (DESIGN_SYSTEM 2.4).

use std::ops::Range;

use crate::buffer::TextBuffer;
use crate::parse::{Block, BlockIndex, BlockKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxToken {
    Marker,
    Heading(u8),
    Emphasis,
    Strong,
    Strikethrough,
    Link,
    Url,
    Code,
    Html,
    Math,
}

/// A styled byte range **relative to the start of the line**. Ranges may
/// overlap (strong inside a link); the widget applies them in order. They
/// are sorted by start, and an enclosing range comes before the ranges
/// inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledRange {
    pub range: Range<usize>,
    pub token: SyntaxToken,
}

/// How an open HTML block ends (CommonMark 4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlEnd {
    /// Types 6 and 7: the block ends before the next blank line.
    BlankLine,
    /// Types 1-5: the block ends with the line containing this text
    /// (compared ASCII case-insensitively).
    Contains(&'static str),
}

/// State carried from one line to the next (open code fence, HTML block).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineState {
    /// The opening fence run (e.g. ```` ``` ```` or `~~~~`) while inside a
    /// fenced code block.
    pub in_fence: Option<String>,
    /// Indentation of the opening fence inside its container, so the
    /// closing fence can be recognised.
    pub fence_indent: usize,
    /// Number of `>` markers in front of the opening fence.
    pub fence_quotes: usize,
    /// Set while inside an HTML block.
    pub in_html: Option<HtmlEnd>,
}

/// Lines longer than this only get block-level styling (markers, headings,
/// code), so a minified one-line file can't make a frame slow.
const MAX_INLINE_LEN: usize = 16 * 1024;

/// How far back [`line_state`] replays a container block (list, quote) to
/// find the state of a line in its middle.
const MAX_REPLAY_LINES: usize = 256;

/// Styled ranges for one line, using and updating the carried `state`.
/// `line` is the line without its line ending.
pub fn highlight_line(line: &str, state: &mut LineState) -> Vec<StyledRange> {
    let mut out = Out::default();
    tokenize(line, state, Mode::Auto, &mut out);
    out.finish()
}

/// Styled ranges for each line in `lines` of a parsed document (one `Vec`
/// per line; lines past the end are left out).
pub fn highlight_lines(index: &BlockIndex, buf: &dyn TextBuffer, lines: Range<usize>) -> Vec<Vec<StyledRange>> {
    let total = buf.len_lines().unwrap_or(0);
    let lines = lines.start.min(total)..lines.end.min(total);
    let mut result = Vec::with_capacity(lines.len());
    let mut state = LineState::default();
    // The block of the previous line.
    let mut current: Option<LineBlock> = None;
    let mut in_gap = false;
    for l in lines {
        let text = buf.line(l);
        let found = match &current {
            Some(c) if l <= c.last => Some(c.clone()),
            _ => block_of_line(index, buf, l, text.len()),
        };
        let mut out = Out::default();
        match found {
            None => {
                if !in_gap {
                    state = LineState::default();
                    in_gap = true;
                }
                current = None;
                tokenize(&text, &mut state, Mode::Auto, &mut out);
            }
            Some(lb) => {
                if current.as_ref().is_none_or(|c| c.index != lb.index) {
                    state = if l == lb.first { LineState::default() } else { replay(&lb.block, buf, lb.first, l) };
                }
                in_gap = false;
                style_block_line(&lb, l, &text, &mut state, &mut out);
                current = Some(lb);
            }
        }
        result.push(out.finish());
    }
    result
}

/// The tokenizer state at the start of `line` of a parsed document, for a
/// caller that then styles lines one by one with [`highlight_line`].
pub fn line_state(index: &BlockIndex, buf: &dyn TextBuffer, line: usize) -> LineState {
    if buf.len_lines().is_none_or(|n| line >= n) {
        return LineState::default();
    }
    let len = buf.line(line).len();
    match block_of_line(index, buf, line, len) {
        Some(lb) if lb.first < line => replay(&lb.block, buf, lb.first, line),
        _ => LineState::default(),
    }
}

/// A block seen from the lines it covers.
#[derive(Clone)]
struct LineBlock {
    index: usize,
    block: Block,
    first: usize,
    last: usize,
}

/// The block containing line `l` (whose text is `len` bytes long).
fn block_of_line(index: &BlockIndex, buf: &dyn TextBuffer, l: usize, len: usize) -> Option<LineBlock> {
    let line_end = buf.line_to_byte(l) + len;
    let i = index.block_at(line_end)?;
    let block = index.get(i)?;
    let last = buf.byte_to_line(block.bytes.end);
    if last < l {
        return None;
    }
    let first = buf.byte_to_line(block.bytes.start);
    Some(LineBlock { index: i, block, first, last })
}

/// State at line `l`, inside `block` which starts at line `first`.
fn replay(block: &Block, buf: &dyn TextBuffer, first: usize, l: usize) -> LineState {
    let mut state = LineState::default();
    let mut sink = Out::default();
    match block.kind {
        BlockKind::CodeBlock { fenced: true, .. } | BlockKind::Math | BlockKind::Mermaid => {
            tokenize(&buf.line(first), &mut state, Mode::Auto, &mut sink);
        }
        BlockKind::BlockQuote | BlockKind::List { .. } | BlockKind::FootnoteDefinition
            if l - first <= MAX_REPLAY_LINES =>
        {
            for k in first..l {
                tokenize(&buf.line(k), &mut state, Mode::Auto, &mut sink);
                sink.ranges.clear();
            }
        }
        _ => {}
    }
    state
}

fn style_block_line(lb: &LineBlock, l: usize, text: &str, state: &mut LineState, out: &mut Out) {
    let body = text.trim_end();
    let lead = leading_ws(text);
    match &lb.block.kind {
        BlockKind::Heading { level } if lb.first < lb.last => {
            // Setext heading: the last line is the underline.
            if l == lb.last {
                out.push(lead..body.len(), SyntaxToken::Marker);
            } else if lead < body.len() {
                out.push(lead..body.len(), SyntaxToken::Heading(*level));
                inline(&text[lead..body.len()], lead, out);
            }
        }
        BlockKind::CodeBlock { fenced: false, .. } => {
            let start = lead.min(4);
            if start < body.len() {
                out.push(start..text.len(), SyntaxToken::Code);
            }
        }
        BlockKind::HtmlBlock => {
            if lead < body.len() {
                out.push(lead..body.len(), SyntaxToken::Html);
            }
        }
        BlockKind::ThematicBreak => out.push(lead..body.len(), SyntaxToken::Marker),
        BlockKind::Paragraph => tokenize(text, state, Mode::Inline, out),
        BlockKind::Table => tokenize(text, state, Mode::Table, out),
        _ => tokenize(text, state, Mode::Auto, out),
    }
}

#[derive(Default)]
struct Out {
    ranges: Vec<StyledRange>,
}

impl Out {
    fn push(&mut self, range: Range<usize>, token: SyntaxToken) {
        if range.start < range.end {
            self.ranges.push(StyledRange { range, token });
        }
    }

    fn finish(mut self) -> Vec<StyledRange> {
        self.ranges.sort_by(|a, b| a.range.start.cmp(&b.range.start).then(b.range.end.cmp(&a.range.end)));
        self.ranges
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Detect block structure from the line itself.
    Auto,
    /// Known paragraph text: inline styling only.
    Inline,
    /// Known table row.
    Table,
}

fn tokenize(line: &str, state: &mut LineState, mode: Mode, out: &mut Out) {
    if state.in_fence.is_some() {
        fence_line(line, state, out);
        return;
    }
    if let Some(end) = state.in_html {
        let body = line.trim_end();
        if body.trim_start().is_empty() {
            if end == HtmlEnd::BlankLine {
                state.in_html = None;
            }
            return;
        }
        out.push(leading_ws(line)..body.len(), SyntaxToken::Html);
        if let HtmlEnd::Contains(pat) = end
            && contains_ignore_case(line, pat)
        {
            state.in_html = None;
        }
        return;
    }
    if line.trim().is_empty() {
        return;
    }
    match mode {
        Mode::Inline => inline(line, 0, out),
        Mode::Table => table_row(line, 0, out),
        Mode::Auto => block_line(line, state, out),
    }
}

/// A line inside an open fence: either the closing fence or code.
fn fence_line(line: &str, state: &mut LineState, out: &mut Out) {
    let b = line.as_bytes();
    let mut pos = 0;
    for _ in 0..state.fence_quotes {
        let p = pos + count_ws(b, pos);
        if b.get(p) != Some(&b'>') {
            break;
        }
        out.push(p..p + 1, SyntaxToken::Marker);
        pos = p + 1;
        if b.get(pos) == Some(&b' ') {
            pos += 1;
        }
    }
    let fence = state.in_fence.as_deref().unwrap_or("```");
    let indent = count_ws(b, pos);
    let start = pos + indent;
    let ch = fence.as_bytes()[0];
    let run = count_byte(b, start, ch);
    if indent <= state.fence_indent + 3 && run >= fence.len() && line[start + run..].trim().is_empty() {
        out.push(start..start + run, SyntaxToken::Marker);
        *state = LineState { in_html: state.in_html, ..LineState::default() };
        return;
    }
    if !line[pos..].trim().is_empty() {
        let code = pos + indent.min(state.fence_indent);
        out.push(code..line.len(), SyntaxToken::Code);
    }
}

/// A line in `Auto` mode: container markers, then the leaf block.
fn block_line(line: &str, state: &mut LineState, out: &mut Out) {
    let b = line.as_bytes();
    let mut pos = 0;
    let mut quotes = 0;
    loop {
        let p = pos + count_ws(b, pos);
        match b.get(p) {
            Some(b'>') => {
                out.push(p..p + 1, SyntaxToken::Marker);
                pos = p + 1;
                if b.get(pos) == Some(&b' ') {
                    pos += 1;
                }
                quotes += 1;
            }
            Some(b'-' | b'+' | b'*') if is_ws_or_end(b, p + 1) => {
                if is_thematic_break(&line[p..]) {
                    out.push(p..line.trim_end().len(), SyntaxToken::Marker);
                    return;
                }
                out.push(p..p + 1, SyntaxToken::Marker);
                pos = p + 1;
                if b.get(pos).is_some_and(|c| *c == b' ' || *c == b'\t') {
                    pos += 1;
                }
                let rest = &line[pos..];
                if (rest.starts_with("[ ]") || rest.starts_with("[x]") || rest.starts_with("[X]"))
                    && is_ws_or_end(b, pos + 3)
                {
                    out.push(pos..pos + 3, SyntaxToken::Marker);
                    pos += 3;
                }
            }
            Some(c) if c.is_ascii_digit() => {
                let digits = b[p..].iter().take_while(|c| c.is_ascii_digit()).count();
                let q = p + digits;
                if digits <= 9 && matches!(b.get(q), Some(b'.' | b')')) && is_ws_or_end(b, q + 1) {
                    out.push(p..q + 1, SyntaxToken::Marker);
                    pos = q + 1;
                    if b.get(pos).is_some_and(|c| *c == b' ' || *c == b'\t') {
                        pos += 1;
                    }
                } else {
                    break;
                }
            }
            _ => break,
        }
    }

    let indent = count_ws(b, pos);
    let p = pos + indent;
    let rest = &line[p..];
    let rest_trimmed = rest.trim_end();
    if rest_trimmed.is_empty() {
        return;
    }
    let end = p + rest_trimmed.len();

    if let Some(run) = fence_open(rest) {
        out.push(p..p + run, SyntaxToken::Marker);
        let info_start = p + run + leading_ws(&rest[run..]);
        out.push(info_start..end, SyntaxToken::Marker);
        state.in_fence = Some(rest[..run].to_string());
        state.fence_indent = indent;
        state.fence_quotes = quotes;
        return;
    }

    let hashes = count_byte(b, p, b'#');
    if (1..=6).contains(&hashes) && is_ws_or_end(b, p + hashes) {
        heading_line(line, p, hashes, end, out);
        return;
    }

    if is_thematic_break(rest) || rest_trimmed.bytes().all(|c| c == b'=') {
        out.push(p..end, SyntaxToken::Marker);
        return;
    }

    if let Some((html_end, open_len)) = html_block_start(rest) {
        out.push(p..end, SyntaxToken::Html);
        let closed = match html_end {
            HtmlEnd::Contains(pat) => contains_ignore_case(&rest[open_len..], pat),
            HtmlEnd::BlankLine => false,
        };
        if !closed {
            state.in_html = Some(html_end);
        }
        return;
    }

    if rest.starts_with('[')
        && let Some(close) = label_end(b, p + 1)
        && b.get(close + 1) == Some(&b':')
    {
        if b.get(p + 1) == Some(&b'^') {
            // Footnote definition: `[^label]:` then ordinary text.
            out.push(p..p + 2, SyntaxToken::Marker);
            out.push(p + 2..close, SyntaxToken::Link);
            out.push(close..close + 2, SyntaxToken::Marker);
            let after = close + 2;
            inline(&line[after..], after, out);
        } else {
            // Link reference definition.
            out.push(p..p + 1, SyntaxToken::Marker);
            out.push(p + 1..close, SyntaxToken::Link);
            out.push(close..close + 2, SyntaxToken::Marker);
            let url = close + 2 + leading_ws(&line[close + 2..]);
            out.push(url..end, SyntaxToken::Url);
        }
        return;
    }

    if rest.starts_with('|') {
        table_row(line, p, out);
        return;
    }

    inline(&line[p..], p, out);
}

/// An ATX heading whose `#` run of length `hashes` starts at `p`; the line
/// content ends at `end`.
fn heading_line(line: &str, p: usize, hashes: usize, end: usize, out: &mut Out) {
    out.push(p..p + hashes, SyntaxToken::Marker);
    let after = p + hashes;
    let mut content_end = end;
    // Heading attributes `{#id .class}` at the end.
    if line[..content_end].ends_with('}')
        && let Some(open) = line[after..content_end].rfind('{')
    {
        let open = after + open;
        if open > after && line.as_bytes()[open - 1].is_ascii_whitespace() {
            out.push(open..content_end, SyntaxToken::Marker);
            content_end = after + line[after..open].trim_end().len();
        }
    }
    // Optional closing sequence of `#`s preceded by a space.
    let trailing = line[after..content_end].bytes().rev().take_while(|c| *c == b'#').count();
    if trailing > 0 {
        let close = content_end - trailing;
        if close == after || line.as_bytes()[close - 1].is_ascii_whitespace() {
            out.push(close..content_end, SyntaxToken::Marker);
            content_end = after + line[after..close].trim_end().len();
        }
    }
    let content_start = after + leading_ws(&line[after..]);
    if content_start < content_end {
        out.push(content_start..content_end, SyntaxToken::Heading(hashes as u8));
        inline(&line[content_start..content_end], content_start, out);
    }
}

/// A table row from byte `from`: pipes are markers, cells get inline
/// styling; the delimiter row is all marker.
fn table_row(line: &str, from: usize, out: &mut Out) {
    let body = &line[from..line.trim_end().len()];
    let start = from + leading_ws(body);
    let row = &line[start..from + body.len()];
    if row.contains('-') && row.bytes().all(|c| matches!(c, b'|' | b'-' | b':' | b' ' | b'\t')) {
        out.push(start..start + row.len(), SyntaxToken::Marker);
        return;
    }
    let b = row.as_bytes();
    let mut cell = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'`' => {
                let run = count_byte(b, i, b'`');
                i = find_backtick_close(b, i + run, run).map_or(i + run, |c| c + run);
            }
            b'|' => {
                inline(&row[cell..i], start + cell, out);
                out.push(start + i..start + i + 1, SyntaxToken::Marker);
                cell = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if cell < b.len() {
        inline(&row[cell..], start + cell, out);
    }
}

struct Delim {
    ch: u8,
    start: usize,
    len: usize,
    orig: usize,
    can_open: bool,
    can_close: bool,
}

struct Bracket {
    pos: usize,
    image: bool,
    active: bool,
}

/// Inline styling of `s`, whose first byte is at `base` in the line.
fn inline(s: &str, base: usize, out: &mut Out) {
    if s.len() > MAX_INLINE_LEN {
        return;
    }
    let b = s.as_bytes();
    let mut delims: Vec<Delim> = Vec::new();
    let mut brackets: Vec<Bracket> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' if b.get(i + 1).is_some_and(u8::is_ascii_punctuation) => i += 2,
            b'`' => {
                let run = count_byte(b, i, b'`');
                if let Some(close) = find_backtick_close(b, i + run, run) {
                    out.push(base + i..base + i + run, SyntaxToken::Marker);
                    out.push(base + i + run..base + close, SyntaxToken::Code);
                    out.push(base + close..base + close + run, SyntaxToken::Marker);
                    i = close + run;
                } else {
                    i += run;
                }
            }
            b'<' => {
                if let Some(end) = autolink(b, i) {
                    out.push(base + i..base + end, SyntaxToken::Url);
                    i = end;
                } else if let Some(end) = inline_html(b, i) {
                    out.push(base + i..base + end, SyntaxToken::Html);
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'!' if b.get(i + 1) == Some(&b'[') => {
                brackets.push(Bracket { pos: i, image: true, active: true });
                i += 2;
            }
            b'[' => {
                brackets.push(Bracket { pos: i, image: false, active: true });
                i += 1;
            }
            b']' => {
                i = close_bracket(b, i, base, &mut brackets, out);
            }
            c @ (b'*' | b'_' | b'~') => {
                let run = count_byte(b, i, c);
                let prev = s[..i].chars().next_back();
                let next = s[i + run..].chars().next();
                let ws = |c: Option<char>| c.is_none_or(char::is_whitespace);
                let punct = |c: Option<char>| c.is_some_and(is_punctuation);
                let left = !ws(next) && (!punct(next) || ws(prev) || punct(prev));
                let right = !ws(prev) && (!punct(prev) || ws(next) || punct(next));
                let (can_open, can_close) = if c == b'_' {
                    (left && (!right || punct(prev)), right && (!left || punct(next)))
                } else {
                    (left, right)
                };
                delims.push(Delim { ch: c, start: i, len: run, orig: run, can_open, can_close });
                i += run;
            }
            _ => i += 1,
        }
    }
    process_emphasis(&mut delims, base, out);
}

/// Handle the `]` at `i`: an inline link, a full reference link or a
/// footnote reference. Returns where scanning continues.
fn close_bracket(b: &[u8], i: usize, base: usize, brackets: &mut Vec<Bracket>, out: &mut Out) -> usize {
    let Some(br) = brackets.pop() else { return i + 1 };
    if !br.active {
        return i + 1;
    }
    let open = if br.image { 2 } else { 1 };
    let text = base + br.pos + open..base + i;
    if let Some(end) = inline_link_tail(b, i + 1) {
        out.push(base + br.pos..text.start, SyntaxToken::Marker);
        out.push(text, SyntaxToken::Link);
        out.push(base + i..base + i + 2, SyntaxToken::Marker);
        out.push(base + i + 2..base + end - 1, SyntaxToken::Url);
        out.push(base + end - 1..base + end, SyntaxToken::Marker);
        if !br.image {
            // Links can't contain links.
            for other in brackets.iter_mut().filter(|o| !o.image) {
                other.active = false;
            }
        }
        return end;
    }
    if b.get(i + 1) == Some(&b'[')
        && let Some(label) = label_end(b, i + 2)
        && label > i + 2
    {
        out.push(base + br.pos..text.start, SyntaxToken::Marker);
        out.push(text, SyntaxToken::Link);
        out.push(base + i..base + i + 2, SyntaxToken::Marker);
        out.push(base + i + 2..base + label, SyntaxToken::Url);
        out.push(base + label..base + label + 1, SyntaxToken::Marker);
        return label + 1;
    }
    if !br.image && b.get(br.pos + 1) == Some(&b'^') && i > br.pos + 2 {
        out.push(base + br.pos..base + br.pos + 2, SyntaxToken::Marker);
        out.push(base + br.pos + 2..base + i, SyntaxToken::Link);
        out.push(base + i..base + i + 1, SyntaxToken::Marker);
    }
    i + 1
}

/// CommonMark's "process emphasis" over one line's delimiter runs.
fn process_emphasis(d: &mut [Delim], base: usize, out: &mut Out) {
    for c in 0..d.len() {
        if !d[c].can_close {
            continue;
        }
        while d[c].len > 0 {
            let found = (0..c).rev().find(|&o| {
                let (op, cl) = (&d[o], &d[c]);
                if op.ch != cl.ch || !op.can_open || op.len == 0 {
                    return false;
                }
                if op.ch == b'~' {
                    return op.len == cl.len && op.len <= 2;
                }
                let multiple_of_three = (op.can_close || cl.can_open)
                    && (op.orig + cl.orig) % 3 == 0
                    && !(op.orig % 3 == 0 && cl.orig % 3 == 0);
                !multiple_of_three
            });
            let Some(o) = found else { break };
            let used = if d[o].ch == b'~' {
                d[o].len
            } else if d[o].len >= 2 && d[c].len >= 2 {
                2
            } else {
                1
            };
            let open_marker = d[o].start + d[o].len - used;
            let close_marker = d[c].start;
            let token = match (d[o].ch, used) {
                (b'~', _) => SyntaxToken::Strikethrough,
                (_, 2) => SyntaxToken::Strong,
                _ => SyntaxToken::Emphasis,
            };
            out.push(base + open_marker..base + open_marker + used, SyntaxToken::Marker);
            out.push(base + open_marker + used..base + close_marker, token);
            out.push(base + close_marker..base + close_marker + used, SyntaxToken::Marker);
            d[o].len -= used;
            d[c].start += used;
            d[c].len -= used;
            for k in &mut d[o + 1..c] {
                k.len = 0;
            }
        }
    }
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace() && !c.is_control())
}

/// Length of an opening code fence at the start of `rest`, if it is one.
fn fence_open(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    let ch = *b.first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let run = count_byte(b, 0, ch);
    if run < 3 || (ch == b'`' && rest[run..].contains('`')) {
        return None;
    }
    Some(run)
}

fn is_thematic_break(s: &str) -> bool {
    let s = s.trim_end();
    let Some(ch) = s.bytes().next() else { return false };
    if !matches!(ch, b'-' | b'*' | b'_') {
        return false;
    }
    let mut n = 0;
    for c in s.bytes() {
        if c == ch {
            n += 1;
        } else if c != b' ' && c != b'\t' {
            return false;
        }
    }
    n >= 3
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

/// If `rest` opens an HTML block, how it ends and the length of the opening
/// token (the end condition is searched after it).
fn html_block_start(rest: &str) -> Option<(HtmlEnd, usize)> {
    let b = rest.as_bytes();
    if b.first() != Some(&b'<') {
        return None;
    }
    for (tag, end) in [("script", "</script>"), ("pre", "</pre>"), ("style", "</style>"), ("textarea", "</textarea>")] {
        if b.len() > tag.len() && b[1..=tag.len()].eq_ignore_ascii_case(tag.as_bytes()) && is_tag_end(b, 1 + tag.len())
        {
            return Some((HtmlEnd::Contains(end), 1 + tag.len()));
        }
    }
    if rest.starts_with("<!--") {
        return Some((HtmlEnd::Contains("-->"), 4));
    }
    if rest.starts_with("<?") {
        return Some((HtmlEnd::Contains("?>"), 2));
    }
    if rest.starts_with("<![CDATA[") {
        return Some((HtmlEnd::Contains("]]>"), 9));
    }
    if rest.starts_with("<!") && b.get(2).is_some_and(u8::is_ascii_alphabetic) {
        return Some((HtmlEnd::Contains(">"), 2));
    }
    let name_start = if b.get(1) == Some(&b'/') { 2 } else { 1 };
    let name_len = b[name_start..].iter().take_while(|c| c.is_ascii_alphanumeric()).count();
    if name_len > 0 {
        let name = rest[name_start..name_start + name_len].to_ascii_lowercase();
        let after = name_start + name_len;
        if BLOCK_TAGS.contains(&name.as_str())
            && (is_tag_end(b, after) || (b.get(after) == Some(&b'/') && b.get(after + 1) == Some(&b'>')))
        {
            return Some((HtmlEnd::BlankLine, after));
        }
    }
    // Type 7: a complete open or closing tag alone on the line.
    let trimmed = rest.trim_end();
    if !trimmed.starts_with("<!")
        && !trimmed.starts_with("<?")
        && inline_html(trimmed.as_bytes(), 0) == Some(trimmed.len())
    {
        return Some((HtmlEnd::BlankLine, trimmed.len()));
    }
    None
}

fn is_tag_end(b: &[u8], i: usize) -> bool {
    b.get(i).is_none_or(|c| c.is_ascii_whitespace() || *c == b'>')
}

/// End (exclusive) of an autolink `<scheme:...>` or `<user@host>` at `i`.
fn autolink(b: &[u8], i: usize) -> Option<usize> {
    let close = i + 1 + b[i + 1..].iter().position(|c| *c == b'>')?;
    let inner = &b[i + 1..close];
    if inner.is_empty() || inner.iter().any(|c| c.is_ascii_whitespace() || *c == b'<') {
        return None;
    }
    let scheme = inner.iter().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-')).count();
    let is_uri = (2..=32).contains(&scheme) && inner[0].is_ascii_alphabetic() && inner.get(scheme) == Some(&b':');
    let is_email = inner.iter().filter(|c| **c == b'@').count() == 1
        && inner.first() != Some(&b'@')
        && inner.last() != Some(&b'@')
        && inner.iter().all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-@".contains(c));
    (is_uri || is_email).then_some(close + 1)
}

/// End (exclusive) of an inline HTML tag or comment at `i`.
fn inline_html(b: &[u8], i: usize) -> Option<usize> {
    let rest = &b[i..];
    let find = |pat: &[u8], from: usize| {
        rest.get(from..)?.windows(pat.len()).position(|w| w == pat).map(|p| i + from + p + pat.len())
    };
    if rest.starts_with(b"<!--") {
        return find(b"-->", 4);
    }
    if rest.starts_with(b"<?") {
        return find(b"?>", 2);
    }
    if rest.starts_with(b"<![CDATA[") {
        return find(b"]]>", 9);
    }
    if rest.starts_with(b"<!") && rest.get(2).is_some_and(u8::is_ascii_alphabetic) {
        return find(b">", 2);
    }
    let closing = rest.get(1) == Some(&b'/');
    let name_start = if closing { 2 } else { 1 };
    if !rest.get(name_start).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let mut j = name_start + rest[name_start..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'-').count();
    if !rest.get(j).is_some_and(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/') {
        return None;
    }
    let mut quote = None;
    while j < rest.len() {
        let c = rest[j];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => {
                if closing {
                    return None;
                }
                quote = Some(c);
            }
            None if c == b'>' => return Some(i + j + 1),
            None if c == b'<' => return None,
            None => {}
        }
        j += 1;
    }
    None
}

/// The end (exclusive, after `)`) of an inline link tail `(dest "title")`
/// starting at `j`.
fn inline_link_tail(b: &[u8], j: usize) -> Option<usize> {
    if b.get(j) != Some(&b'(') {
        return None;
    }
    let mut k = j + 1;
    k += count_ws(b, k);
    if b.get(k) == Some(&b'<') {
        k += 1 + b[k + 1..].iter().position(|c| *c == b'>' || *c == b'<')?;
        if b[k] != b'>' {
            return None;
        }
        k += 1;
    } else {
        let mut depth = 0usize;
        while let Some(&c) = b.get(k) {
            match c {
                b'\\' => k += 1,
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                c if c.is_ascii_whitespace() || c.is_ascii_control() => break,
                _ => {}
            }
            k += 1;
        }
        if depth != 0 {
            return None;
        }
    }
    let ws = count_ws(b, k);
    k += ws;
    if let Some(&q) = b.get(k)
        && ws > 0
        && matches!(q, b'"' | b'\'' | b'(')
    {
        let close = if q == b'(' { b')' } else { q };
        k += 1;
        while let Some(&c) = b.get(k) {
            if c == b'\\' {
                k += 1;
            } else if c == close {
                break;
            }
            k += 1;
        }
        if b.get(k) != Some(&close) {
            return None;
        }
        k += 1;
        k += count_ws(b, k);
    }
    (b.get(k) == Some(&b')')).then_some(k + 1)
}

/// Index of the `]` closing a link label that starts at `j`.
fn label_end(b: &[u8], j: usize) -> Option<usize> {
    let mut k = j;
    while let Some(&c) = b.get(k) {
        match c {
            b'\\' => k += 1,
            b'[' => return None,
            b']' => return (k - j <= 999).then_some(k),
            _ => {}
        }
        k += 1;
    }
    None
}

/// Start of a backtick run of exactly `run` backticks at or after `from`.
fn find_backtick_close(b: &[u8], from: usize, run: usize) -> Option<usize> {
    let mut k = from;
    while k < b.len() {
        if b[k] == b'`' {
            let n = count_byte(b, k, b'`');
            if n == run {
                return Some(k);
            }
            k += n;
        } else {
            k += 1;
        }
    }
    None
}

fn count_byte(b: &[u8], from: usize, ch: u8) -> usize {
    b.get(from..).map_or(0, |s| s.iter().take_while(|c| **c == ch).count())
}

fn count_ws(b: &[u8], from: usize) -> usize {
    b.get(from..).map_or(0, |s| s.iter().take_while(|c| **c == b' ' || **c == b'\t').count())
}

fn leading_ws(s: &str) -> usize {
    count_ws(s.as_bytes(), 0)
}

fn is_ws_or_end(b: &[u8], i: usize) -> bool {
    b.get(i).is_none_or(|c| *c == b' ' || *c == b'\t')
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    h.len() >= n.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::SyntaxToken::*;
    use super::*;
    use crate::buffer::RopeBuffer;

    /// `(text, token)` pairs for readable assertions.
    fn spans(line: &str, ranges: &[StyledRange]) -> Vec<(String, SyntaxToken)> {
        ranges.iter().map(|r| (line[r.range.clone()].to_string(), r.token)).collect()
    }

    fn hl(line: &str) -> Vec<(String, SyntaxToken)> {
        spans(line, &highlight_line(line, &mut LineState::default()))
    }

    fn s(text: &str, token: SyntaxToken) -> (String, SyntaxToken) {
        (text.to_string(), token)
    }

    #[test]
    fn atx_heading_with_inline_and_attributes() {
        assert_eq!(
            hl("## Hello *big* world ## {#id}"),
            vec![
                s("##", Marker),
                s("Hello *big* world", Heading(2)),
                s("*", Marker),
                s("big", Emphasis),
                s("*", Marker),
                s("##", Marker),
                s("{#id}", Marker),
            ]
        );
        assert_eq!(hl("#hashtag"), vec![]);
    }

    #[test]
    fn emphasis_strong_and_strikethrough() {
        assert_eq!(
            hl("a **b** _c_ ~~d~~"),
            vec![
                s("**", Marker),
                s("b", Strong),
                s("**", Marker),
                s("_", Marker),
                s("c", Emphasis),
                s("_", Marker),
                s("~~", Marker),
                s("d", Strikethrough),
                s("~~", Marker),
            ]
        );
        assert_eq!(hl("snake_case_name and 2 * 3 * 4"), vec![]);
        assert_eq!(
            hl("***both***"),
            vec![
                s("*", Marker),
                s("**both**", Emphasis),
                s("**", Marker),
                s("both", Strong),
                s("**", Marker),
                s("*", Marker)
            ]
        );
    }

    #[test]
    fn code_spans_hide_other_syntax() {
        assert_eq!(hl("x `a *b*` y"), vec![s("`", Marker), s("a *b*", Code), s("`", Marker)]);
        assert_eq!(hl("``a ` b``"), vec![s("``", Marker), s("a ` b", Code), s("``", Marker)]);
        assert_eq!(hl("unclosed `tick"), vec![]);
        assert_eq!(hl(r"escaped \*not\*"), vec![]);
    }

    #[test]
    fn links_images_and_autolinks() {
        assert_eq!(
            hl(r#"see [the **docs**](http://x.y "T") now"#),
            vec![
                s("[", Marker),
                s("the **docs**", Link),
                s("**", Marker),
                s("docs", Strong),
                s("**", Marker),
                s("](", Marker),
                s(r#"http://x.y "T""#, Url),
                s(")", Marker),
            ]
        );
        assert_eq!(
            hl("![alt](a.png)"),
            vec![s("![", Marker), s("alt", Link), s("](", Marker), s("a.png", Url), s(")", Marker)]
        );
        assert_eq!(hl("[a][ref]"), vec![s("[", Marker), s("a", Link), s("][", Marker), s("ref", Url), s("]", Marker)]);
        assert_eq!(hl("<https://a.b/c> <me@x.org>"), vec![s("<https://a.b/c>", Url), s("<me@x.org>", Url)]);
        assert_eq!(hl("x<br/>y <!-- c -->"), vec![s("<br/>", Html), s("<!-- c -->", Html)]);
        assert_eq!(hl("a [^1] b"), vec![s("[^", Marker), s("1", Link), s("]", Marker)]);
        assert_eq!(hl("plain [brackets] and 1 < 2"), vec![]);
    }

    #[test]
    fn containers_and_tasks() {
        assert_eq!(hl("> - [x] done"), vec![s(">", Marker), s("-", Marker), s("[x]", Marker)]);
        assert_eq!(hl("12. item"), vec![s("12.", Marker)]);
        assert_eq!(hl("* * *"), vec![s("* * *", Marker)]);
        assert_eq!(hl("2024 was a year"), vec![]);
    }

    #[test]
    fn fence_state_carries_across_lines() {
        let mut st = LineState::default();
        let lines = ["```rust", "let a = *b*;", "", "```", "*after*"];
        let out: Vec<_> = lines.iter().map(|l| spans(l, &highlight_line(l, &mut st))).collect();
        assert_eq!(out[0], vec![s("```", Marker), s("rust", Marker)]);
        assert_eq!(out[1], vec![s("let a = *b*;", Code)]);
        assert_eq!(out[2], vec![]);
        assert_eq!(out[3], vec![s("```", Marker)]);
        assert_eq!(out[4], vec![s("*", Marker), s("after", Emphasis), s("*", Marker)]);
        assert_eq!(st, LineState::default());

        // A shorter run does not close the fence; a quoted fence closes inside the quote.
        let mut st = LineState::default();
        highlight_line("````", &mut st);
        assert_eq!(spans("```", &highlight_line("```", &mut st)), vec![s("```", Code)]);
        let mut st = LineState::default();
        highlight_line("> ~~~", &mut st);
        assert_eq!(spans("> x", &highlight_line("> x", &mut st)), vec![s(">", Marker), s("x", Code)]);
        assert_eq!(spans("> ~~~", &highlight_line("> ~~~", &mut st)), vec![s(">", Marker), s("~~~", Marker)]);
        assert!(st.in_fence.is_none());
    }

    #[test]
    fn html_blocks() {
        let mut st = LineState::default();
        assert_eq!(spans("<div>", &highlight_line("<div>", &mut st)), vec![s("<div>", Html)]);
        assert_eq!(spans("*x*", &highlight_line("*x*", &mut st)), vec![s("*x*", Html)]);
        highlight_line("", &mut st);
        assert!(st.in_html.is_none());
        let mut st = LineState::default();
        highlight_line("<!-- one", &mut st);
        assert_eq!(st.in_html, Some(HtmlEnd::Contains("-->")));
        highlight_line("two -->", &mut st);
        assert!(st.in_html.is_none());
        let mut st = LineState::default();
        highlight_line("<!-- one -->", &mut st);
        assert!(st.in_html.is_none());
    }

    #[test]
    fn reference_definitions_and_tables() {
        assert_eq!(
            hl("[foo]: http://x \"t\""),
            vec![s("[", Marker), s("foo", Link), s("]:", Marker), s("http://x \"t\"", Url)]
        );
        assert_eq!(
            hl("| a | `|` |"),
            vec![s("|", Marker), s("|", Marker), s("`", Marker), s("|", Code), s("`", Marker), s("|", Marker)]
        );
        assert_eq!(hl("|---|:-:|"), vec![s("|---|:-:|", Marker)]);
    }

    fn doc_lines(src: &str) -> Vec<Vec<(String, SyntaxToken)>> {
        let buf = RopeBuffer::from_text(src);
        let index = BlockIndex::parse(&buf);
        let n = buf.len_lines().unwrap();
        highlight_lines(&index, &buf, 0..n + 3).iter().enumerate().map(|(l, r)| spans(&buf.line(l), r)).collect()
    }

    #[test]
    fn block_index_gives_context() {
        let out = doc_lines("Title *x*\n===\n\na | b\n--|--\n1 | 2\n\n    code *x*\n\n<table>\n*x*\n</table>\n");
        assert_eq!(out[0], vec![s("Title *x*", Heading(1)), s("*", Marker), s("x", Emphasis), s("*", Marker)]);
        assert_eq!(out[1], vec![s("===", Marker)]);
        assert_eq!(out[3], vec![s("|", Marker)]);
        assert_eq!(out[4], vec![s("--|--", Marker)]);
        assert_eq!(out[7], vec![s("code *x*", Code)]);
        assert_eq!(out[10], vec![s("*x*", Html)]);
        assert_eq!(out.len(), 13);
    }

    #[test]
    fn paragraph_lines_are_not_blocks() {
        // "2. b" can't interrupt a paragraph, so it is plain text there.
        let out = doc_lines("a\n2. b\n");
        assert_eq!(out[1], vec![]);
    }

    #[test]
    fn viewport_starting_inside_blocks() {
        let src = "intro\n\n```\n# not a heading\n*x*\n```\n\n- a\n  ```\n  *in fence*\n  ```\n";
        let buf = RopeBuffer::from_text(src);
        let index = BlockIndex::parse(&buf);
        let mid = highlight_lines(&index, &buf, 4..6);
        assert_eq!(spans("*x*", &mid[0]), vec![s("*x*", Code)]);
        assert_eq!(spans("```", &mid[1]), vec![s("```", Marker)]);
        let in_list = highlight_lines(&index, &buf, 9..10);
        assert_eq!(spans("  *in fence*", &in_list[0]), vec![s("*in fence*", Code)]);
        assert!(line_state(&index, &buf, 9).in_fence.is_some());
        assert_eq!(line_state(&index, &buf, 0), LineState::default());
    }

    proptest::proptest! {
        #[test]
        fn ranges_stay_on_char_boundaries(lines in proptest::collection::vec("[ #>*_~`\\[\\]()<>!|:a-zé\\-=0-9.\t]{0,40}", 0..12)) {
            let mut st = LineState::default();
            for line in &lines {
                for r in highlight_line(line, &mut st) {
                    proptest::prop_assert!(r.range.start < r.range.end && r.range.end <= line.len());
                    proptest::prop_assert!(line.is_char_boundary(r.range.start) && line.is_char_boundary(r.range.end));
                }
            }
            let buf = RopeBuffer::from_text(&lines.join("\n"));
            let index = BlockIndex::parse(&buf);
            for (l, ranges) in highlight_lines(&index, &buf, 0..lines.len()).iter().enumerate() {
                let line = buf.line(l);
                for r in ranges {
                    proptest::prop_assert!(r.range.end <= line.len() && line.is_char_boundary(r.range.start) && line.is_char_boundary(r.range.end));
                }
            }
        }
    }
}
