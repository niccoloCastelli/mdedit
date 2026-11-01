//! GFM tables in Edit mode (DESIGN_SYSTEM 8.4): finding the table at the
//! cursor, auto-alignment (widths in grapheme clusters, wide characters
//! counting as two columns), row and column operations, cell navigation and
//! CSV/TSV paste.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::block::{After, insert_block};
use super::text::{Changes, LineInfo, is_blank, line_count, line_range};
use super::{ColumnAlign, CommandResult, TableOp};
use crate::Document;
use crate::buffer::TextBuffer;
use crate::selection::Selection;

/// Display width of `s`: one column per grapheme cluster, two for wide
/// (CJK, emoji) clusters.
pub fn display_width(s: &str) -> usize {
    s.graphemes(true).map(|g| if g.width() >= 2 { 2 } else { 1 }).sum()
}

/// A parsed table. Rows exclude the delimiter row: `rows[0]` is the header.
#[derive(Debug, Clone)]
pub(crate) struct Table {
    /// From the first byte of the header line to the end of the last row.
    pub range: Range<usize>,
    pub first_line: usize,
    /// Blockquote prefix of the header line, repeated on every line.
    pub prefix: String,
    pub rows: Vec<Vec<String>>,
    pub aligns: Vec<ColumnAlign>,
    /// Absolute byte ranges of each source row's cells: trimmed content and
    /// the slot between the pipes (`None` for the delimiter row).
    cells: Vec<Option<Vec<(Range<usize>, Range<usize>)>>>,
}

/// Split a row into trimmed cells: `(content range, slot range)` relative to
/// `row`, where the slot runs between the surrounding pipes.
fn split_row(row: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let b = row.as_bytes();
    let mut pipes = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'|' => pipes.push(i),
            _ => {}
        }
        i += 1;
    }
    let lead = row.len() - row.trim_start().len();
    let trail_end = row.trim_end().len();
    let mut bounds: Vec<usize> = Vec::new();
    let leading_pipe = pipes.first() == Some(&lead);
    let trailing_pipe = trail_end > 0 && pipes.last() == Some(&(trail_end - 1)) && pipes.len() > leading_pipe as usize;
    let start = if leading_pipe { lead + 1 } else { lead };
    bounds.push(start);
    let inner: Vec<usize> = pipes
        .iter()
        .copied()
        .filter(|&p| !(leading_pipe && p == lead) && !(trailing_pipe && p == trail_end - 1))
        .collect();
    for p in &inner {
        bounds.push(p + 1);
    }
    let end = if trailing_pipe { trail_end - 1 } else { trail_end };
    let mut out = Vec::new();
    for (k, &s) in bounds.iter().enumerate() {
        let e = inner.get(k).copied().unwrap_or(end).max(s);
        let cell = &row[s..e];
        let cs = s + (cell.len() - cell.trim_start().len());
        let ce = (s + cell.trim_end().len()).max(cs);
        out.push((cs..ce, s..e));
    }
    out
}

fn has_pipe(row: &str) -> bool {
    let b = row.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'|' => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

fn parse_align(cell: &str) -> Option<ColumnAlign> {
    let c = cell.trim();
    let left = c.starts_with(':');
    let right = c.ends_with(':') && c.len() > 1;
    let dashes = c.trim_start_matches(':').trim_end_matches(':');
    if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
        return None;
    }
    Some(match (left, right) {
        (true, true) => ColumnAlign::Center,
        (true, false) => ColumnAlign::Left,
        (false, true) => ColumnAlign::Right,
        (false, false) => ColumnAlign::None,
    })
}

fn delimiter_aligns(row: &str) -> Option<Vec<ColumnAlign>> {
    if !has_pipe(row) {
        return None;
    }
    split_row(row).iter().map(|(c, _)| parse_align(&row[c.clone()])).collect()
}

fn quote_depth(prefix: &str) -> usize {
    prefix.bytes().filter(|&b| b == b'>').count()
}

/// The table containing byte `at`, if any.
pub(crate) fn table_at(buf: &dyn TextBuffer, at: usize) -> Option<Table> {
    let line_no = buf.byte_to_line(at.min(buf.len_bytes()));
    let total = line_count(buf);
    let split = |l: usize| {
        let line = buf.line(l).into_owned();
        let q = LineInfo::parse(&line).quote;
        let body = line[q..].to_string();
        (line[..q].to_string(), body)
    };
    let (prefix, body) = split(line_no);
    let depth = quote_depth(&prefix);
    let is_row = |l: usize| {
        let (p, b) = split(l);
        quote_depth(&p) == depth && !is_blank(&b) && has_pipe(&b)
    };
    if !has_pipe(&body) || is_blank(&body) {
        return None;
    }
    let mut top = line_no;
    while top > 0 && is_row(top - 1) {
        top -= 1;
    }
    let mut bottom = line_no;
    while bottom + 1 < total && is_row(bottom + 1) {
        bottom += 1;
    }
    let delim = (top + 1..=bottom).find(|&d| {
        let header = split(d - 1).1;
        delimiter_aligns(&split(d).1).is_some_and(|a| a.len() == split_row(&header).len())
    })?;
    let header = delim - 1;
    if line_no < header {
        return None;
    }
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    let mut aligns = Vec::new();
    for l in header..=bottom {
        let start = buf.line_to_byte(l);
        let (p, b) = split(l);
        if l == delim {
            aligns = delimiter_aligns(&b).unwrap_or_default();
            cells.push(None);
            continue;
        }
        let parts = split_row(&b);
        let base = start + p.len();
        rows.push(parts.iter().map(|(c, _)| b[c.clone()].to_string()).collect());
        let abs = |r: &Range<usize>| base + r.start..base + r.end;
        cells.push(Some(parts.iter().map(|(c, s)| (abs(c), abs(s))).collect()));
    }
    let header_prefix = split(header).0;
    let range = buf.line_to_byte(header)..line_range(buf, bottom).end;
    Some(Table { range, first_line: header, prefix: header_prefix, rows, aligns, cells })
}

/// A cell position: row (0 = header, delimiter excluded), column and byte
/// offset inside the trimmed content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellPos {
    row: usize,
    col: usize,
    offset: usize,
}

impl Table {
    fn cols(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0).max(self.aligns.len()).max(1)
    }

    fn normalise(&mut self) {
        let n = self.cols();
        for row in &mut self.rows {
            row.resize(n, String::new());
        }
        self.aligns.resize(n, ColumnAlign::None);
    }

    /// The cell holding byte `at` (on the delimiter row: the header cell).
    fn pos_of(&self, buf: &dyn TextBuffer, at: usize) -> CellPos {
        let line = buf.byte_to_line(at) - self.first_line;
        let src = line.min(self.cells.len() - 1);
        let row = if src >= 2 { src - 1 } else { 0 };
        let Some(cells) = &self.cells[src] else { return CellPos { row: 0, col: 0, offset: 0 } };
        let col = cells.iter().rposition(|(_, slot)| slot.start <= at).unwrap_or(0);
        // In the padding around the content: its nearest end.
        let c = &cells[col].0;
        let offset = at.clamp(c.start, c.end) - c.start;
        CellPos { row, col, offset }
    }

    /// Aligned text and the byte offset of each cell's content start.
    fn render(&self, nl: &str) -> (String, Vec<Vec<(usize, usize)>>) {
        let n = self.cols();
        let mut widths = vec![3usize; n];
        for row in &self.rows {
            for (c, cell) in row.iter().enumerate() {
                widths[c] = widths[c].max(display_width(cell));
            }
        }
        let mut out = String::new();
        let mut positions = Vec::new();
        let empty = String::new();
        for (r, row) in self.rows.iter().enumerate() {
            if r == 1 {
                out.push_str(&self.prefix);
                out.push('|');
                for (c, w) in widths.iter().enumerate() {
                    let a = self.aligns.get(c).copied().unwrap_or_default();
                    let dashes = match a {
                        ColumnAlign::None => "-".repeat(*w),
                        ColumnAlign::Left => format!(":{}", "-".repeat(w - 1)),
                        ColumnAlign::Right => format!("{}:", "-".repeat(w - 1)),
                        ColumnAlign::Center => format!(":{}:", "-".repeat(w - 2)),
                    };
                    out.push(' ');
                    out.push_str(&dashes);
                    out.push_str(" |");
                }
                out.push_str(nl);
            }
            out.push_str(&self.prefix);
            out.push('|');
            let mut pos = Vec::new();
            for (c, w) in widths.iter().enumerate() {
                let cell = row.get(c).unwrap_or(&empty);
                let pad = w - display_width(cell);
                let (l, rt) = match self.aligns.get(c).copied().unwrap_or_default() {
                    ColumnAlign::Right => (pad, 0),
                    ColumnAlign::Center => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                out.push(' ');
                out.push_str(&" ".repeat(l));
                pos.push((out.len(), cell.len()));
                out.push_str(cell);
                out.push_str(&" ".repeat(rt));
                out.push_str(" |");
            }
            positions.push(pos);
            if r + 1 < self.rows.len() || self.rows.len() == 1 {
                out.push_str(nl);
            }
        }
        if self.rows.len() == 1 {
            // Header only: the delimiter row still has to follow it.
            out.push_str(&self.prefix);
            out.push('|');
            for w in &widths {
                out.push(' ');
                out.push_str(&"-".repeat(*w));
                out.push_str(" |");
            }
        }
        (out, positions)
    }

    /// Replace the table with its aligned rendering and put the cursor (or a
    /// selection of the whole cell with `select_cell`) at `target`.
    fn commit(&self, doc: &Document, target: Option<CellPos>, select_cell: bool, sel: Selection) -> CommandResult {
        let buf = doc.buffer();
        let (text, positions) = self.render(doc.format.line_ending.as_str());
        let old = buf.slice(self.range.clone()).into_owned();
        let mut ch = Changes::default();
        ch.replace_minimal(self.range.clone(), &old, &text);
        let selection = match target {
            Some(t) => {
                let row = t.row.min(positions.len() - 1);
                let (start, len) = positions[row][t.col.min(positions[row].len() - 1)];
                let start = self.range.start + start;
                if select_cell {
                    Selection { anchor: start, head: start + len }
                } else {
                    Selection::cursor(start + t.offset.min(len))
                }
            }
            None => ch.map_selection(sel),
        };
        ch.finish(selection)
    }
}

/// Align the table at byte `at`; `None` when there is no table there. The
/// result has no edits when the table is already aligned.
pub(crate) fn align_at(doc: &Document, at: usize, sel: Selection) -> Option<CommandResult> {
    let buf = doc.buffer();
    let mut table = table_at(buf, at)?;
    table.normalise();
    let r = sel.range();
    let inside = table.range.start <= r.start && r.end <= table.range.end;
    let target = (inside && sel.is_empty()).then(|| table.pos_of(buf, sel.head));
    Some(table.commit(doc, target, false, sel))
}

/// Run a table operation at the cursor; `None` outside a table.
pub(crate) fn run_op(doc: &Document, sel: Selection, op: TableOp) -> Option<CommandResult> {
    let buf = doc.buffer();
    let mut t = table_at(buf, sel.head)?;
    t.normalise();
    let pos = t.pos_of(buf, sel.head);
    let (row, col) = (pos.row, pos.col);
    let ncols = t.cols();
    let nrows = t.rows.len();
    let empty_row = || vec![String::new(); ncols];
    let at = |row, col| Some(CellPos { row, col, offset: 0 });
    let (target, select) = match op {
        TableOp::Format => (Some(pos), false),
        TableOp::NextCell => {
            if col + 1 < ncols {
                (at(row, col + 1), true)
            } else {
                if row + 1 == nrows {
                    t.rows.push(empty_row());
                }
                (at(row + 1, 0), true)
            }
        }
        TableOp::PrevCell => match (row, col) {
            (0, 0) => (at(0, 0), true),
            (r, 0) => (at(r - 1, ncols - 1), true),
            (r, c) => (at(r, c - 1), true),
        },
        TableOp::InsertRowAbove | TableOp::InsertRowBelow => {
            let above = op == TableOp::InsertRowAbove;
            let i = if above { row.max(1) } else { row + 1 };
            t.rows.insert(i, empty_row());
            (at(i, col), false)
        }
        TableOp::DeleteRow => {
            if row == 0 {
                return Some(CommandResult { edits: Vec::new(), selection: sel });
            }
            t.rows.remove(row);
            (at(row.min(t.rows.len() - 1), col), false)
        }
        TableOp::InsertColumnLeft | TableOp::InsertColumnRight => {
            let i = if op == TableOp::InsertColumnLeft { col } else { col + 1 };
            for r in &mut t.rows {
                r.insert(i, String::new());
            }
            t.aligns.insert(i, ColumnAlign::None);
            (at(row, i), false)
        }
        TableOp::DeleteColumn => {
            if ncols == 1 {
                return Some(CommandResult { edits: Vec::new(), selection: sel });
            }
            for r in &mut t.rows {
                r.remove(col);
            }
            t.aligns.remove(col);
            (at(row, col.min(ncols - 2)), false)
        }
        TableOp::MoveRowUp => {
            if row < 2 {
                return Some(CommandResult { edits: Vec::new(), selection: sel });
            }
            t.rows.swap(row, row - 1);
            (Some(CellPos { row: row - 1, ..pos }), false)
        }
        TableOp::MoveRowDown => {
            if row == 0 || row + 1 >= nrows {
                return Some(CommandResult { edits: Vec::new(), selection: sel });
            }
            t.rows.swap(row, row + 1);
            (Some(CellPos { row: row + 1, ..pos }), false)
        }
        TableOp::MoveColumnLeft | TableOp::MoveColumnRight => {
            let other = if op == TableOp::MoveColumnLeft { col.checked_sub(1) } else { Some(col + 1) };
            let Some(other) = other.filter(|&o| o < ncols) else {
                return Some(CommandResult { edits: Vec::new(), selection: sel });
            };
            for r in &mut t.rows {
                r.swap(col, other);
            }
            t.aligns.swap(col, other);
            (Some(CellPos { col: other, ..pos }), false)
        }
        TableOp::Align(a) => {
            t.aligns[col] = a;
            (Some(pos), false)
        }
    };
    Some(t.commit(doc, target, select, sel))
}

/// A new empty table, `rows` counting the header row.
pub(crate) fn insert_table(doc: &Document, sel: Selection, rows: u8, cols: u8) -> CommandResult {
    let (rows, cols) = (rows.clamp(1, 100) as usize, cols.clamp(1, 100) as usize);
    let t = Table {
        range: 0..0,
        first_line: 0,
        prefix: String::new(),
        rows: vec![vec![String::new(); cols]; rows],
        aligns: vec![ColumnAlign::None; cols],
        cells: Vec::new(),
    };
    let (text, positions) = t.render("\n");
    insert_block(doc, sel, &text, After::In(positions[0][0].0))
}

/// Parse CSV or TSV: tab-separated when the first line has a tab, else
/// semicolons when they outnumber commas there, else commas. Quoted fields
/// may contain separators, newlines and `""`.
pub fn parse_delimited(text: &str) -> Vec<Vec<String>> {
    let first = text.lines().next().unwrap_or("");
    let sep = if first.contains('\t') {
        '\t'
    } else if first.matches(';').count() > first.matches(',').count() {
        ';'
    } else {
        ','
    };
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut at_start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            c if c == sep => {
                row.push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_start = true;
            }
            _ => {
                field.push(c);
                at_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn escape_cell(s: &str) -> String {
    s.trim().replace('|', "\\|").replace("\r\n", "<br>").replace(['\n', '\r'], "<br>")
}

/// Ctrl+Shift+V: paste CSV/TSV as a table (first row is the header).
pub(crate) fn paste_table(doc: &Document, sel: Selection, text: &str) -> CommandResult {
    let rows: Vec<Vec<String>> =
        parse_delimited(text).into_iter().map(|r| r.iter().map(|c| escape_cell(c)).collect()).collect();
    if rows.is_empty() {
        return CommandResult { edits: Vec::new(), selection: sel };
    }
    let mut t = Table { range: 0..0, first_line: 0, prefix: String::new(), rows, aligns: Vec::new(), cells: Vec::new() };
    t.normalise();
    let (text, _) = t.render("\n");
    insert_block(doc, sel, &text, After::Below)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("日本"), 4);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(display_width("👍🏽"), 2);
    }

    #[test]
    fn split_rows() {
        let cells = |s: &str| split_row(s).into_iter().map(|(c, _)| s[c].to_string()).collect::<Vec<_>>();
        assert_eq!(cells("| a | b |"), ["a", "b"]);
        assert_eq!(cells("a | b"), ["a", "b"]);
        assert_eq!(cells("| a \\| x | |"), ["a \\| x", ""]);
        assert_eq!(cells("|a|"), ["a"]);
    }

    #[test]
    fn csv_parsing() {
        assert_eq!(parse_delimited("a,b\n1,\"x, \"\"y\"\"\"\n"), vec![vec!["a", "b"], vec!["1", "x, \"y\""]]);
        assert_eq!(parse_delimited("a\tb\r\n1\t2"), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(parse_delimited("a;b;c,d\n1;2"), vec![vec!["a", "b", "c,d"], vec!["1", "2"]]);
        assert_eq!(parse_delimited("\"multi\nline\",z"), vec![vec!["multi\nline", "z"]]);
    }
}
