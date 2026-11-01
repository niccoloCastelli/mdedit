//! Keystroke cost on a 10 MB document (SPEC 1: typing p99 < 8 ms end to
//! end; the buffer + undo + edit log share of that should be microseconds).
//!
//! The block index is a full reparse placeholder until the parsing
//! workstream lands, so these benches use a large-file-mode document
//! (no parse) to measure only the buffer path.

use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use mdedit_core::{Document, Edit, EditOrigin, RopeBuffer, TextBuffer, TxnId};

fn ten_mb() -> String {
    let para = "Lorem ipsum dolor sit amet, **consectetur** adipiscing elit. Sed do eiusmod tempor.\n\n";
    para.repeat(10 * 1024 * 1024 / para.len())
}

fn bench(c: &mut Criterion) {
    let text = ten_mb();

    c.bench_function("rope insert char in 10 MB", |b| {
        let mut buf = RopeBuffer::from_text(&text);
        let mid = buf.len_bytes() / 2;
        b.iter(|| buf.apply(&Edit::insert(mid, "x", EditOrigin::User, TxnId(0))));
    });

    c.bench_function("document type char in 10 MB (undo + log)", |b| {
        let mut doc = Document::with_buffer(Box::new(RopeBuffer::from_text(&text)), false);
        let mut at = doc.buffer().len_bytes() / 2;
        let now = Instant::now();
        b.iter(|| {
            doc.apply_at(&Edit::insert(at, "x", EditOrigin::User, TxnId(0)), now);
            at += 1;
        });
    });

    c.bench_function("load 10 MB into rope", |b| b.iter(|| RopeBuffer::from_text(&text)));

    c.bench_function("line lookup in 10 MB", |b| {
        let buf = RopeBuffer::from_text(&text);
        let n = buf.len_lines().unwrap();
        let mut i = 0;
        b.iter(|| {
            i = (i + 7919) % n;
            buf.line(i).len()
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
