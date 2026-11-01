//! Smart editing (DESIGN_SYSTEM 8.3): list and quote continuation on Enter,
//! Tab / Shift+Tab on list items and table cells, wrapping a selection when
//! a pairing character is typed.

use super::text::{Changes, LineInfo, ListKind, columns, is_blank, line_range, touched_lines};
use super::{CommandResult, TableOp, table};
use crate::Document;
use crate::selection::Selection;

/// Enter: continue the list item or quote on the cursor line, or end it
/// when the item is empty. `None` when a plain newline is right.
pub(crate) fn enter(doc: &Document, sel: Selection) -> Option<CommandResult> {
    let buf = doc.buffer();
    let range = sel.range();
    let l = buf.byte_to_line(range.start);
    if buf.byte_to_line(range.end) != l {
        return None;
    }
    let line = buf.line(l).into_owned();
    let start = buf.line_to_byte(l);
    let info = LineInfo::parse(&line);
    if info.list.is_none() && info.quote == 0 {
        return None;
    }
    let text_start = info.list.as_ref().map_or(info.indent, |m| m.end());
    let col = range.start - start;
    if col < text_start {
        return None;
    }
    let nl = doc.format.line_ending.as_str();
    let mut ch = Changes::default();
    if is_blank(&line[text_start..]) {
        // An empty item (or `> `) ends the list or quote: a nested item
        // moves out one level, a top-level one loses its marker.
        if info.list.is_some()
            && info.indent > info.quote
            && let Some(r) = outdent_lines(doc, l, l)
        {
            return Some(r);
        }
        let from = if info.list.is_some() { info.quote } else { 0 };
        ch.replace(start + from..start + line.len(), "");
        return Some(ch.finish(Selection::cursor(start + from)));
    }
    let marker = match &info.list {
        None => String::new(),
        Some(m) => {
            let mut s = match m.kind {
                ListKind::Bullet(c) => format!("{} ", c as char),
                ListKind::Ordered { number, delim } => format!("{}{} ", number + 1, delim as char),
            };
            if m.task.is_some() {
                s.push_str("[ ] ");
            }
            s
        }
    };
    let insert = format!("{nl}{}{marker}", &line[..info.indent]);
    let cursor = range.start + insert.len();
    ch.replace(range, insert);
    Some(ch.finish(Selection::cursor(cursor)))
}

/// The marker width (`- ` = 2, `10. ` = 4) of the nearest list item above
/// line `l` with the same indentation, or of line `l` itself.
fn indent_unit(doc: &Document, l: usize, info: &LineInfo, line: &str) -> usize {
    let buf = doc.buffer();
    let cols = info.indent_cols(line);
    let own = info.list.as_ref().map_or(2, |m| m.range.len());
    for p in (0..l).rev() {
        let pl = buf.line(p);
        if is_blank(&pl) {
            continue;
        }
        let pi = LineInfo::parse(&pl);
        let Some(m) = &pi.list else {
            if pi.indent_cols(&pl) < cols {
                break;
            }
            continue;
        };
        let pc = pi.indent_cols(&pl);
        if pc == cols {
            return m.range.len();
        }
        if pc < cols {
            break;
        }
    }
    own
}

/// Tab: next table cell, or indent the list items in the selection.
pub(crate) fn indent(doc: &Document, sel: Selection) -> Option<CommandResult> {
    if let Some(r) = table::run_op(doc, sel, TableOp::NextCell) {
        return Some(r);
    }
    let buf = doc.buffer();
    let (first, last) = touched_lines(buf, &sel.range());
    let mut ch = Changes::default();
    for l in first..=last {
        let line = buf.line(l);
        let info = LineInfo::parse(&line);
        let Some(m) = &info.list else { continue };
        let start = buf.line_to_byte(l);
        let unit = indent_unit(doc, l, &info, &line);
        let cols = info.indent_cols(&line) + unit;
        // A nested numbered list starts again at 1.
        let number_end = start + m.range.start + m.number_digits();
        let number = if m.number_digits() > 0 { "1" } else { "" };
        ch.replace(start + info.quote..number_end, format!("{}{number}", " ".repeat(cols)));
    }
    if ch.is_empty() {
        return None;
    }
    Some(ch.finish_mapped(sel))
}

/// Shift+Tab: previous table cell, or outdent the list items in the
/// selection.
pub(crate) fn outdent(doc: &Document, sel: Selection) -> Option<CommandResult> {
    if let Some(r) = table::run_op(doc, sel, TableOp::PrevCell) {
        return Some(r);
    }
    let buf = doc.buffer();
    let (first, last) = touched_lines(buf, &sel.range());
    let mut ch = Changes::default();
    collect_outdent(doc, first, last, &mut ch);
    if ch.is_empty() {
        return None;
    }
    Some(ch.finish_mapped(sel))
}

fn outdent_lines(doc: &Document, first: usize, last: usize) -> Option<CommandResult> {
    let buf = doc.buffer();
    let mut ch = Changes::default();
    collect_outdent(doc, first, last, &mut ch);
    if ch.is_empty() {
        return None;
    }
    let cursor = line_range(buf, first).end;
    Some(ch.finish_mapped(Selection::cursor(cursor)))
}

fn collect_outdent(doc: &Document, first: usize, last: usize, ch: &mut Changes) {
    let buf = doc.buffer();
    for l in first..=last {
        let line = buf.line(l);
        let info = LineInfo::parse(&line);
        let Some(m) = &info.list else { continue };
        let cols = info.indent_cols(&line);
        if cols == 0 {
            continue;
        }
        // The parent item: the nearest list item above with less indent.
        let mut parent = None;
        for p in (0..l).rev() {
            let pl = buf.line(p);
            let pi = LineInfo::parse(&pl);
            if let Some(pm) = &pi.list
                && pi.indent_cols(&pl) < cols
            {
                parent = Some((columns(&pl[pi.quote..pi.indent]), pm.kind));
                break;
            }
        }
        let (new_cols, parent_kind) = parent.unwrap_or((0, m.kind));
        let start = buf.line_to_byte(l);
        let mut end = start + info.indent;
        let mut number = String::new();
        // The item becomes the parent's next sibling.
        if let (ListKind::Ordered { .. }, ListKind::Ordered { number: n, .. }) = (m.kind, parent_kind) {
            end = start + m.range.start + m.number_digits();
            number = (n + 1).to_string();
        }
        ch.replace(start + info.quote..end, format!("{}{number}", " ".repeat(new_cols)));
    }
}

/// The closing character for a typed pairing character, when typing it
/// over a selection should wrap the selection (DESIGN_SYSTEM 8.3).
fn pair_for(text: &str) -> Option<&'static str> {
    match text {
        "*" => Some("*"),
        "**" => Some("**"),
        "_" => Some("_"),
        "`" => Some("`"),
        "~" => Some("~"),
        "~~" => Some("~~"),
        "[" => Some("]"),
        _ => None,
    }
}

pub(crate) fn wrap(sel: Selection, text: &str) -> Option<CommandResult> {
    let close = pair_for(text)?;
    if sel.is_empty() {
        return None;
    }
    let r = sel.range();
    let mut ch = Changes::default();
    ch.insert(r.start, text);
    ch.insert(r.end, close);
    Some(ch.finish_mapped(sel))
}
