//! Keystroke-to-reparse cost on a 10 MB document (ARCHITECTURE 3.2: a
//! typical keystroke reparses one paragraph, microseconds) and Edit-mode
//! highlighting of one screen.

use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use mdedit_core::highlight::highlight_lines;
use mdedit_core::parse::BlockIndex;
use mdedit_core::{Document, Edit, EditOrigin, RopeBuffer, TextBuffer, TxnId};

/// About 10 MB of mixed markdown: headings, paragraphs, lists, quotes,
/// fenced code, tables and link reference definitions.
fn ten_mb() -> String {
    let section = "## Section heading\n\n\
        Lorem ipsum dolor sit amet, **consectetur** adipiscing elit, sed do *eiusmod* tempor \
        incididunt ut labore et [dolore](https://example.com) magna aliqua.\nUt enim ad minim veniam, \
        quis `nostrud` exercitation ullamco laboris.\n\n\
        - first item\n- second item with [a ref][r]\n  continued\n- third\n\n\
        > A quote that\n> spans two lines.\n\n\
        ```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n\
        | a | b |\n|---|---|\n| 1 | 2 |\n\n\
        [r]: https://example.com/ref\n\n";
    section.repeat(10 * 1024 * 1024 / section.len())
}

fn bench(c: &mut Criterion) {
    let text = ten_mb();

    c.bench_function("reparse after a keystroke in 10 MB", |b| {
        let mut buf = RopeBuffer::from_text(&text);
        let mut index = BlockIndex::parse(&buf);
        // Inside a paragraph in the middle of the document.
        let mid = buf.len_bytes() / 2;
        let at = mid + buf.slice(mid..mid + 4096).find("Lorem").unwrap() + 6;
        let mut insert = true;
        b.iter(|| {
            let edit = if insert {
                Edit::insert(at, "x", EditOrigin::User, TxnId(0))
            } else {
                Edit::delete(at..at + 1, EditOrigin::User, TxnId(0))
            };
            insert = !insert;
            let applied = buf.apply(&edit);
            index.on_edit(&applied, &buf);
        });
    });

    c.bench_function("reparse after Enter in a list in 10 MB", |b| {
        let mut buf = RopeBuffer::from_text(&text);
        let mut index = BlockIndex::parse(&buf);
        let mid = buf.len_bytes() / 2;
        let at = mid + buf.slice(mid..mid + 4096).find("- second").unwrap();
        let mut insert = true;
        b.iter(|| {
            let edit = if insert {
                Edit::insert(at, "- new\n", EditOrigin::User, TxnId(0))
            } else {
                Edit::delete(at..at + 6, EditOrigin::User, TxnId(0))
            };
            insert = !insert;
            let applied = buf.apply(&edit);
            index.on_edit(&applied, &buf);
        });
    });

    c.bench_function("document keystroke in parsed 10 MB (buffer + undo + log + reparse)", |b| {
        let mut doc = Document::from_text(&text);
        let mid = doc.buffer().len_bytes() / 2;
        let mut at = mid + doc.buffer().slice(mid..mid + 4096).find("Lorem").unwrap() + 6;
        let now = Instant::now();
        b.iter(|| {
            doc.apply_at(&Edit::insert(at, "x", EditOrigin::User, TxnId(0)), now);
            at += 1;
        });
    });

    c.bench_function("highlight 60 lines of 10 MB", |b| {
        let buf = RopeBuffer::from_text(&text);
        let index = BlockIndex::parse(&buf);
        let mid = buf.len_lines().unwrap() / 2;
        b.iter(|| highlight_lines(&index, &buf, mid..mid + 60));
    });

    let mut group = c.benchmark_group("slow");
    group.sample_size(10);
    group.bench_function("full parse of 10 MB", |b| {
        let buf = RopeBuffer::from_text(&text);
        b.iter(|| BlockIndex::parse(&buf));
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
