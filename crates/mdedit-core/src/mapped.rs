//! `MappedBuffer` for large files (large-files & search workstream).
//!
//! SPEC 8, ARCHITECTURE 2: a `memmap2` map of the original file plus add
//! text for the edits, as a piece table in a persistent balanced tree
//! (B+ tree of pieces with `Arc` nodes, so a snapshot is one `Arc` clone and
//! an edit copies only the path it touches). A background thread scans the
//! map with `memchr` and records a checkpoint every
//! [`LINES_PER_CHECKPOINT`] lines; [`TextBuffer::len_lines`] is `None` until
//! it finishes. Line queries work before that, they just scan from the
//! nearest known checkpoint. Saving streams the pieces to a temp file
//! ([`MappedBuffer::save_to`]), building the new line index on the way, and
//! [`MappedBuffer::remap`] then maps the new file. [`MappedBuffer::disk_change`]
//! detects changes made by other programs.
//!
//! Gives the same text as `RopeBuffer` for the same edits (property test).
//! Two differences in line handling, both deliberate: only `\n` ends a line
//! (CRLF works, a lone CR is not a break as it is in the rope), and asking
//! for a line past the end clamps instead of panicking.
//!
//! Map safety: if another program truncates the mapped file in place,
//! reading the missing pages faults (SIGBUS on Unix). Editors and tools that
//! save atomically (rename) are harmless, the old inode stays mapped. The app
//! polls [`MappedBuffer::disk_change`] on watcher events and focus and offers
//! a reload, which is the mitigation SPEC 8 asks for.

use std::borrow::Cow;
use std::fmt;
use std::fs::{self, File, Metadata};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use memmap2::{Mmap, MmapMut};

use crate::buffer::TextBuffer;
use crate::edit::{AppliedEdit, Edit, EditOrigin};
use crate::io::{self, BOM, FileFormat, IoError, LineEnding};

/// Lines between two checkpoints of the line index.
pub const LINES_PER_CHECKPOINT: usize = 1024;

/// The indexer reads this much before publishing checkpoints and progress.
const INDEX_CHUNK: usize = 4 << 20;
/// Newline counts over ranges shorter than this skip the index.
const DIRECT_COUNT: usize = 64 * 1024;
const MAX_LEAF: usize = 64;
const MAX_CHILDREN: usize = 32;

/// What changed on disk since the file was mapped (or last saved by us).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskChange {
    Unchanged,
    /// Size, modification time or inode differ: offer a reload, and stop
    /// trusting the map if the file was rewritten in place.
    Modified,
    Removed,
}

/// Memory-mapped piece-table buffer for files above the large-file
/// threshold. Cloning is cheap (a snapshot).
#[derive(Clone)]
pub struct MappedBuffer {
    map: Arc<Mapping>,
    index: Arc<LineIndex>,
    root: Arc<Node>,
    version: u64,
    file: Option<(PathBuf, Stamp)>,
}

impl fmt::Debug for MappedBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MappedBuffer")
            .field("len_bytes", &self.root.bytes)
            .field("original_len", &self.map.text().len())
            .field("indexed", &self.is_indexed())
            .field("version", &self.version)
            .field("path", &self.file.as_ref().map(|(p, _)| p))
            .finish()
    }
}

/// Result of [`MappedBuffer::save_to`], to hand to [`MappedBuffer::remap`]
/// (possibly back on the UI thread after a background save).
#[derive(Debug)]
pub struct Remap {
    path: PathBuf,
    version: u64,
    len: usize,
    bom: bool,
    lines: LineCounter,
}

impl MappedBuffer {
    /// Map `path`. The BOM, if any, is not part of the text (it is recorded in
    /// the returned format, as for rope documents). `read_only` is left
    /// false: invalid UTF-8 is detected by the indexer ([`Self::utf8_valid`]),
    /// and saving copies mapped bytes verbatim, so it cannot corrupt them.
    pub fn open(path: &Path) -> Result<(Self, FileFormat), IoError> {
        let file = File::open(path)?;
        let meta = file.metadata()?;
        // SAFETY: the map is only read. Another process changing the file
        // under us is the documented risk (module docs), detected through
        // `disk_change`.
        let map = if meta.len() == 0 { None } else { Some(unsafe { Mmap::map(&file)? }) };
        let mapping = Mapping::new(map);
        let head = &mapping.text()[..mapping.text().len().min(64 * 1024)];
        let format = FileFormat {
            bom: mapping.base > 0,
            line_ending: LineEnding::detect(&String::from_utf8_lossy(head)),
            read_only: false,
        };
        let mut buf = Self::from_mapping(mapping);
        buf.file = Some((path.to_path_buf(), Stamp::of(&meta)));
        Ok((buf, format))
    }

    /// A buffer over an anonymous map holding `text` (tests, benches).
    pub fn from_text(text: &str) -> Self {
        let map = (!text.is_empty()).then(|| {
            let mut m = MmapMut::map_anon(text.len()).expect("anonymous map");
            m.copy_from_slice(text.as_bytes());
            m.make_read_only().expect("anonymous map")
        });
        Self::from_mapping(Mapping { map, base: 0, file_backed: false })
    }

    fn from_mapping(map: Mapping) -> Self {
        let map = Arc::new(map);
        let len = map.text().len();
        let index = Arc::new(LineIndex::default());
        let root = Arc::new(Node::leaf(if len == 0 { vec![] } else { vec![Piece::map(0, len)] }));
        spawn_indexer(map.clone(), index.clone());
        Self { map, index, root, version: 0, file: None }
    }

    /// The line index is complete (`len_lines` is `Some`).
    pub fn is_indexed(&self) -> bool {
        self.index.done.load(Ordering::Acquire)
    }

    /// Indexing progress in 0..=1, for the scrollbar.
    pub fn index_progress(&self) -> f32 {
        let len = self.map.text().len();
        if self.is_indexed() || len == 0 {
            1.0
        } else {
            self.index.scanned.load(Ordering::Relaxed) as f32 / len as f32
        }
    }

    /// Block until indexing finished (tests, benches).
    pub fn wait_indexed(&self) {
        while !self.is_indexed() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Whether the mapped file is valid UTF-8; `None` until indexed. Invalid
    /// files show replacement characters, so the app should open them
    /// read-only like small ones.
    pub fn utf8_valid(&self) -> Option<bool> {
        match self.index.utf8.load(Ordering::Acquire) {
            UTF8_VALID => Some(true),
            UTF8_INVALID => Some(false),
            _ => None,
        }
    }

    /// The mapped file, if this buffer came from one.
    pub fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(|(p, _)| p.as_path())
    }

    /// Compare the file on disk with what was mapped. Cheap (one `stat`).
    pub fn disk_change(&self) -> DiskChange {
        let Some((path, stamp)) = &self.file else { return DiskChange::Unchanged };
        match fs::metadata(path) {
            Ok(meta) if Stamp::of(&meta) == *stamp => DiskChange::Unchanged,
            Ok(_) => DiskChange::Modified,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DiskChange::Removed,
            Err(_) => DiskChange::Modified,
        }
    }

    /// Number of pieces (diagnostics; grows with edits).
    pub fn piece_count(&self) -> usize {
        self.root.pieces()
    }

    /// Drop the mapped pages of `range` from this process's resident set.
    /// They stay in the OS page cache and are read back on access; workers
    /// that stream the whole file (search) call this behind them so RSS
    /// stays flat.
    pub fn release_pages(&self, range: Range<usize>) {
        walk(&self.root, 0, &range, &mut |p, r| {
            if let Src::Map = p.src {
                self.map.release(p.start + r.start..p.start + r.end);
            }
        });
    }

    /// Stream the text to `path` atomically (as [`io::write_atomic`]),
    /// building the line index of the new file on the way. Works on a
    /// snapshot from a worker thread. Pass the result to [`Self::remap`].
    pub fn save_to(&self, path: &Path, format: FileFormat) -> Result<Remap, IoError> {
        let mut lines = LineCounter::default();
        let len = self.len_bytes();
        let real = io::write_chunks_atomic(path, self.chunks(0..len).inspect(|c| lines.feed(c)), format)?;
        Ok(Remap { path: real, version: self.version, len, bom: format.bom, lines })
    }

    /// Map the file written by [`Self::save_to`] and drop the old map, so the
    /// piece table collapses to one piece and the replaced file's disk space
    /// is freed. Returns `false` (and changes nothing) if the buffer was
    /// edited since the save started; the next save remaps.
    pub fn remap(&mut self, saved: Remap) -> Result<bool, IoError> {
        if saved.version != self.version {
            return Ok(false);
        }
        let file = File::open(&saved.path)?;
        let meta = file.metadata()?;
        let expected = saved.len + if saved.bom { BOM.len() } else { 0 };
        if meta.len() != expected as u64 {
            return Ok(false);
        }
        // SAFETY: as in `open`.
        let map = if meta.len() == 0 { None } else { Some(unsafe { Mmap::map(&file)? }) };
        let mapping = Mapping::new(map);
        if mapping.text().len() != saved.len {
            return Ok(false);
        }
        let utf8 = match self.index.utf8.load(Ordering::Acquire) {
            UTF8_UNKNOWN => UTF8_VALID,
            state => state,
        };
        self.index = Arc::new(LineIndex {
            checkpoints: Mutex::new(saved.lines.checkpoints),
            scanned: AtomicUsize::new(saved.len),
            newlines: AtomicUsize::new(saved.lines.lines),
            utf8: AtomicU8::new(utf8),
            done: AtomicBool::new(true),
        });
        self.map = Arc::new(mapping);
        self.root = Arc::new(Node::leaf(if saved.len == 0 { vec![] } else { vec![Piece::map(0, saved.len)] }));
        self.file = Some((saved.path, Stamp::of(&meta)));
        Ok(true)
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx { text: self.map.text(), index: &self.index }
    }

    fn collect(&self, range: Range<usize>) -> Vec<&[u8]> {
        let mut out = Vec::new();
        walk(&self.root, 0, &range, &mut |p, r| out.push(&p.bytes(self.map.text())[r]));
        out
    }

    fn grow(&mut self, sibling: Arc<Node>) {
        let left = std::mem::replace(&mut self.root, Arc::new(Node::leaf(vec![])));
        self.root = Arc::new(Node::inner(vec![left, sibling]));
    }
}

impl TextBuffer for MappedBuffer {
    fn len_bytes(&self) -> usize {
        self.root.bytes
    }

    fn len_lines(&self) -> Option<usize> {
        self.is_indexed().then(|| self.root.lines(&self.ctx()) + 1)
    }

    fn line(&self, idx: usize) -> Cow<'_, str> {
        let start = self.line_to_byte(idx);
        // The end is the next newline: scan forward in growing steps rather
        // than a second index lookup.
        let len = self.len_bytes();
        let mut step = 4096;
        let end = loop {
            let probe = start..(start + step).min(len);
            let mut off = probe.start;
            let found = self.collect(probe.clone()).into_iter().find_map(|c| {
                let hit = memchr::memchr(b'\n', c).map(|i| off + i + 1);
                off += c.len();
                hit
            });
            match found {
                Some(end) => break end,
                None if probe.end == len => break len,
                None => step *= 4,
            }
        };
        let mut line = self.slice(start..end);
        let trimmed = line.trim_end_matches(['\n', '\r']).len();
        match &mut line {
            Cow::Borrowed(b) => *b = &b[..trimmed],
            Cow::Owned(o) => o.truncate(trimmed),
        }
        line
    }

    fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        let chunks = self.collect(range);
        match chunks.as_slice() {
            [] => Cow::Borrowed(""),
            [one] => String::from_utf8_lossy(one),
            many => Cow::Owned(String::from_utf8_lossy(&many.concat()).into_owned()),
        }
    }

    fn byte_to_line(&self, byte: usize) -> usize {
        lines_before(&self.root, &self.ctx(), byte.min(self.root.bytes))
    }

    fn line_to_byte(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        find_line(&self.root, &self.ctx(), line).unwrap_or(self.root.bytes)
    }

    fn chunks(&self, range: Range<usize>) -> Box<dyn Iterator<Item = &[u8]> + '_> {
        Box::new(self.collect(range).into_iter())
    }

    fn apply(&mut self, edit: &Edit) -> AppliedEdit {
        let Range { start, end } = edit.range;
        assert!(start <= end && end <= self.len_bytes(), "edit {start}..{end} out of 0..{}", self.len_bytes());
        let removed = self.slice(start..end).into_owned();
        if start < end
            && let Some(sibling) = remove(&mut self.root, start, end)
        {
            self.grow(sibling);
        }
        if !edit.text.is_empty()
            && let Some(sibling) = insert(&mut self.root, start, &edit.text)
        {
            self.grow(sibling);
        }
        loop {
            match &self.root.kind {
                Kind::Inner(children) if children.len() == 1 => self.root = children[0].clone(),
                Kind::Inner(children) if children.is_empty() => self.root = Arc::new(Node::leaf(vec![])),
                _ => break,
            }
        }
        self.version += 1;
        AppliedEdit {
            edit: edit.clone(),
            inverse: Edit {
                range: start..start + edit.text.len(),
                text: removed,
                origin: EditOrigin::Undo,
                txn: edit.txn,
            },
            version: self.version,
        }
    }

    fn version(&self) -> u64 {
        self.version
    }

    fn snapshot(&self) -> Arc<dyn TextBuffer> {
        Arc::new(self.clone())
    }

    fn as_mapped(&self) -> Option<&MappedBuffer> {
        Some(self)
    }

    fn as_mapped_mut(&mut self) -> Option<&mut MappedBuffer> {
        Some(self)
    }
}

// ---------------------------------------------------------------------------
// The mapped file and its line index

struct Mapping {
    map: Option<Mmap>,
    /// Bytes skipped at the start (the BOM).
    base: usize,
    /// Pages can be dropped and read back (not so for an anonymous map,
    /// where dropping them zeroes them).
    file_backed: bool,
}

impl Mapping {
    fn new(map: Option<Mmap>) -> Self {
        let base = if map.as_deref().unwrap_or(&[]).starts_with(BOM) { BOM.len() } else { 0 };
        Self { map, base, file_backed: true }
    }

    /// The original text, without the BOM.
    fn text(&self) -> &[u8] {
        self.map.as_deref().map_or(&[], |m| &m[self.base..])
    }

    fn release(&self, range: Range<usize>) {
        #[cfg(unix)]
        if let Some(m) = &self.map
            && self.file_backed
            && !range.is_empty()
        {
            // SAFETY: a shared read-only file mapping has no private pages;
            // DONTNEED only drops page-table entries and the next access
            // reads the same bytes back from the page cache.
            let _ = unsafe {
                m.unchecked_advise_range(memmap2::UncheckedAdvice::DontNeed, self.base + range.start, range.len())
            };
        }
        #[cfg(not(unix))]
        let _ = range;
    }
}

const UTF8_UNKNOWN: u8 = 0;
const UTF8_VALID: u8 = 1;
const UTF8_INVALID: u8 = 2;

/// Line index over the original text. Checkpoint `k` is the offset where
/// line `(k + 1) * LINES_PER_CHECKPOINT` starts; checkpoints only grow, so
/// any prefix of them is usable while the scan runs.
#[derive(Default)]
struct LineIndex {
    checkpoints: Mutex<Vec<usize>>,
    scanned: AtomicUsize,
    newlines: AtomicUsize,
    utf8: AtomicU8,
    done: AtomicBool,
}

impl LineIndex {
    /// The last known line start at or before `x`, as (line, offset).
    fn checkpoint_before(&self, x: usize) -> (usize, usize) {
        let cps = self.checkpoints.lock().unwrap();
        match cps.partition_point(|&c| c <= x) {
            0 => (0, 0),
            i => (i * LINES_PER_CHECKPOINT, cps[i - 1]),
        }
    }

    /// Newlines in `text[..x]`.
    fn lines_before(&self, text: &[u8], x: usize) -> usize {
        let (line, from) = self.checkpoint_before(x);
        line + count_nl(&text[from..x])
    }

    /// Newlines in `text[a..b]`.
    fn count(&self, text: &[u8], a: usize, b: usize) -> usize {
        if b - a <= DIRECT_COUNT {
            count_nl(&text[a..b])
        } else {
            self.lines_before(text, b) - self.lines_before(text, a)
        }
    }

    /// Offset (relative to `a`) just after the `n`-th newline (`n >= 1`) of
    /// `text[a..b]`, or the number of newlines there if it has fewer.
    fn find(&self, text: &[u8], a: usize, b: usize, n: usize) -> Result<usize, usize> {
        let mut from = a;
        let mut need = n;
        if n > LINES_PER_CHECKPOINT {
            let target = self.lines_before(text, a) + n;
            let k = target / LINES_PER_CHECKPOINT;
            let cp = {
                let cps = self.checkpoints.lock().unwrap();
                (k >= 1 && k <= cps.len()).then(|| cps[k - 1])
            };
            if let Some(cp) = cp {
                // k * LPC > lines_before(a), so cp > a.
                if cp > b {
                    return Err(self.count(text, a, b));
                }
                from = cp;
                need = target - k * LINES_PER_CHECKPOINT;
                if need == 0 {
                    return Ok(cp - a);
                }
            }
        }
        let mut seen = 0;
        for p in memchr::memchr_iter(b'\n', &text[from..b]) {
            seen += 1;
            if seen == need {
                return Ok(from + p + 1 - a);
            }
        }
        if from == a { Err(seen) } else { Err(self.count(text, a, b)) }
    }
}

fn count_nl(bytes: &[u8]) -> usize {
    // Vectorises well; faster than memchr_iter for counting.
    bytes.iter().filter(|&&b| b == b'\n').count()
}

/// Streaming newline counter that records checkpoints.
#[derive(Debug, Default)]
struct LineCounter {
    offset: usize,
    lines: usize,
    checkpoints: Vec<usize>,
}

impl LineCounter {
    fn feed(&mut self, bytes: &[u8]) {
        for block in bytes.chunks(64 * 1024) {
            let n = count_nl(block);
            if self.lines % LINES_PER_CHECKPOINT + n >= LINES_PER_CHECKPOINT {
                for p in memchr::memchr_iter(b'\n', block) {
                    self.lines += 1;
                    if self.lines.is_multiple_of(LINES_PER_CHECKPOINT) {
                        self.checkpoints.push(self.offset + p + 1);
                    }
                }
            } else {
                self.lines += n;
            }
            self.offset += block.len();
        }
    }
}

fn spawn_indexer(map: Arc<Mapping>, index: Arc<LineIndex>) {
    let spawned = std::thread::Builder::new().name("mdedit-line-index".into()).spawn({
        let (map, index) = (map.clone(), index.clone());
        move || run_indexer(&map, &index)
    });
    if spawned.is_err() {
        run_indexer(&map, &index);
    }
}

fn run_indexer(map: &Mapping, index: &Arc<LineIndex>) {
    let text = map.text();
    let mut counter = LineCounter::default();
    let mut utf8_ok = true;
    let mut utf8_from = 0;
    let mut pos = 0;
    while pos < text.len() {
        // Every buffer and snapshot sharing this index is gone.
        if Arc::strong_count(index) == 1 {
            return;
        }
        let end = (pos + INDEX_CHUNK).min(text.len());
        if utf8_ok {
            match std::str::from_utf8(&text[utf8_from..end]) {
                Ok(_) => utf8_from = end,
                // A char split by the chunk end: validate it with the next chunk.
                Err(e) if e.error_len().is_none() && end < text.len() => utf8_from += e.valid_up_to(),
                Err(_) => utf8_ok = false,
            }
        }
        counter.feed(&text[pos..end]);
        if !counter.checkpoints.is_empty() {
            index.checkpoints.lock().unwrap().append(&mut counter.checkpoints);
        }
        index.scanned.store(end, Ordering::Relaxed);
        map.release(pos..end);
        pos = end;
    }
    index.newlines.store(counter.lines, Ordering::Relaxed);
    index.utf8.store(if utf8_ok { UTF8_VALID } else { UTF8_INVALID }, Ordering::Release);
    index.done.store(true, Ordering::Release);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    inode: u64,
}

impl Stamp {
    fn of(meta: &Metadata) -> Self {
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(meta);
        #[cfg(not(unix))]
        let inode = 0;
        Self { len: meta.len(), modified: meta.modified().ok(), inode }
    }
}

// ---------------------------------------------------------------------------
// Piece tree

#[derive(Clone)]
enum Src {
    /// The original text (`Mapping::text`).
    Map,
    /// Inserted text. Typing appends to the string while the piece is its
    /// only user, so a typed word is one piece and one allocation.
    Add(Arc<String>),
}

#[derive(Clone)]
struct Piece {
    src: Src,
    start: usize,
    len: usize,
    /// Newline count, computed on first use (survives the node copies an
    /// edit makes, so a query after an edit only counts the new pieces).
    nl: OnceLock<usize>,
}

impl Piece {
    fn map(start: usize, len: usize) -> Self {
        Self { src: Src::Map, start, len, nl: OnceLock::new() }
    }

    fn add(text: &str) -> Self {
        let mut s = String::with_capacity(text.len().max(64));
        s.push_str(text);
        Self { src: Src::Add(Arc::new(s)), start: 0, len: text.len(), nl: OnceLock::new() }
    }

    fn sub(&self, from: usize, to: usize) -> Self {
        Self { src: self.src.clone(), start: self.start + from, len: to - from, nl: OnceLock::new() }
    }

    fn bytes<'a>(&'a self, text: &'a [u8]) -> &'a [u8] {
        let range = self.start..self.start + self.len;
        match &self.src {
            Src::Map => &text[range],
            Src::Add(s) => &s.as_bytes()[range],
        }
    }

    /// Extend in place if this piece is the only user of its add string.
    fn try_append(&mut self, text: &str) -> bool {
        if let Src::Add(s) = &mut self.src
            && let Some(s) = Arc::get_mut(s)
        {
            s.truncate(self.start + self.len);
            s.push_str(text);
            self.len += text.len();
            self.nl = OnceLock::new();
            return true;
        }
        false
    }
}

struct Ctx<'a> {
    text: &'a [u8],
    index: &'a LineIndex,
}

impl Ctx<'_> {
    /// Newlines in the first `upto` bytes of `p`.
    fn newlines(&self, p: &Piece, upto: usize) -> usize {
        if upto == p.len {
            return *p.nl.get_or_init(|| self.count(p, upto));
        }
        self.count(p, upto)
    }

    fn count(&self, p: &Piece, upto: usize) -> usize {
        match &p.src {
            Src::Map => self.index.count(self.text, p.start, p.start + upto),
            Src::Add(_) => count_nl(&p.bytes(self.text)[..upto]),
        }
    }

    fn find(&self, p: &Piece, n: usize) -> Result<usize, usize> {
        if let Some(&nl) = p.nl.get()
            && nl < n
        {
            return Err(nl);
        }
        match &p.src {
            Src::Map => self.index.find(self.text, p.start, p.start + p.len, n),
            Src::Add(_) => {
                let mut seen = 0;
                for i in memchr::memchr_iter(b'\n', p.bytes(self.text)) {
                    seen += 1;
                    if seen == n {
                        return Ok(i + 1);
                    }
                }
                Err(seen)
            }
        }
    }
}

#[derive(Clone)]
struct Node {
    bytes: usize,
    /// Newline count, computed on first use. Edits build new nodes along
    /// their path, so a cached count never goes stale.
    lines: OnceLock<usize>,
    kind: Kind,
}

#[derive(Clone)]
enum Kind {
    Leaf(Vec<Piece>),
    Inner(Vec<Arc<Node>>),
}

impl Node {
    fn leaf(pieces: Vec<Piece>) -> Self {
        let mut n = Self { bytes: 0, lines: OnceLock::new(), kind: Kind::Leaf(pieces) };
        n.refresh();
        n
    }

    fn inner(children: Vec<Arc<Node>>) -> Self {
        let mut n = Self { bytes: 0, lines: OnceLock::new(), kind: Kind::Inner(children) };
        n.refresh();
        n
    }

    fn refresh(&mut self) {
        self.bytes = match &self.kind {
            Kind::Leaf(ps) => ps.iter().map(|p| p.len).sum(),
            Kind::Inner(cs) => cs.iter().map(|c| c.bytes).sum(),
        };
        self.lines = OnceLock::new();
    }

    fn split_if_full(&mut self) -> Option<Arc<Node>> {
        let right = match &mut self.kind {
            Kind::Leaf(ps) if ps.len() > MAX_LEAF => Node::leaf(ps.split_off(ps.len() / 2)),
            Kind::Inner(cs) if cs.len() > MAX_CHILDREN => Node::inner(cs.split_off(cs.len() / 2)),
            _ => return None,
        };
        self.refresh();
        Some(Arc::new(right))
    }

    fn lines(&self, ctx: &Ctx<'_>) -> usize {
        *self.lines.get_or_init(|| match &self.kind {
            Kind::Leaf(ps) => ps.iter().map(|p| ctx.newlines(p, p.len)).sum(),
            Kind::Inner(cs) => cs.iter().map(|c| c.lines(ctx)).sum(),
        })
    }

    fn pieces(&self) -> usize {
        match &self.kind {
            Kind::Leaf(ps) => ps.len(),
            Kind::Inner(cs) => cs.iter().map(|c| c.pieces()).sum(),
        }
    }
}

/// Remove `a..b` (node-relative, non-empty). Returns a new right sibling if
/// the node overflowed.
fn remove(node: &mut Arc<Node>, a: usize, b: usize) -> Option<Arc<Node>> {
    let n = Arc::make_mut(node);
    match &mut n.kind {
        Kind::Leaf(pieces) => {
            let mut out = Vec::with_capacity(pieces.len() + 1);
            let mut off = 0;
            for p in pieces.drain(..) {
                let (ps, pe) = (off, off + p.len);
                off = pe;
                if pe <= a || ps >= b {
                    out.push(p);
                    continue;
                }
                if ps < a {
                    out.push(p.sub(0, a - ps));
                }
                if pe > b {
                    out.push(p.sub(b - ps, p.len));
                }
            }
            *pieces = out;
        }
        Kind::Inner(children) => {
            let mut out = Vec::with_capacity(children.len() + 2);
            let mut off = 0;
            for mut c in children.drain(..) {
                let (cs, ce) = (off, off + c.bytes);
                off = ce;
                if ce <= a || cs >= b {
                    out.push(c);
                    continue;
                }
                if a <= cs && ce <= b {
                    continue;
                }
                let sibling = remove(&mut c, a.max(cs) - cs, b.min(ce) - cs);
                if c.bytes > 0 {
                    out.push(c);
                }
                out.extend(sibling);
            }
            *children = out;
        }
    }
    n.refresh();
    n.split_if_full()
}

/// Insert `text` (non-empty) at node-relative `at`. Returns a new right
/// sibling if the node overflowed.
fn insert(node: &mut Arc<Node>, at: usize, text: &str) -> Option<Arc<Node>> {
    let n = Arc::make_mut(node);
    match &mut n.kind {
        Kind::Leaf(pieces) => {
            // First piece whose end is at or after `at`, so typing at the end
            // of a piece extends it.
            let mut i = 0;
            let mut off = 0;
            while i < pieces.len() && at > off + pieces[i].len {
                off += pieces[i].len;
                i += 1;
            }
            if i < pieces.len() && at > off {
                let k = at - off;
                if k == pieces[i].len {
                    if !pieces[i].try_append(text) {
                        pieces.insert(i + 1, Piece::add(text));
                    }
                } else {
                    let right = pieces[i].sub(k, pieces[i].len);
                    pieces[i] = pieces[i].sub(0, k);
                    pieces.splice(i + 1..i + 1, [Piece::add(text), right]);
                }
            } else {
                pieces.insert(i, Piece::add(text));
            }
        }
        Kind::Inner(children) => {
            if children.is_empty() {
                children.push(Arc::new(Node::leaf(vec![])));
            }
            let mut i = 0;
            let mut off = 0;
            while i + 1 < children.len() && at > off + children[i].bytes {
                off += children[i].bytes;
                i += 1;
            }
            if let Some(sibling) = insert(&mut children[i], at - off, text) {
                children.insert(i + 1, sibling);
            }
        }
    }
    n.refresh();
    n.split_if_full()
}

/// Call `f` for each piece overlapping `range` (node offsets start at
/// `off`), with the overlapping part relative to the piece.
fn walk<'a>(node: &'a Node, off: usize, range: &Range<usize>, f: &mut impl FnMut(&'a Piece, Range<usize>)) {
    let mut o = off;
    match &node.kind {
        Kind::Leaf(pieces) => {
            for p in pieces {
                let (ps, pe) = (o, o + p.len);
                o = pe;
                if pe <= range.start {
                    continue;
                }
                if ps >= range.end {
                    break;
                }
                f(p, range.start.max(ps) - ps..range.end.min(pe) - ps);
            }
        }
        Kind::Inner(children) => {
            for c in children {
                let (cs, ce) = (o, o + c.bytes);
                o = ce;
                if ce <= range.start {
                    continue;
                }
                if cs >= range.end {
                    break;
                }
                walk(c, cs, range, f);
            }
        }
    }
}

/// Newlines in the node's first `k` bytes.
fn lines_before(node: &Node, ctx: &Ctx<'_>, k: usize) -> usize {
    if k >= node.bytes {
        return node.lines(ctx);
    }
    let mut lines = 0;
    let mut off = 0;
    match &node.kind {
        Kind::Leaf(pieces) => {
            for p in pieces {
                if off + p.len <= k {
                    lines += ctx.newlines(p, p.len);
                    off += p.len;
                } else {
                    return lines + ctx.newlines(p, k - off);
                }
            }
        }
        Kind::Inner(children) => {
            for c in children {
                if off + c.bytes <= k {
                    lines += c.lines(ctx);
                    off += c.bytes;
                } else {
                    return lines + lines_before(c, ctx, k - off);
                }
            }
        }
    }
    lines
}

/// Node offset just after the `n`-th newline (`n >= 1`), or the node's
/// newline count if it has fewer. Skips subtrees with a cached count, and
/// otherwise scans only up to the target, so it stays cheap near the start
/// of a file that is still being indexed.
fn find_line(node: &Node, ctx: &Ctx<'_>, n: usize) -> Result<usize, usize> {
    let mut rem = n;
    let mut off = 0;
    match &node.kind {
        Kind::Leaf(pieces) => {
            for p in pieces {
                match ctx.find(p, rem) {
                    Ok(pos) => return Ok(off + pos),
                    Err(seen) => rem -= seen,
                }
                off += p.len;
            }
        }
        Kind::Inner(children) => {
            for c in children {
                if let Some(&lines) = c.lines.get()
                    && lines < rem
                {
                    rem -= lines;
                } else {
                    match find_line(c, ctx, rem) {
                        Ok(pos) => return Ok(off + pos),
                        Err(seen) => rem -= seen,
                    }
                }
                off += c.bytes;
            }
        }
    }
    let _ = node.lines.set(n - rem);
    Err(n - rem)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::RopeBuffer;
    use crate::edit::TxnId;

    fn edit(range: Range<usize>, text: &str) -> Edit {
        Edit { range, text: text.into(), origin: EditOrigin::User, txn: TxnId(0) }
    }

    fn indexed(text: &str) -> MappedBuffer {
        let b = MappedBuffer::from_text(text);
        b.wait_indexed();
        b
    }

    fn assert_same_lines(m: &MappedBuffer, r: &RopeBuffer) {
        let text = r.text();
        assert_eq!(m.len_lines(), r.len_lines(), "text {text:?}");
        let lines = r.len_lines().unwrap();
        for i in 0..lines {
            assert_eq!(m.line(i), r.line(i), "line {i} of {text:?}");
            assert_eq!(m.line_to_byte(i), r.line_to_byte(i), "line_to_byte {i} of {text:?}");
        }
        for (b, _) in text.char_indices().chain([(text.len(), ' ')]) {
            assert_eq!(m.byte_to_line(b), r.byte_to_line(b), "byte_to_line {b} of {text:?}");
        }
    }

    #[test]
    fn edits_lines_and_inverse() {
        let mut m = indexed("one\ntwo\nthree\n");
        let a = m.apply(&edit(4..7, "2\n2b"));
        assert_eq!(m.text(), "one\n2\n2b\nthree\n");
        assert_eq!(m.len_lines(), Some(5));
        assert_eq!(m.line(2), "2b");
        assert_eq!(m.line_to_byte(3), 9);
        assert_eq!(m.byte_to_line(9), 3);
        m.apply(&a.inverse);
        assert_eq!(m.text(), "one\ntwo\nthree\n");
        assert_eq!(m.version(), 2);
    }

    #[test]
    fn typing_extends_one_piece() {
        let mut m = indexed("abc");
        for (i, c) in "hello".char_indices() {
            m.apply(&edit(3 + i..3 + i, &c.to_string()));
        }
        assert_eq!(m.text(), "abchello");
        assert_eq!(m.piece_count(), 2);
    }

    #[test]
    fn snapshots_are_isolated() {
        let mut m = indexed("hello world");
        let snap = m.snapshot();
        m.apply(&edit(5..5, ","));
        m.apply(&edit(6..6, "!"));
        assert_eq!(snap.text(), "hello world");
        assert_eq!(m.text(), "hello,! world");
        // Typing after a snapshot must not write into a string it shares.
        let snap2 = m.snapshot();
        m.apply(&edit(7..7, "?"));
        assert_eq!(snap2.text(), "hello,! world");
        assert_eq!(m.text(), "hello,!? world");
    }

    #[test]
    fn many_edits_build_a_deep_tree() {
        let base: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let mut m = indexed(&base);
        let mut r = RopeBuffer::from_text(&base);
        for i in 0..3000usize {
            let at = (i * 7919) % m.len_bytes();
            let e = if i % 3 == 0 { edit(at..(at + 5).min(m.len_bytes()), "") } else { edit(at..at, "x\ny") };
            m.apply(&e);
            r.apply(&e);
        }
        assert!(m.piece_count() > MAX_LEAF * MAX_CHILDREN / 4, "{} pieces", m.piece_count());
        assert!(matches!(&m.root.kind, Kind::Inner(cs) if matches!(cs[0].kind, Kind::Inner(_))));
        assert_eq!(m.text(), r.text());
        assert_same_lines(&m, &r);
    }

    #[test]
    fn line_queries_before_and_after_indexing_agree() {
        let text: String = (0..20_000).map(|i| format!("{i}\n")).collect();
        let m = MappedBuffer::from_text(&text);
        // Possibly still indexing: answers must already be right.
        assert_eq!(m.line_to_byte(15_000), text.find("\n15000\n").unwrap() + 1);
        assert_eq!(m.byte_to_line(text.len()), 20_000);
        m.wait_indexed();
        assert_eq!(m.len_lines(), Some(20_001));
        assert_eq!(m.line(12_345), "12345");
        assert_eq!(m.line_to_byte(15_000), text.find("\n15000\n").unwrap() + 1);
        assert_eq!(m.line_to_byte(99_999), text.len());
        assert_eq!(m.byte_to_line(text.find("\n4321\n").unwrap() + 3), 4321);
    }

    #[test]
    fn detects_invalid_utf8() {
        assert_eq!(indexed("ok ✓").utf8_valid(), Some(true));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin1.md");
        fs::write(&path, b"caf\xE9\n").unwrap();
        let (m, _) = MappedBuffer::open(&path).unwrap();
        m.wait_indexed();
        assert_eq!(m.utf8_valid(), Some(false));
        assert_eq!(m.line(0), "caf\u{FFFD}");
    }

    #[test]
    fn open_save_remap_and_disk_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.md");
        let body: String = (0..3000).map(|i| format!("row {i}\r\n")).collect();
        fs::write(&path, [BOM, body.as_bytes()].concat()).unwrap();
        let (mut m, format) = MappedBuffer::open(&path).unwrap();
        assert!(format.bom);
        assert_eq!(format.line_ending, LineEnding::CrLf);
        assert_eq!(m.text(), body);
        assert_eq!(m.disk_change(), DiskChange::Unchanged);

        m.apply(&edit(0..0, "# Title\r\n"));
        let saved = m.save_to(&path, format).unwrap();
        assert!(m.remap(saved).unwrap());
        assert_eq!(m.piece_count(), 1);
        assert!(m.is_indexed());
        assert_eq!(m.len_lines(), Some(3002));
        assert_eq!(m.line(2000), "row 1999");
        assert_eq!(fs::read(&path).unwrap(), [BOM, b"# Title\r\n", body.as_bytes()].concat());
        assert_eq!(m.disk_change(), DiskChange::Unchanged);

        // A save that raced with an edit does not remap.
        let saved = m.save_to(&path, format).unwrap();
        m.apply(&edit(0..1, ""));
        assert!(!m.remap(saved).unwrap());

        fs::write(&path, "changed elsewhere").unwrap();
        assert_eq!(m.disk_change(), DiskChange::Modified);
        fs::remove_file(&path).unwrap();
        assert_eq!(m.disk_change(), DiskChange::Removed);
    }

    #[test]
    fn empty_and_bom_only_files() {
        let dir = tempfile::tempdir().unwrap();
        for bytes in [&b""[..], BOM] {
            let path = dir.path().join("e.md");
            fs::write(&path, bytes).unwrap();
            let (mut m, _) = MappedBuffer::open(&path).unwrap();
            m.wait_indexed();
            assert_eq!(m.len_bytes(), 0);
            assert_eq!(m.len_lines(), Some(1));
            assert_eq!(m.line(0), "");
            m.apply(&edit(0..0, "a\n"));
            assert_eq!(m.len_lines(), Some(2));
        }
    }

    #[derive(Debug, Clone)]
    enum Op {
        Edit { at: usize, del: usize, text: String },
        Undo,
        Snapshot,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            8 => (any::<usize>(), 0usize..12, "[ab\n\r\u{e9}\u{1F980}]{0,6}")
                .prop_map(|(at, del, text)| Op::Edit { at, del, text }),
            1 => Just(Op::Undo),
            1 => Just(Op::Snapshot),
        ]
    }

    fn floor_boundary(s: &str, mut i: usize) -> usize {
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Same edits as RopeBuffer give the same text (ARCHITECTURE 11).
        #[test]
        fn same_text_as_rope(initial in "[ab\n\u{e9}]{0,40}", ops in prop::collection::vec(op(), 1..120)) {
            let mut m = MappedBuffer::from_text(&initial);
            let mut r = RopeBuffer::from_text(&initial);
            let mut undo: Vec<Edit> = Vec::new();
            let mut snaps: Vec<(Arc<dyn TextBuffer>, String)> = Vec::new();
            for op in ops {
                match op {
                    Op::Edit { at, del, text } => {
                        let s = r.text().into_owned();
                        let start = floor_boundary(&s, at % (s.len() + 1));
                        let end = floor_boundary(&s, (start + del).min(s.len()));
                        let e = edit(start..end, &text);
                        let am = m.apply(&e);
                        let ar = r.apply(&e);
                        prop_assert_eq!(&am.inverse, &ar.inverse);
                        undo.push(am.inverse);
                    }
                    Op::Undo => {
                        if let Some(inv) = undo.pop() {
                            m.apply(&inv);
                            r.apply(&inv);
                        }
                    }
                    Op::Snapshot => snaps.push((m.snapshot(), r.text().into_owned())),
                }
                prop_assert_eq!(m.text(), r.text());
                prop_assert_eq!(m.len_bytes(), r.len_bytes());
            }
            for (snap, text) in &snaps {
                prop_assert_eq!(snap.text(), text.as_str());
            }
            // Lines agree unless the text has a lone CR (a break only in the rope).
            let text = r.text().into_owned();
            if !text.replace("\r\n", "").contains('\r') {
                m.wait_indexed();
                assert_same_lines(&m, &r);
            }
        }
    }
}
