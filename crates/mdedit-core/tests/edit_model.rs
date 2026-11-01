//! Property tests: random edit sequences on a `Document` behave like the
//! same edits on a plain `String`, and undo/redo walk back and forth through
//! exactly the same texts.

use std::time::{Duration, Instant};

use mdedit_core::{Document, Edit, EditOrigin, TxnId};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    /// Insert at a position given as a fraction of the length.
    Insert {
        at: f64,
        text: String,
    },
    Delete {
        at: f64,
        len: usize,
    },
    Undo,
    Redo,
    /// Advance the clock past the grouping pause.
    Pause,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0.0..=1.0, "[a-z \\n]{1,3}|é|日本|🦀|\r\n").prop_map(|(at, text)| Op::Insert { at, text }),
        2 => (0.0..=1.0, 1usize..6).prop_map(|(at, len)| Op::Delete { at, len }),
        1 => Just(Op::Undo),
        1 => Just(Op::Redo),
        1 => Just(Op::Pause),
    ]
}

/// Largest char boundary <= `i`.
fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn document_matches_string_model(initial in "[a-z\\n é]{0,40}", ops in prop::collection::vec(op(), 1..60)) {
        let mut doc = Document::from_text(&initial);
        // Text after each undo step, newest last; `redo` holds undone texts.
        let mut model = initial.clone();
        let mut now = Instant::now();
        for op in ops {
            match op {
                Op::Insert { at, text } => {
                    let at = floor_boundary(&model, (at * model.len() as f64) as usize);
                    doc.apply_at(&Edit::insert(at, text.clone(), EditOrigin::User, TxnId(0)), now);
                    model.insert_str(at, &text);
                }
                Op::Delete { at, len } => {
                    let start = floor_boundary(&model, (at * model.len() as f64) as usize);
                    let end = floor_boundary(&model, start + len);
                    if start < end {
                        doc.apply_at(&Edit::delete(start..end, EditOrigin::User, TxnId(0)), now);
                        model.replace_range(start..end, "");
                    }
                }
                Op::Undo => {
                    doc.undo();
                    model = doc.buffer().text().into_owned();
                }
                Op::Redo => {
                    doc.redo();
                    model = doc.buffer().text().into_owned();
                }
                Op::Pause => now += Duration::from_secs(2),
            }
            prop_assert_eq!(doc.buffer().text(), model.as_str());
            prop_assert_eq!(doc.buffer().len_bytes(), model.len());
        }

        // Undoing everything returns to the initial text and a clean
        // document; redoing as many steps returns to the final text.
        let last = doc.buffer().text().into_owned();
        let mut steps = 0;
        while doc.undo().is_some() {
            steps += 1;
        }
        prop_assert_eq!(doc.buffer().text(), initial.as_str());
        prop_assert!(!doc.is_dirty());
        for _ in 0..steps {
            doc.redo();
        }
        prop_assert_eq!(doc.buffer().text(), last.as_str());
    }

    #[test]
    fn lines_match_model(text in "([a-z]{0,5}(\\n|\\r\\n|\\r)?){0,20}") {
        let doc = Document::from_text(&text);
        let buf = doc.buffer();
        // CommonMark line endings: LF, CRLF, CR.
        let expected: Vec<&str> = text.split("\r\n").flat_map(|p| p.split(['\n', '\r'])).collect();
        let n = buf.len_lines().unwrap();
        prop_assert_eq!(n, expected.len());
        for (i, line) in expected.iter().enumerate() {
            prop_assert_eq!(buf.line(i), *line);
            prop_assert_eq!(buf.byte_to_line(buf.line_to_byte(i)), i);
        }
    }
}
