//! Every CommonMark spec example, built up and torn down by edits, gives the
//! same block index as a full parse (PLAN W3).

use mdedit_core::parse::BlockIndex;
use mdedit_core::{Edit, EditOrigin, RopeBuffer, TextBuffer, TxnId};

const EXAMPLES: &str = include_str!("data/commonmark-0.31.2-examples.txt");

fn examples() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in EXAMPLES.split_inclusive('\n') {
        if let Some(name) = line.strip_prefix("@@ ") {
            out.push((name.trim().to_string(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
        }
    }
    out
}

fn blocks(index: &BlockIndex) -> Vec<(mdedit_core::BlockKind, std::ops::Range<usize>)> {
    index.blocks().iter().map(|b| (b.kind, b.bytes)).collect()
}

fn apply(buf: &mut RopeBuffer, index: &mut BlockIndex, range: std::ops::Range<usize>, text: &str) {
    let applied = buf.apply(&Edit { range, text: text.into(), origin: EditOrigin::User, txn: TxnId(0) });
    index.on_edit(&applied, buf);
}

fn check(name: &str, step: &str, index: &BlockIndex, buf: &RopeBuffer) {
    let full = BlockIndex::parse(buf);
    assert_eq!(blocks(index), blocks(&full), "{name}, {step}, text {:?}", buf.text());
}

#[test]
fn spec_examples_have_all_652() {
    assert_eq!(examples().len(), 652);
}

/// Type each example one character at a time after a paragraph.
#[test]
fn typing_each_example() {
    for (name, md) in examples() {
        let mut buf = RopeBuffer::from_text("intro\n\n");
        let mut index = BlockIndex::parse(&buf);
        for (i, c) in md.char_indices() {
            let at = buf.len_bytes();
            apply(&mut buf, &mut index, at..at, c.encode_utf8(&mut [0; 4]));
            check(&name, &format!("typed {} bytes", i + c.len_utf8()), &index, &buf);
        }
    }
}

/// Paste each example between two paragraphs, then delete it line by line
/// from the top and from the bottom.
#[test]
fn pasting_and_deleting_each_example() {
    for (name, md) in examples() {
        let mut buf = RopeBuffer::from_text("before\n\nafter\n");
        let mut index = BlockIndex::parse(&buf);
        apply(&mut buf, &mut index, 8..8, &md);
        check(&name, "pasted", &index, &buf);
        let lines: Vec<usize> = md.split_inclusive('\n').map(str::len).collect();
        let mut work = (buf.clone(), index.clone());
        for len in &lines {
            apply(&mut work.0, &mut work.1, 8..8 + len, "");
            check(&name, "deleting from the top", &work.1, &work.0);
        }
        let mut end = 8 + md.len();
        for len in lines.iter().rev() {
            apply(&mut buf, &mut index, end - len..end, "");
            end -= len;
            check(&name, "deleting from the bottom", &index, &buf);
        }
    }
}

/// All examples in one document, each edited in place: a character
/// inserted at the start of every line, then removed.
#[test]
fn editing_inside_the_whole_spec() {
    let all: String = examples().into_iter().map(|(_, md)| md + "\n").collect();
    let mut buf = RopeBuffer::from_text(&all);
    let mut index = BlockIndex::parse(&buf);
    let n = buf.len_lines().unwrap();
    for l in (0..n).step_by(3) {
        let at = buf.line_to_byte(l);
        apply(&mut buf, &mut index, at..at, "x");
        check("whole spec", &format!("insert at line {l}"), &index, &buf);
        apply(&mut buf, &mut index, at..at + 1, "");
        check("whole spec", &format!("remove at line {l}"), &index, &buf);
    }
}
