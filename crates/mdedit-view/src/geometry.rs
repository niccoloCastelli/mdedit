//! Positions inside one shaped paragraph: caret, hit testing and range
//! rectangles. Coordinates are relative to the paragraph's top-left; byte
//! offsets are into the paragraph text, which may hold `\n` (one cosmic
//! buffer line per text line, `line_starts` maps between the two).

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::graphics::text::cosmic_text::LayoutRun;
use iced::{Point, Rectangle, Size};

/// Byte offsets where each `\n`-separated line of `text` starts.
pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect()
}

fn line_of(starts: &[usize], offset: usize) -> (usize, usize) {
    let line = starts.partition_point(|&s| s <= offset).saturating_sub(1);
    (line, offset - starts[line])
}

fn run_range(run: &LayoutRun<'_>) -> (usize, usize) {
    let start = run.glyphs.iter().map(|g| g.start).min().unwrap_or(0);
    let end = run.glyphs.iter().map(|g| g.end).max().unwrap_or(start);
    (start, end)
}

fn x_in_run(run: &LayoutRun<'_>, index: usize) -> f32 {
    let mut end_x: f32 = 0.0;
    for g in run.glyphs {
        let rtl = g.level.is_rtl();
        if index > g.start && index < g.end {
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

/// The visual rows of a text line, in order, with the byte range each
/// covers (rows meet, so every offset belongs to one row).
fn rows_of_line<'a>(p: &'a Paragraph, line: usize) -> Vec<LayoutRun<'a>> {
    p.buffer().layout_runs().filter(|r| r.line_i == line).collect()
}

/// Caret rectangle (2 px wide) for `offset`; `fallback` is the line height
/// used when the paragraph has no rows (empty text).
pub(crate) fn caret(p: &Paragraph, starts: &[usize], offset: usize, fallback: f32) -> Rectangle {
    let (line, index) = line_of(starts, offset);
    let rows = rows_of_line(p, line);
    let pick = rows.iter().rposition(|r| run_range(r).0 <= index || r.glyphs.is_empty()).unwrap_or(0);
    match rows.get(pick) {
        Some(run) => Rectangle::new(Point::new(x_in_run(run, index), run.line_top), Size::new(2.0, run.line_height)),
        None => {
            // A line without a run: under the previous one.
            let top = p
                .buffer()
                .layout_runs()
                .filter(|r| r.line_i < line)
                .map(|r| r.line_top + r.line_height)
                .fold(0.0, f32::max);
            Rectangle::new(Point::new(0.0, top), Size::new(2.0, fallback))
        }
    }
}

/// Byte offset nearest to `point`.
pub(crate) fn hit(p: &Paragraph, starts: &[usize], text_len: usize, point: Point) -> usize {
    let buffer = p.buffer();
    if let Some(c) = buffer.hit(point.x, point.y.max(0.0)) {
        let base = starts.get(c.line).copied().unwrap_or(text_len);
        return (base + c.index).min(text_len);
    }
    if point.y < 0.0 { 0 } else { text_len }
}

/// Rectangles covering bytes `start..end` (one per visual row); a selected
/// line break shows as a narrow stub at the end of its row.
pub(crate) fn range_rects(p: &Paragraph, starts: &[usize], start: usize, end: usize) -> Vec<Rectangle> {
    let mut out = Vec::new();
    if start >= end {
        return out;
    }
    let (l0, i0) = line_of(starts, start);
    let (l1, i1) = line_of(starts, end);
    let runs: Vec<LayoutRun<'_>> = p.buffer().layout_runs().collect();
    for (k, run) in runs.iter().enumerate() {
        if run.line_i < l0 || run.line_i > l1 {
            continue;
        }
        let line_len = run.text.len();
        let a = if run.line_i == l0 { i0 } else { 0 };
        let b = if run.line_i == l1 { i1 } else { line_len };
        let (rs, re) = run_range(run);
        let last_row = runs.get(k + 1).is_none_or(|n| n.line_i != run.line_i);
        let (s, e) = (a.max(rs), b.min(re));
        if s < e {
            let (x0, x1) = (x_in_run(run, s), x_in_run(run, e));
            let (x0, x1) = (x0.min(x1), x0.max(x1));
            out.push(Rectangle::new(Point::new(x0, run.line_top), Size::new((x1 - x0).max(1.0), run.line_height)));
        }
        if last_row && run.line_i < l1 && a <= line_len {
            let x = x_in_run(run, re);
            out.push(Rectangle::new(Point::new(x, run.line_top), Size::new(6.0, run.line_height)));
        }
    }
    out
}

/// Byte offsets of the start and end of the visual row holding `offset`.
pub(crate) fn row_bounds(p: &Paragraph, starts: &[usize], offset: usize) -> (usize, usize) {
    let (line, index) = line_of(starts, offset);
    let rows = rows_of_line(p, line);
    let pick = rows.iter().rposition(|r| run_range(r).0 <= index).unwrap_or(0);
    match rows.get(pick) {
        Some(run) => {
            let (s, e) = run_range(run);
            let next = rows.get(pick + 1).map(|r| run_range(r).0);
            (starts[line] + s, starts[line] + next.unwrap_or(e))
        }
        None => (offset, offset),
    }
}
