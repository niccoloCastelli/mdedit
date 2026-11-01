//! Inline formatting toggles (bold, italic, strike, code), links and images
//! (DESIGN_SYSTEM 8.2).
//!
//! Whether text is already formatted is decided by parsing the surrounding
//! block with pulldown-cmark, so `**a**`, `__a__` and `***a***` are all
//! recognised and toggling off removes exactly the markers of that span.

use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag};
use unicode_segmentation::UnicodeSegmentation;

use super::CommandResult;
use super::text::{Changes, LineInfo, is_url, line_range, link_dest, touched_lines};
use crate::Document;
use crate::parse::markdown_options;
use crate::selection::Selection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Inline {
    Bold,
    Italic,
    Strike,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpanKind {
    Emphasis,
    Strong,
    Strike,
    Code,
    Link,
}

#[derive(Debug, Clone)]
struct Span {
    kind: SpanKind,
    range: Range<usize>,
    /// For links: the source range of the link text (empty for `[](x)`).
    inner: Range<usize>,
}

impl Inline {
    fn span_kind(self) -> SpanKind {
        match self {
            Inline::Bold => SpanKind::Strong,
            Inline::Italic => SpanKind::Emphasis,
            Inline::Strike => SpanKind::Strike,
            Inline::Code => SpanKind::Code,
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Inline::Bold => "**",
            Inline::Italic => "*",
            Inline::Strike => "~~",
            Inline::Code => "`",
        }
    }
}

/// The text the inline parse runs over: the top-level block holding
/// `range`, or the touched lines when there is no block index.
fn context(doc: &Document, range: &Range<usize>) -> Range<usize> {
    let buf = doc.buffer();
    if let Some(index) = doc.blocks()
        && let Some(block) = index.block_at(range.start).and_then(|i| index.get(i))
        && block.bytes.start <= range.start
        && range.end <= block.bytes.end
        && block.bytes.end <= buf.len_bytes()
    {
        return block.bytes;
    }
    let (first, last) = touched_lines(buf, range);
    buf.line_to_byte(first)..line_range(buf, last).end
}

/// An open tag while scanning: its span kind (if one we track), its range
/// and, for links, the range of the link text seen so far.
type Frame = (Option<SpanKind>, Range<usize>, Option<Range<usize>>);

fn spans(doc: &Document, range: &Range<usize>) -> Vec<Span> {
    let ctx = context(doc, range);
    let text = doc.buffer().slice(ctx.clone());
    let base = ctx.start;
    let abs = |r: Range<usize>| base + r.start..base + r.end;
    let mut out = Vec::new();
    // (kind, range, link text range) per open tag.
    let mut stack: Vec<Frame> = Vec::new();
    let extend_links = |stack: &mut Vec<Frame>, r: &Range<usize>| {
        for (kind, _, inner) in stack.iter_mut() {
            if *kind == Some(SpanKind::Link) {
                *inner = Some(match inner.take() {
                    Some(i) => i.start.min(r.start)..i.end.max(r.end),
                    None => r.clone(),
                });
            }
        }
    };
    for (event, r) in Parser::new_ext(&text, markdown_options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                extend_links(&mut stack, &r);
                let kind = match tag {
                    Tag::Emphasis => Some(SpanKind::Emphasis),
                    Tag::Strong => Some(SpanKind::Strong),
                    Tag::Strikethrough => Some(SpanKind::Strike),
                    Tag::Link { .. } => Some(SpanKind::Link),
                    _ => None,
                };
                stack.push((kind, r, None));
            }
            Event::End(_) => {
                if let Some((Some(kind), range, inner)) = stack.pop() {
                    let inner = inner.unwrap_or(range.start + 1..range.start + 1);
                    out.push(Span { kind, range: abs(range), inner: abs(inner) });
                }
            }
            Event::Code(_) => {
                extend_links(&mut stack, &r);
                out.push(Span { kind: SpanKind::Code, inner: abs(r.clone()), range: abs(r) });
            }
            _ => extend_links(&mut stack, &r),
        }
    }
    out
}

/// The innermost span of `kind` containing `seg`. An empty `seg` (a cursor)
/// must be strictly inside, not at an outer edge.
fn containing<'a>(spans: &'a [Span], kind: SpanKind, seg: &Range<usize>) -> Option<&'a Span> {
    spans
        .iter()
        .filter(|s| s.kind == kind)
        .filter(|s| {
            if seg.is_empty() {
                s.range.start < seg.start && seg.start < s.range.end
            } else {
                s.range.start <= seg.start && seg.end <= s.range.end
            }
        })
        .min_by_key(|s| s.range.len())
}

/// The non-blank, trimmed piece of each touched line inside `range`,
/// skipping block prefixes (`- `, `> `, `# `).
fn segments(doc: &Document, range: &Range<usize>) -> Vec<Range<usize>> {
    let buf = doc.buffer();
    let (first, last) = touched_lines(buf, range);
    let mut out = Vec::new();
    for l in first..=last {
        let lr = line_range(buf, l);
        let line = buf.line(l);
        let content = lr.start + LineInfo::parse(&line).content;
        let start = range.start.max(content);
        let end = range.end.min(lr.end);
        if start >= end {
            continue;
        }
        let s = &line[start - lr.start..end - lr.start];
        let lead = s.len() - s.trim_start().len();
        let trail = s.len() - s.trim_end().len();
        if lead + trail < s.len() {
            out.push(start + lead..end - trail);
        }
    }
    out
}

/// The word around a cursor, if any.
fn word_at(doc: &Document, at: usize) -> Option<Range<usize>> {
    let buf = doc.buffer();
    let line_no = buf.byte_to_line(at);
    let lr = line_range(buf, line_no);
    let line = buf.line(line_no);
    let col = at - lr.start;
    let is_word = |w: &str| w.chars().any(|c| c.is_alphanumeric());
    let mut best = None;
    for (i, w) in line.split_word_bound_indices() {
        let r = i..i + w.len();
        if !is_word(w) {
            continue;
        }
        if r.start < col && col < r.end {
            best = Some(r);
            break;
        }
        if r.start == col || r.end == col {
            // Prefer the word starting at the cursor; keep looking for it.
            best = Some(r);
            if i == col {
                break;
            }
        }
    }
    best.map(|r| lr.start + r.start..lr.start + r.end)
}

fn marker_len(doc: &Document, span: &Span) -> usize {
    match span.kind {
        SpanKind::Strong => 2,
        SpanKind::Emphasis => 1,
        SpanKind::Strike | SpanKind::Code => {
            let c = if span.kind == SpanKind::Strike { '~' } else { '`' };
            let text = doc.buffer().slice(span.range.clone());
            text.chars().take_while(|&x| x == c).count().min(text.len() / 2)
        }
        SpanKind::Link => 0,
    }
}

fn unwrap(doc: &Document, span: &Span, ch: &mut Changes) {
    let n = marker_len(doc, span);
    let (mut open, mut close) = (span.range.start..span.range.start + n, span.range.end - n..span.range.end);
    if span.kind == SpanKind::Code {
        // `` ` a ` `` strips one space on each side when both are there.
        let inner = doc.buffer().slice(open.end..close.start);
        if inner.len() >= 2 && inner.starts_with(' ') && inner.ends_with(' ') && !inner.trim().is_empty() {
            open.end += 1;
            close.start -= 1;
        }
    }
    ch.delete(open);
    ch.delete(close);
}

/// Opening and closing markers that wrap `text`.
fn wrap_markers(kind: Inline, text: &str) -> (String, String) {
    if kind != Inline::Code {
        return (kind.marker().into(), kind.marker().into());
    }
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') { " " } else { "" };
    (format!("{fence}{pad}"), format!("{pad}{fence}"))
}

pub(crate) fn toggle(doc: &Document, sel: Selection, kind: Inline) -> CommandResult {
    let buf = doc.buffer();
    let range = sel.range();
    let all = spans(doc, &range);
    let mut ch = Changes::default();

    if range.is_empty() {
        let at = range.start;
        if let Some(span) = containing(&all, kind.span_kind(), &range) {
            unwrap(doc, span, &mut ch);
            return ch.finish_mapped(sel);
        }
        // `**|**`: undo an empty pair.
        let m = kind.marker();
        let c = m.as_bytes()[0];
        let run = |r: Range<usize>, rev: bool| {
            let s = buf.slice(r);
            let b = s.as_bytes();
            if rev {
                b.iter().rev().take_while(|&&x| x == c).count()
            } else {
                b.iter().take_while(|&&x| x == c).count()
            }
        };
        let lr = line_range(buf, buf.byte_to_line(at));
        if run(lr.start..at, true) == m.len() && run(at..lr.end, false) == m.len() {
            ch.delete(at - m.len()..at);
            ch.delete(at..at + m.len());
            return ch.finish(Selection::cursor(at - m.len()));
        }
        if let Some(word) = word_at(doc, at) {
            let (open, close) = wrap_markers(kind, &buf.slice(word.clone()));
            ch.insert(word.start, open);
            ch.insert(word.end, close);
            let cursor = ch.map(at, at == word.start);
            return ch.finish(Selection::cursor(cursor));
        }
        let (open, close) = wrap_markers(kind, "");
        let len = open.len();
        ch.insert(at, format!("{open}{close}"));
        return ch.finish(Selection::cursor(at + len));
    }

    let segs = segments(doc, &range);
    let wrapped: Vec<Option<&Span>> = segs.iter().map(|s| containing(&all, kind.span_kind(), s)).collect();
    if !segs.is_empty() && wrapped.iter().all(Option::is_some) {
        let mut done: Vec<Range<usize>> = Vec::new();
        for span in wrapped.into_iter().flatten() {
            if !done.contains(&span.range) {
                done.push(span.range.clone());
                unwrap(doc, span, &mut ch);
            }
        }
    } else {
        for (seg, span) in segs.iter().zip(wrapped) {
            if span.is_none() {
                let (open, close) = wrap_markers(kind, &buf.slice(seg.clone()));
                ch.insert(seg.start, open);
                ch.insert(seg.end, close);
            }
        }
    }
    ch.finish_mapped(sel)
}

/// Ctrl+K: wrap the selection (or the word at the cursor) in a link, or
/// remove the link the cursor is in. Without a URL the cursor lands where
/// the URL goes.
pub(crate) fn link(doc: &Document, sel: Selection, url: Option<&str>) -> CommandResult {
    let range = sel.range();
    let all = spans(doc, &range);
    let mut ch = Changes::default();
    if let Some(span) = containing(&all, SpanKind::Link, &range) {
        // Keep the link text (for autolinks `<url>`, the URL) and delete
        // the syntax around it, so a cursor in the text stays put.
        ch.delete(span.range.start..span.inner.start);
        ch.delete(span.inner.end..span.range.end);
        return ch.finish_mapped(sel);
    }
    insert_link(doc, sel, url, false)
}

/// `![alt](url)` around the selection or word (alt text), or empty.
pub(crate) fn image(doc: &Document, sel: Selection, url: Option<&str>) -> CommandResult {
    insert_link(doc, sel, url, true)
}

fn insert_link(doc: &Document, sel: Selection, url: Option<&str>, image: bool) -> CommandResult {
    let buf = doc.buffer();
    let mut range = sel.range();
    if range.is_empty()
        && let Some(word) = word_at(doc, range.start)
    {
        range = word;
    }
    let text = buf.slice(range.clone()).into_owned();
    let bang = if image { "!" } else { "" };
    let mut ch = Changes::default();
    let start = range.start;
    let (new, cursor) = match url {
        Some(url) => {
            let s = format!("{bang}[{text}]({})", link_dest(url));
            let cursor = if text.is_empty() { start + bang.len() + 1 } else { start + s.len() };
            (s, cursor)
        }
        None if !image && is_url(&text) => (format!("[]({})", link_dest(text.trim())), start + 1),
        None if text.is_empty() && !image => ("[]()".to_string(), start + 1),
        None => {
            let s = format!("{bang}[{text}]()");
            let len = s.len();
            (s, start + len - 1)
        }
    };
    ch.replace(range, new);
    ch.finish(Selection::cursor(cursor))
}

/// Pasting a URL onto a selection makes a link (DESIGN_SYSTEM 8.3).
pub(crate) fn paste_url(doc: &Document, sel: Selection, text: &str) -> Option<CommandResult> {
    let buf = doc.buffer();
    let range = sel.range();
    if range.is_empty() || !is_url(text) || buf.slice(range.clone()).contains('\n') {
        return None;
    }
    Some(insert_link(doc, sel, Some(text.trim()), false))
}
