//! Large-file numbers for docs/PERF.md (SPEC 8).
//!
//! ```bash
//! cargo run --release -p mdedit-core --example large_file -- /path/to/big.md [size_mb]
//! ```
//!
//! Generates a markdown-like file of `size_mb` (default 1024) at the path if
//! it does not exist, then opens it in large-file mode and measures open,
//! indexing, line lookups, edits, streamed literal and regex search,
//! replace-all and save, with resident memory after each step (Linux,
//! from /proc/self/status).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mdedit_core::search::{self, SearchEvent, SearchQuery};
use mdedit_core::{Edit, EditOrigin, TextBuffer, TxnId, io};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: large_file <path> [size_mb]");
    let size_mb: usize = args.next().map_or(1024, |s| s.parse().expect("size in MB"));
    let path = Path::new(&path);
    if !path.exists() {
        let t = Instant::now();
        generate(path, size_mb << 20);
        println!("generated {} MB in {:.1?}", size_mb, t.elapsed());
    }
    let file_len = std::fs::metadata(path).unwrap().len();
    println!("file: {} ({:.0} MB)", path.display(), file_len as f64 / 1048576.0);
    let base = Mem::now();
    println!("baseline: {base}");

    let t = Instant::now();
    let mut doc = io::load(path, io::DEFAULT_LARGE_FILE_THRESHOLD).unwrap();
    let open = t.elapsed();
    let first: usize = (0..60).map(|i| doc.buffer().line(i).len()).sum();
    let first_screen = t.elapsed();
    println!("open: {open:.1?}, first 60 lines read after {first_screen:.1?} ({first} bytes)");
    let mapped = doc.mapped().unwrap().clone();
    let mut last = 0.0;
    while !mapped.is_indexed() {
        let p = mapped.index_progress();
        if p - last >= 0.25 {
            println!("  indexing {:.0}% at {:.1?}", p * 100.0, t.elapsed());
            last = p;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let lines = doc.buffer().len_lines().unwrap();
    println!("indexed: {lines} lines after {:.1?}; utf8 valid: {:?}", t.elapsed(), mapped.utf8_valid());
    drop(mapped);
    println!("after open + index: {}", Mem::now().minus(&base));

    let t = Instant::now();
    let mut acc = 0;
    for i in 0..1000 {
        let line = (i * 7_919_993) % lines;
        acc += doc.buffer().line_to_byte(line);
        acc += doc.buffer().byte_to_line(acc % doc.buffer().len_bytes());
    }
    println!("line_to_byte + byte_to_line, random: {:.1?} per pair ({acc})", t.elapsed() / 1000);

    let t = Instant::now();
    for i in 0..1000 {
        let line = (i * 7_919_993) % lines;
        acc += doc.buffer().line(line).len();
    }
    println!("line(i), random: {:.1?} per line", t.elapsed() / 1000);
    let t = Instant::now();
    let top = lines / 2;
    for i in 0..60 {
        acc += doc.buffer().line(top + i).len();
    }
    println!("one screen (60 x line(i)) mid-file: {:.1?} ({acc})", t.elapsed());

    let q = SearchQuery { pattern: "zebra42".into(), case_sensitive: true, ..Default::default() };
    let t = Instant::now();
    let mut n = 0;
    search::search(doc.buffer(), &q, |_| {
        n += 1;
        ControlFlow::Continue(())
    })
    .unwrap();
    println!(
        "search \"zebra42\" before edits, in place: {n} matches in {:.2?}; {}",
        t.elapsed(),
        Mem::now().minus(&base)
    );

    let t = Instant::now();
    let len = doc.buffer().len_bytes();
    for i in 0..10_000usize {
        let at = (i * 104_729_123) % len;
        let at = doc.buffer().line_to_byte(doc.buffer().byte_to_line(at));
        doc.apply(&Edit::insert(at, "edit ", EditOrigin::User, TxnId(i as u64)));
    }
    println!(
        "10k scattered edits: {:.1?} per edit, {} pieces; {}",
        t.elapsed() / 10_000,
        doc.mapped().unwrap().piece_count(),
        Mem::now().minus(&base)
    );
    let t = Instant::now();
    let mid = doc.buffer().len_bytes() / 2;
    for i in 0..10_000usize {
        doc.apply(&Edit::insert(mid + i, "x", EditOrigin::User, TxnId(0)));
    }
    println!("10k typed chars: {:.2?} per char", t.elapsed() / 10_000);

    for query in [
        SearchQuery { pattern: "zebra42".into(), case_sensitive: true, ..Default::default() },
        SearchQuery { pattern: "Quantum".into(), case_sensitive: false, whole_word: true, ..Default::default() },
        SearchQuery { pattern: r"^#{2,3} \w+ \d+$".into(), regex: true, case_sensitive: true, ..Default::default() },
    ] {
        let t = Instant::now();
        let job = search::spawn_search(doc.buffer().snapshot(), &query).unwrap();
        let (mut first, mut n) = (None, 0);
        for event in job.events.iter() {
            match event {
                SearchEvent::Matches(m) => {
                    first.get_or_insert(t.elapsed());
                    n += m.len();
                }
                SearchEvent::Progress { .. } => {}
                SearchEvent::Done { .. } => break,
            }
        }
        let total = t.elapsed();
        println!(
            "search {:?} (case {}, word {}, regex {}): {n} matches, first after {:.1?}, all in {total:.2?} ({:.2} GB/s); {}",
            query.pattern,
            query.case_sensitive,
            query.whole_word,
            query.regex,
            first.unwrap_or_default(),
            len as f64 / total.as_secs_f64() / 1e9,
            Mem::now().minus(&base)
        );
    }

    let t = Instant::now();
    let q = SearchQuery { pattern: "zebra42".into(), case_sensitive: true, ..Default::default() };
    let n = search::replace_all(&mut doc, &q, "okapi7").unwrap();
    println!("replace-all {n} matches: {:.2?}; {}", t.elapsed(), Mem::now().minus(&base));
    let t = Instant::now();
    doc.undo();
    println!("undo replace-all: {:.2?}", t.elapsed());
    let mut count = 0;
    search::search(doc.buffer(), &q, |_| {
        count += 1;
        ControlFlow::Continue(())
    })
    .unwrap();
    assert_eq!(count, n);

    let t = Instant::now();
    io::save(&mut doc, None).unwrap();
    println!(
        "save (stream + fsync + rename + remap): {:.2?}, pieces after: {}; {}",
        t.elapsed(),
        doc.mapped().unwrap().piece_count(),
        Mem::now().minus(&base)
    );
    let snapshot: Arc<dyn TextBuffer> = doc.buffer().snapshot();
    assert_eq!(snapshot.len_bytes(), std::fs::metadata(path).unwrap().len() as usize);
}

/// Deterministic markdown-ish text: headings, paragraphs, lists, code.
fn generate(path: &Path, size: usize) {
    const WORDS: &[&str] = &[
        "lorem", "ipsum", "dolor", "sit", "amet", "quantum", "Quantum", "editor", "buffer", "piece", "table",
        "rope", "markdown", "render", "window", "search", "index", "line", "byte", "memory", "map", "giorni",
        "café", "naïve", "über", "日本語",
    ];
    let mut out = BufWriter::with_capacity(1 << 20, File::create(path).unwrap());
    let mut rng = 0x2545F4914F6CDD1Du64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let (mut written, mut n) = (0, 0u64);
    let mut line = String::new();
    while written < size {
        line.clear();
        n += 1;
        let r = next();
        if n % 100_000 == 0 {
            line.push_str("A rare zebra42 appears here.");
        } else if r % 40 == 0 {
            line.push_str(["## ", "### ", "# "][(r >> 8) as usize % 3]);
            line.push_str(WORDS[(r >> 16) as usize % WORDS.len()]);
            line.push_str(&format!(" {}", n));
        } else if r % 9 == 0 {
            line.push_str("- ");
        }
        if line.is_empty() || line.starts_with("- ") {
            for _ in 0..(4 + next() % 14) {
                line.push_str(WORDS[next() as usize % WORDS.len()]);
                line.push(' ');
            }
            line.pop();
        }
        line.push('\n');
        out.write_all(line.as_bytes()).unwrap();
        written += line.len();
    }
    out.flush().unwrap();
}

/// Resident memory from /proc/self/status, in KB.
#[derive(Debug, Default, Clone, Copy)]
struct Mem {
    rss: i64,
    anon: i64,
    file: i64,
}

impl Mem {
    fn now() -> Self {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let field = |name: &str| -> i64 {
            status
                .lines()
                .find(|l| l.starts_with(name))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        Self { rss: field("VmRSS:"), anon: field("RssAnon:"), file: field("RssFile:") + field("RssShmem:") }
    }

    fn minus(self, base: &Self) -> Self {
        Self { rss: self.rss - base.rss, anon: self.anon - base.anon, file: self.file - base.file }
    }
}

impl std::fmt::Display for Mem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mb = |kb: i64| kb as f64 / 1024.0;
        write!(f, "RSS {:+.1} MB (anon {:+.1} MB, file-backed {:+.1} MB)", mb(self.rss), mb(self.anon), mb(self.file))
    }
}
