//! Positions inside one shaped line: caret placement, hit testing,
//! selection rectangles and visual rows. All coordinates are relative to the
//! top-left of the line's paragraph; byte indices are relative to the line.

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::graphics::text::cosmic_text::{Buffer, Cursor, LayoutRun};

/// One visual row of a wrapped line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Row {
    pub top: f32,
    pub height: f32,
    /// Byte range covered by the row's glyphs.
    pub start: usize,
    pub end: usize,
}

fn buffer(p: &Paragraph) -> &Buffer {
    p.buffer()
}

fn run_range(run: &LayoutRun<'_>) -> (usize, usize) {
    let start = run.glyphs.iter().map(|g| g.start).min().unwrap_or(0);
    let end = run.glyphs.iter().map(|g| g.end).max().unwrap_or(start);
    (start, end)
}

pub(crate) fn rows(p: &Paragraph, line_height: f32) -> Vec<Row> {
    let mut rows: Vec<Row> = buffer(p)
        .layout_runs()
        .map(|run| {
            let (start, end) = run_range(&run);
            Row { top: run.line_top, height: run.line_height, start, end }
        })
        .collect();
    if rows.is_empty() {
        rows.push(Row { top: 0.0, height: line_height, start: 0, end: 0 });
    }
    // Rows meet: the start of each row is the end of the previous one, so a
    // byte offset always belongs to exactly one row (wrapped spaces stay on
    // the row they were laid out on).
    for i in 1..rows.len() {
        rows[i].start = rows[i - 1].end;
    }
    rows[0].start = 0;
    rows
}

/// Index of the row holding byte `index`. An index at a wrap point belongs
/// to the row it starts.
pub(crate) fn row_of(rows: &[Row], index: usize) -> usize {
    rows.iter().rposition(|r| r.start <= index).unwrap_or(0)
}

/// Caret x and its row.
pub(crate) fn caret(p: &Paragraph, index: usize, line_height: f32) -> (f32, Row) {
    let rows = rows(p, line_height);
    let r = row_of(&rows, index);
    let row = rows[r];
    let Some(run) = buffer(p).layout_runs().nth(r) else {
        return (0.0, row);
    };
    (x_in_run(&run, index), row)
}

fn x_in_run(run: &LayoutRun<'_>, index: usize) -> f32 {
    let mut end_x: f32 = 0.0;
    for g in run.glyphs {
        let rtl = g.level.is_rtl();
        if index >= g.start && index < g.end {
            // Inside a cluster (ligature, multi-byte grapheme): interpolate.
            let frac = (index - g.start) as f32 / (g.end - g.start) as f32;
            return if rtl { g.x + g.w * (1.0 - frac) } else { g.x + g.w * frac };
        }
        if index == g.start {
            return if rtl { g.x + g.w } else { g.x };
        }
        end_x = end_x.max(if rtl { g.x } else { g.x + g.w });
    }
    end_x
}

/// Byte index nearest to `(x, y)`.
pub(crate) fn hit(p: &Paragraph, x: f32, y: f32, len: usize) -> usize {
    match buffer(p).hit(x, y) {
        Some(c) => c.index.min(len),
        None if y < 0.0 => 0,
        None => len,
    }
}

/// Byte index on row `row` nearest to `x` (for up/down movement).
pub(crate) fn index_at_x(p: &Paragraph, rows: &[Row], row: usize, x: f32, len: usize) -> usize {
    let row = rows[row.min(rows.len() - 1)];
    let i = hit(p, x, row.top + row.height / 2.0, len);
    // A hit past the end of a wrapped row lands on the next row's first byte;
    // keep the caret on this row instead.
    if i > row.start && i >= row.end && row.end < len { prev_boundary(p, row.end).max(row.start) } else { i }
}

fn prev_boundary(p: &Paragraph, index: usize) -> usize {
    let text = buffer(p).lines.first().map(|l| l.text()).unwrap_or("");
    let mut i = index.saturating_sub(1).min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Rectangles `(x, top, width, height)` covering bytes `start..end`.
/// `newline` extends the last row a little to show the line break is
/// selected.
pub(crate) fn selection_rects(
    p: &Paragraph,
    start: usize,
    end: usize,
    newline: bool,
    newline_w: f32,
) -> Vec<(f32, f32, f32, f32)> {
    let mut out = Vec::new();
    let runs: Vec<LayoutRun<'_>> = buffer(p).layout_runs().collect();
    let last = runs.len().saturating_sub(1);
    for (i, run) in runs.iter().enumerate() {
        let (rs, re) = run_range(run);
        let mut rect = if start < re && end > rs || (start == end && rs == re) {
            run.highlight(Cursor::new(run.line_i, start), Cursor::new(run.line_i, end)).map(|(x, w)| (x, w))
        } else {
            None
        };
        if i == last && newline {
            let end_x = run.line_w;
            rect = Some(match rect {
                Some((x, w)) => (x, (end_x - x).max(w) + newline_w),
                None if end >= re => (end_x, newline_w),
                None => continue,
            });
        }
        if let Some((x, w)) = rect
            && w > 0.0
        {
            out.push((x, run.line_top, w, run.line_height));
        }
    }
    if runs.is_empty() && newline {
        out.push((0.0, 0.0, newline_w, p.min_bounds().height));
    }
    out
}

/// Horizontal spans `(x, width, top, height)` of bytes `start..end`, one per
/// row (inline code backgrounds, highlights).
pub(crate) fn span_rects(p: &Paragraph, start: usize, end: usize) -> Vec<(f32, f32, f32, f32)> {
    buffer(p)
        .layout_runs()
        .filter_map(|run| {
            let (rs, re) = run_range(&run);
            if start >= re || end <= rs {
                return None;
            }
            let (x, w) = run.highlight(Cursor::new(run.line_i, start), Cursor::new(run.line_i, end))?;
            (w > 0.0).then_some((x, w, run.line_top, run.line_height))
        })
        .collect()
}

use iced::advanced::text::Paragraph as _;

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::text::{Alignment, LineHeight, Shaping, Text, Wrapping};
    use iced::{Font, Pixels, Size, alignment};

    fn para(text: &str, width: f32) -> Paragraph {
        Paragraph::with_text(Text {
            content: text,
            bounds: Size::new(width, f32::INFINITY),
            size: Pixels(16.0),
            line_height: LineHeight::Absolute(Pixels(20.0)),
            font: Font::MONOSPACE,
            align_x: Alignment::Left,
            align_y: alignment::Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::WordOrGlyph,
        })
    }

    #[test]
    fn caret_moves_right_and_hits_back() {
        let p = para("hello world", 1000.0);
        let (x0, _) = caret(&p, 0, 20.0);
        let (x5, row) = caret(&p, 5, 20.0);
        let (x11, _) = caret(&p, 11, 20.0);
        assert_eq!(x0, 0.0);
        assert!(x5 > x0 && x11 > x5);
        assert_eq!(row.top, 0.0);
        assert_eq!(hit(&p, x5 + 0.5, 5.0, 11), 5);
        assert_eq!(hit(&p, 9999.0, 5.0, 11), 11);
    }

    #[test]
    fn wrapped_rows_partition_the_line() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let p = para(text, 120.0);
        let rows = rows(&p, 20.0);
        assert!(rows.len() > 2, "{rows:?}");
        assert_eq!(rows[0].start, 0);
        assert_eq!(rows.last().unwrap().end, text.len());
        for w in rows.windows(2) {
            assert_eq!(w[0].end, w[1].start);
            assert!(w[1].top > w[0].top);
        }
        // Moving down keeps the column roughly.
        let (x, _) = caret(&p, 3, 20.0);
        assert_eq!(row_of(&rows, 3), 0);
        let below = index_at_x(&p, &rows, 1, x, text.len());
        assert_eq!(row_of(&rows, below), 1);
    }

    #[test]
    fn selection_covers_rows() {
        let p = para("abc def", 1000.0);
        let rects = selection_rects(&p, 1, 5, false, 4.0);
        assert_eq!(rects.len(), 1);
        assert!(rects[0].2 > 0.0);
        let with_nl = selection_rects(&p, 1, 7, true, 4.0);
        assert!(with_nl[0].2 > rects[0].2);
        let empty_line = para("", 1000.0);
        assert_eq!(selection_rects(&empty_line, 0, 0, true, 4.0).len(), 1);
    }
}
