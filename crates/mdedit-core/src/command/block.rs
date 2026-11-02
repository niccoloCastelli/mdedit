//! Block formatting: headings, lists, quotes, code blocks, rules and
//! inserted blocks (DESIGN_SYSTEM 8.2). Each command works on every line the
//! selection touches and toggles off when all of them already have it.

use std::collections::BTreeMap;

use super::CommandResult;
use super::text::{Changes, LineInfo, ListKind, is_blank, line_count, line_range, setext_level, touched_lines};
use crate::Document;
use crate::parse::BlockKind;
use crate::selection::Selection;

/// The lines a block command acts on: the touched non-blank lines, or the
/// cursor line when they are all blank.
fn target_lines(doc: &Document, sel: Selection) -> Vec<usize> {
    let buf = doc.buffer();
    let (first, last) = touched_lines(buf, &sel.range());
    let lines: Vec<usize> = (first..=last).filter(|&l| !is_blank(&buf.line(l))).collect();
    if lines.is_empty() { vec![first] } else { lines }
}

/// Level of the heading on line `l`, ATX or setext.
fn heading_level(doc: &Document, l: usize, info: &LineInfo) -> Option<u8> {
    if let Some((level, _)) = info.heading {
        return Some(level);
    }
    setext_underline(doc, l).map(|(level, _)| level)
}

/// When line `l` is the text of a setext heading: its level and the line
/// number of the underline.
fn setext_underline(doc: &Document, l: usize) -> Option<(u8, usize)> {
    let buf = doc.buffer();
    if l + 1 >= line_count(buf) || is_blank(&buf.line(l)) {
        return None;
    }
    let level = setext_level(&buf.line(l + 1))?;
    // `---` under a paragraph line is a heading, but under a list item it is
    // a rule; the block index knows which.
    match doc.blocks() {
        Some(index) => {
            let start = buf.line_to_byte(l);
            let block = index.block_at(start).and_then(|i| index.get(i))?;
            matches!(block.kind, BlockKind::Heading { .. }).then_some((level, l + 1))
        }
        None => (level == 1).then_some((level, l + 1)),
    }
}

pub(crate) fn set_heading(doc: &Document, sel: Selection, level: u8) -> CommandResult {
    let buf = doc.buffer();
    let mut lines = target_lines(doc, sel);
    // A setext underline goes away with its heading text.
    let underlines: Vec<usize> = lines.iter().filter_map(|&l| setext_underline(doc, l)).map(|u| u.1).collect();
    lines.retain(|l| !underlines.contains(l));
    let infos: Vec<(usize, String, LineInfo)> = lines
        .iter()
        .map(|&l| {
            let line = buf.line(l).into_owned();
            let info = LineInfo::parse(&line);
            (l, line, info)
        })
        .collect();
    let level = if level > 0 && infos.iter().all(|(l, _, i)| heading_level(doc, *l, i) == Some(level)) {
        0
    } else {
        level.min(6)
    };
    let prefix = if level == 0 { String::new() } else { format!("{} ", "#".repeat(level as usize)) };
    let mut ch = Changes::default();
    for (l, line, info) in &infos {
        let start = buf.line_to_byte(*l);
        // A paragraph has no list marker or heading marks; drop the
        // indentation too so the text does not turn into a code block.
        let from = if info.list.is_some() || info.heading.is_some() { info.quote } else { info.indent };
        let mut to = info.content;
        // Closing `#`s of an ATX heading.
        let mut end_trim = None;
        if info.heading.is_some() {
            let body = line[to..].trim_end();
            let stripped = body.trim_end_matches('#');
            if stripped.len() < body.len() && (stripped.is_empty() || stripped.ends_with([' ', '\t'])) {
                end_trim = Some(to + stripped.trim_end().len()..line.len());
            }
        }
        if line[to..].trim().is_empty() {
            to = line.len();
        }
        ch.replace(start + from..start + to, prefix.clone());
        if let Some(r) = end_trim
            && r.start >= to
        {
            ch.delete(start + r.start..start + r.end);
        }
        if let Some((_, underline)) = setext_underline(doc, *l) {
            ch.delete(line_range(buf, *l).end..line_range(buf, underline).end);
        }
    }
    ch.finish_mapped(sel)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum List {
    Bullet,
    Numbered,
    Task,
}

fn is_list(info: &LineInfo, kind: List) -> bool {
    match (&info.list, kind) {
        (Some(m), List::Task) => m.task.is_some(),
        (Some(m), List::Bullet) => m.task.is_none() && matches!(m.kind, ListKind::Bullet(_)),
        (Some(m), List::Numbered) => m.task.is_none() && matches!(m.kind, ListKind::Ordered { .. }),
        (None, _) => false,
    }
}

pub(crate) fn toggle_list(doc: &Document, sel: Selection, kind: List) -> CommandResult {
    let buf = doc.buffer();
    let lines = target_lines(doc, sel);
    let infos: Vec<(usize, String, LineInfo)> = lines
        .iter()
        .map(|&l| {
            let line = buf.line(l).into_owned();
            let info = LineInfo::parse(&line);
            (l, line, info)
        })
        .collect();
    let mut ch = Changes::default();
    if infos.iter().all(|(_, _, i)| is_list(i, kind)) {
        for (l, _, info) in &infos {
            let start = buf.line_to_byte(*l);
            let m = info.list.as_ref().expect("checked above");
            ch.delete(start + m.range.start..start + m.end());
        }
        return ch.finish_mapped(sel);
    }
    // Numbering restarts at 1 for each indentation level.
    let mut counters: BTreeMap<usize, u64> = BTreeMap::new();
    for (l, line, info) in &infos {
        let start = buf.line_to_byte(*l);
        let cols = info.indent_cols(line);
        counters.retain(|&c, _| c <= cols);
        let n = counters.entry(cols).or_insert(0);
        *n += 1;
        let marker = match kind {
            List::Bullet => "- ".to_string(),
            List::Numbered => format!("{n}. "),
            List::Task => {
                let checked = info.list.as_ref().and_then(|m| m.task.as_ref()).is_some_and(|t| t.0);
                if checked { "- [x] ".into() } else { "- [ ] ".into() }
            }
        };
        ch.replace(start + info.indent..start + info.content, marker);
    }
    ch.finish_mapped(sel)
}

pub(crate) fn toggle_quote(doc: &Document, sel: Selection) -> CommandResult {
    let buf = doc.buffer();
    let (first, last) = touched_lines(buf, &sel.range());
    let lines: Vec<(usize, String)> = (first..=last).map(|l| (l, buf.line(l).into_owned())).collect();
    let quoted = |line: &str| line.trim_start_matches(' ').starts_with('>');
    let mut ch = Changes::default();
    let non_blank: Vec<&(usize, String)> = lines.iter().filter(|(_, s)| !is_blank(s)).collect();
    if !non_blank.is_empty() && non_blank.iter().all(|(_, s)| quoted(s)) {
        for (l, line) in &lines {
            let Some(gt) = line.find('>').filter(|_| quoted(line)) else { continue };
            let end = if line[gt + 1..].starts_with(' ') { gt + 2 } else { gt + 1 };
            let start = buf.line_to_byte(*l);
            ch.delete(start + gt..start + end);
        }
    } else {
        for (l, line) in &lines {
            let start = buf.line_to_byte(*l);
            ch.insert(start, if is_blank(line) && lines.len() > 1 { ">" } else { "> " });
        }
    }
    ch.finish_mapped(sel)
}

pub(crate) fn toggle_code_block(doc: &Document, sel: Selection) -> CommandResult {
    let buf = doc.buffer();
    let range = sel.range();
    let nl = doc.format.line_ending.as_str();
    let mut ch = Changes::default();
    if let Some(index) = doc.blocks()
        && let Some(block) = index.block_at(range.start).and_then(|i| index.get(i))
        && matches!(block.kind, BlockKind::CodeBlock { fenced: true, .. } | BlockKind::Math | BlockKind::Mermaid)
        && block.bytes.start <= range.start
        && range.end <= block.bytes.end
    {
        let first = buf.byte_to_line(block.bytes.start);
        let last = buf.byte_to_line(block.bytes.end);
        let closing = |l: usize| {
            let line = buf.line(l);
            let t = line.trim();
            t.len() >= 3 && (t.bytes().all(|b| b == b'`') || t.bytes().all(|b| b == b'~'))
        };
        if last > first + 1 && closing(last) {
            ch.delete(line_range(buf, last - 1).end..line_range(buf, last).end);
        } else if last == first + 1 && closing(last) {
            ch.delete(line_range(buf, last));
        }
        ch.delete(buf.line_to_byte(first)..buf.line_to_byte(first + 1));
        return ch.finish_mapped(sel);
    }
    let (first, last) = touched_lines(buf, &range);
    if first == last && is_blank(&buf.line(first)) {
        let lr = line_range(buf, first);
        ch.replace(lr.clone(), format!("```{nl}{nl}```"));
        return ch.finish(Selection::cursor(lr.start + 3 + nl.len()));
    }
    let longest =
        (first..=last).map(|l| buf.line(l).trim_start().bytes().take_while(|&b| b == b'`').count()).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    ch.insert(buf.line_to_byte(first), format!("{fence}{nl}"));
    ch.insert(line_range(buf, last).end, format!("{nl}{fence}"));
    ch.finish_mapped(sel)
}

/// Where the cursor goes after [`insert_block`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum After {
    /// This byte offset inside the inserted block.
    In(usize),
    /// The start of the line after the block.
    Below,
}

/// Insert a block (lines joined with `\n`, converted to the document's line
/// ending) as its own paragraph-separated block at the cursor line: in place
/// of a blank line, or after a non-blank one.
pub(crate) fn insert_block(doc: &Document, sel: Selection, block: &str, after: After) -> CommandResult {
    let buf = doc.buffer();
    let nl = doc.format.line_ending.as_str();
    let block = block.replace('\n', nl);
    let line = buf.byte_to_line(sel.range().end);
    let lr = line_range(buf, line);
    let total = line_count(buf);
    let next_blank = line + 1 < total && is_blank(&buf.line(line + 1));
    let suffix = if next_blank { "" } else { nl };
    let mut ch = Changes::default();
    let (at, prefix) = if is_blank(&buf.line(line)) {
        let prev_blank = line == 0 || is_blank(&buf.line(line - 1));
        ch.replace(lr.clone(), format!("{}{block}{suffix}", if prev_blank { "" } else { nl }));
        (lr.start, if prev_blank { 0 } else { nl.len() })
    } else {
        ch.insert(lr.end, format!("{nl}{nl}{block}{suffix}"));
        (lr.end, 2 * nl.len())
    };
    let block_start = at + prefix;
    let cursor = match after {
        After::In(off) => block_start + off,
        After::Below => block_start + block.len() + nl.len(),
    };
    ch.finish(Selection::cursor(cursor))
}

pub(crate) fn mermaid(doc: &Document, sel: Selection) -> CommandResult {
    let nl = doc.format.line_ending.as_str().len();
    let caret = "```mermaid".len() + nl + "flowchart LR".len() + nl + "  A --> B".len();
    insert_block(doc, sel, "```mermaid\nflowchart LR\n  A --> B\n```", After::In(caret))
}

pub(crate) fn horizontal_rule(doc: &Document, sel: Selection) -> CommandResult {
    insert_block(doc, sel, "---", After::Below)
}
