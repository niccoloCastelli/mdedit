//! Table grid operations in View mode (DESIGN_SYSTEM 8.4): rebuild the
//! table's markdown from its cells after inserting, deleting or aligning a
//! row or column. The result is auto-aligned (columns padded to the same
//! width), which is also what Edit mode produces.

use mdedit_render::Align;
use unicode_segmentation::UnicodeSegmentation;

use crate::layout::TableOp;

/// A table as cell texts (row 0 is the header) and column alignments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Grid {
    pub rows: Vec<Vec<String>>,
    pub align: Vec<Align>,
}

impl Grid {
    pub fn cols(&self) -> usize {
        self.align.len().max(self.rows.iter().map(Vec::len).max().unwrap_or(0)).max(1)
    }

    /// Apply `op` with the cursor in cell (`row`, `col`); returns the cell
    /// the cursor should move to, or `None` if the op does not apply.
    pub fn apply(&mut self, op: TableOp, row: usize, col: usize) -> Option<(usize, usize)> {
        let cols = self.cols();
        self.normalize(cols);
        let empty_row = || vec![String::new(); cols];
        match op {
            TableOp::RowAbove => {
                let at = row.max(1);
                self.rows.insert(at, empty_row());
                Some((at, col))
            }
            TableOp::RowBelow => {
                let at = (row + 1).min(self.rows.len());
                self.rows.insert(at, empty_row());
                Some((at, col))
            }
            TableOp::ColLeft | TableOp::ColRight => {
                let at = if op == TableOp::ColLeft { col } else { col + 1 }.min(cols);
                for r in &mut self.rows {
                    r.insert(at, String::new());
                }
                self.align.insert(at, Align::None);
                Some((row, at))
            }
            TableOp::DeleteRow => {
                if row == 0 || row >= self.rows.len() {
                    return None;
                }
                self.rows.remove(row);
                Some((row.min(self.rows.len() - 1), col))
            }
            TableOp::DeleteCol => {
                if cols <= 1 || col >= cols {
                    return None;
                }
                for r in &mut self.rows {
                    r.remove(col);
                }
                self.align.remove(col);
                Some((row, col.min(cols - 2)))
            }
            TableOp::Align(a) => {
                let target = if self.align[col] == a { Align::None } else { a };
                self.align[col] = target;
                Some((row, col))
            }
        }
    }

    fn normalize(&mut self, cols: usize) {
        self.align.resize(cols, Align::None);
        for r in &mut self.rows {
            r.resize(cols, String::new());
        }
        if self.rows.is_empty() {
            self.rows.push(vec![String::new(); cols]);
        }
    }

    /// The markdown for the grid, and the byte offset (from its start) of
    /// each cell's content.
    pub fn to_markdown(&self, eol: &str) -> (String, Vec<Vec<usize>>) {
        let cols = self.cols();
        let width = |s: &str| s.graphemes(true).count();
        let mut widths = vec![3usize; cols];
        for r in &self.rows {
            for (i, cell) in r.iter().enumerate().take(cols) {
                widths[i] = widths[i].max(width(cell));
            }
        }
        let mut out = String::new();
        let mut offsets = Vec::new();
        let write_row = |out: &mut String, cells: &[String]| {
            let mut row_offsets = Vec::new();
            out.push('|');
            for (i, w) in widths.iter().enumerate() {
                let cell = cells.get(i).map_or("", String::as_str);
                out.push(' ');
                row_offsets.push(out.len());
                out.push_str(cell);
                out.extend(std::iter::repeat_n(' ', w - width(cell)));
                out.push_str(" |");
            }
            row_offsets
        };
        for (n, r) in self.rows.iter().enumerate() {
            if n > 0 {
                out.push_str(eol);
            }
            offsets.push(write_row(&mut out, r));
            if n == 0 {
                out.push_str(eol);
                out.push('|');
                for (i, w) in widths.iter().enumerate() {
                    let (l, rt) = match self.align.get(i) {
                        Some(Align::Left) => (":", ""),
                        Some(Align::Center) => (":", ":"),
                        Some(Align::Right) => ("", ":"),
                        _ => ("", ""),
                    };
                    out.push(' ');
                    out.push_str(l);
                    out.extend(std::iter::repeat_n('-', w - l.len() - rt.len()));
                    out.push_str(rt);
                    out.push_str(" |");
                }
            }
        }
        (out, offsets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Grid {
        Grid {
            rows: vec![vec!["Name".into(), "Qty".into()], vec!["apple".into(), "3".into()]],
            align: vec![Align::None, Align::Right],
        }
    }

    #[test]
    fn writes_aligned_markdown() {
        let (md, offsets) = grid().to_markdown("\n");
        assert_eq!(md, "| Name  | Qty |\n| ----- | --: |\n| apple | 3   |");
        assert_eq!(&md[offsets[1][0]..offsets[1][0] + 5], "apple");
        let parsed = mdedit_render::layout_document(&md);
        assert!(matches!(&parsed[0], mdedit_render::LayoutNode::Table { rows, .. } if rows.len() == 1));
    }

    #[test]
    fn row_and_column_ops() {
        let mut g = grid();
        assert_eq!(g.apply(TableOp::RowBelow, 1, 0), Some((2, 0)));
        assert_eq!(g.rows.len(), 3);
        assert_eq!(g.apply(TableOp::RowAbove, 0, 1), Some((1, 1)), "above the header means first body row");
        assert_eq!(g.apply(TableOp::ColLeft, 0, 0), Some((0, 0)));
        assert_eq!(g.rows[0], vec!["", "Name", "Qty"]);
        assert_eq!(g.apply(TableOp::DeleteCol, 0, 0), Some((0, 0)));
        assert_eq!(g.rows[0], vec!["Name", "Qty"]);
        assert_eq!(g.apply(TableOp::DeleteRow, 0, 0), None, "the header stays");
        assert_eq!(g.apply(TableOp::DeleteRow, 1, 0), Some((1, 0)));
        assert_eq!(g.apply(TableOp::Align(Align::Center), 1, 0), Some((1, 0)));
        assert_eq!(g.align[0], Align::Center);
        g.apply(TableOp::Align(Align::Center), 1, 0);
        assert_eq!(g.align[0], Align::None, "same alignment again clears it");
    }

    #[test]
    fn wide_graphemes_pad_by_cluster() {
        let g = Grid { rows: vec![vec!["é".into()], vec!["abcd".into()]], align: vec![Align::None] };
        let (md, _) = g.to_markdown("\r\n");
        assert_eq!(md, "| é    |\r\n| ---- |\r\n| abcd |");
    }
}
