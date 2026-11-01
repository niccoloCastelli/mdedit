//! Block index and markdown parsing (parsing workstream).
//!
//! ARCHITECTURE 3: the document is a list of top-level blocks with byte
//! ranges. Blocks live in a two-level tree (leaves of about 64 blocks with
//! leaf-relative offsets, Fenwick trees over leaf byte spans and block
//! counts), so an edit shifts every later block in O(log n) and lookups by
//! byte or by block number are O(log n).
//!
//! An edit reparses a window with pulldown-cmark: from the block before the
//! first touched block to the block after the last one (ARCHITECTURE 3.2).
//! The result is spliced in when its last block reproduces an old block
//! boundary; otherwise (an unclosed fence or HTML block swallowed text) the
//! window grows, doubling, until it does or reaches the end of the document.
//! Windows over [`DEFAULT_OFFLOAD_THRESHOLD`] become a [`ParseJob`] for a
//! worker; until its result arrives the old blocks are kept, shifted.
//!
//! Link reference definitions are document-global and do not produce
//! blocks; they are kept in a side table ([`BlockIndex::link_ref`]) and each
//! edit reports the labels whose definition changed ([`Change::labels`]).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{Event, Options, Parser, Tag};

use crate::buffer::TextBuffer;
use crate::edit::AppliedEdit;

/// The pulldown-cmark options mdedit uses everywhere (SPEC 4). Export and
/// render must use the same set so all views agree.
pub fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES
}

/// Windows larger than this are parsed by a [`ParseJob`] on a worker
/// (ARCHITECTURE 3.2).
pub const DEFAULT_OFFLOAD_THRESHOLD: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading {
        level: u8,
    },
    BlockQuote,
    List {
        ordered: bool,
    },
    CodeBlock {
        fenced: bool,
        info: String,
    },
    HtmlBlock,
    Table,
    ThematicBreak,
    FootnoteDefinition,
    /// Fenced `math` / `$$` block (v1.x); parsed as code until then.
    Math,
    /// Fenced `mermaid` block (v1.x); parsed as code until then.
    Mermaid,
}

/// One top-level block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Source bytes, from the first byte of the block to the end of its last
    /// line (excluding the blank lines that separate blocks).
    pub bytes: Range<usize>,
    /// Bumped whenever the block is reparsed; render caches key on it. A
    /// block that a reparse reproduces unchanged keeps its generation.
    pub generation: u64,
}

/// A link reference definition (`[label]: dest "title"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRef {
    pub dest: String,
    pub title: Option<String>,
}

/// What the last edit (or finished parse job) changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// Indices, in the current index, of the blocks that were reparsed.
    pub blocks: Range<usize>,
    /// Normalised labels whose link reference definition appeared,
    /// disappeared or changed; blocks using them need re-rendering.
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockData {
    kind: BlockKind,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefDef {
    /// Normalised with [`normalize_label`].
    label: String,
    link: LinkRef,
}

#[derive(Debug, Clone)]
pub struct BlockIndex {
    blocks: Tree<BlockData>,
    refs: Tree<RefDef>,
    /// First definition (in document order) of each label.
    ref_table: HashMap<String, LinkRef>,
    generation: u64,
    last_change: Change,
    offload_threshold: Option<usize>,
    /// Region (current coordinates) whose blocks are stale until a parse
    /// job or [`Self::flush`] reparses it.
    pending: Option<Range<usize>>,
    /// Sequence number of the job handed out for `pending`, if any.
    job_out: Option<u64>,
    job_seq: u64,
}

impl Default for BlockIndex {
    fn default() -> Self {
        Self {
            blocks: Tree::default(),
            refs: Tree::default(),
            ref_table: HashMap::new(),
            generation: 0,
            last_change: Change::default(),
            offload_threshold: Some(DEFAULT_OFFLOAD_THRESHOLD),
            pending: None,
            job_out: None,
            job_seq: 0,
        }
    }
}

impl BlockIndex {
    /// Parse the whole document (on the calling thread).
    pub fn parse(buf: &dyn TextBuffer) -> Self {
        let text = buf.text();
        let (blocks, refs) = parse_window(&text, 0, 1);
        let mut index = Self {
            generation: 1,
            last_change: Change { blocks: 0..blocks.len(), labels: Vec::new() },
            blocks: Tree::from_entries(blocks),
            refs: Tree::from_entries(refs),
            ..Self::default()
        };
        index.rebuild_ref_table();
        index
    }

    /// An empty index whose whole document is pending, for opening a file
    /// without parsing it on the UI thread: the first
    /// [`Self::take_parse_job`] parses everything (a full parse of 10 MB
    /// takes a few hundred milliseconds).
    pub fn deferred(buf: &dyn TextBuffer) -> Self {
        Self { pending: Some(0..buf.len_bytes()), ..Self::default() }
    }

    /// Windows larger than `threshold` bytes become parse jobs; `None`
    /// parses everything on the calling thread.
    pub fn set_offload_threshold(&mut self, threshold: Option<usize>) {
        self.offload_threshold = threshold;
    }

    /// Keep the index in step with an applied edit. `buf` is the text after
    /// the edit.
    pub fn on_edit(&mut self, applied: &AppliedEdit, buf: &dyn TextBuffer) {
        let range = applied.edit.range.clone();
        let new_range = range.start..range.start + applied.edit.text.len();
        if let Some(pending) = self.pending.clone() {
            self.shift_only(applied);
            let mapped = applied.map_offset(pending.start, false)..applied.map_offset(pending.end, true);
            self.pending = Some(mapped.start.min(new_range.start)..mapped.end.max(new_range.end));
            return;
        }
        let delta = applied.edit.delta();
        let plan = self.plan(range.clone(), delta, buf);
        match run_plan(&plan, buf, self.offload_threshold) {
            Some(result) => self.apply_result(result, delta, range),
            None => {
                self.shift_only(applied);
                self.pending = Some(new_range);
            }
        }
    }

    /// True while a large reparse is outstanding and some blocks are stale.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// The parse job to run on a worker, if a large reparse is outstanding
    /// and no job for it is out. Run it with [`ParseJob::run`] and hand the
    /// result to [`Self::finish_parse`].
    pub fn take_parse_job(&mut self, buf: &dyn TextBuffer) -> Option<ParseJob> {
        let pending = self.pending.clone()?;
        if self.job_out.is_some() {
            return None;
        }
        self.job_seq += 1;
        self.job_out = Some(self.job_seq);
        let plan = self.plan(pending.clone(), 0, buf);
        Some(ParseJob { snapshot: buf.snapshot(), plan, pending, version: buf.version(), seq: self.job_seq })
    }

    /// Splice in a finished job. A result for an older text version is
    /// dropped (the region stays pending and the next
    /// [`Self::take_parse_job`] covers the newer edits too); returns whether
    /// it was applied.
    pub fn finish_parse(&mut self, result: ParseResult, buf: &dyn TextBuffer) -> bool {
        if self.job_out != Some(result.seq) {
            return false;
        }
        self.job_out = None;
        if result.version != buf.version() {
            return false;
        }
        self.pending = None;
        self.apply_result(result.window, 0, result.pending);
        true
    }

    /// Finish any outstanding reparse on the calling thread.
    pub fn flush(&mut self, buf: &dyn TextBuffer) {
        if let Some(pending) = self.pending.take() {
            self.job_out = None;
            let plan = self.plan(pending.clone(), 0, buf);
            if let Some(result) = run_plan(&plan, buf, None) {
                self.apply_result(result, 0, pending);
            }
        }
    }

    /// All blocks, in document order.
    pub fn blocks(&self) -> Blocks<'_> {
        Blocks { index: Some(self) }
    }

    /// Block `i` in document order.
    pub fn get(&self, i: usize) -> Option<Block> {
        self.blocks.get(i).map(to_block)
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.len() == 0
    }

    /// Index of the block containing `byte`, or the block just before it
    /// when `byte` is in the gap between blocks.
    pub fn block_at(&self, byte: usize) -> Option<usize> {
        self.blocks.count_starting_before(byte, true).checked_sub(1)
    }

    /// Blocks overlapping `range`, for rendering a viewport.
    pub fn blocks_in(&self, range: Range<usize>) -> impl Iterator<Item = Block> + '_ {
        let first = match self.block_at(range.start) {
            Some(i) if self.blocks.get(i).is_some_and(|(r, _)| r.end > range.start) => i,
            Some(i) => i + 1,
            None => 0,
        };
        self.blocks.iter_from(first).take_while(move |(r, _)| r.start < range.end).map(to_block)
    }

    /// Bumped on every change to the index.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What the last edit or finished parse changed.
    pub fn last_change(&self) -> &Change {
        &self.last_change
    }

    /// The link reference definition for `label` (matched like CommonMark:
    /// case-insensitive, whitespace collapsed); the first one in the
    /// document wins.
    pub fn link_ref(&self, label: &str) -> Option<&LinkRef> {
        self.ref_table.get(&normalize_label(label))
    }

    /// Every defined label (normalised) with its definition.
    pub fn link_refs(&self) -> impl Iterator<Item = (&str, &LinkRef)> {
        self.ref_table.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Decide the reparse window for an edit of old range `range` that
    /// changed the length by `delta`. The tree is in old coordinates.
    fn plan(&self, range: Range<usize>, delta: isize, buf: &dyn TextBuffer) -> Plan {
        let n = self.blocks.len();
        let mut first = self.block_at(range.start).map_or(0, |k| k.saturating_sub(1));
        let mut start = match self.blocks.get(first) {
            Some((r, _)) if r.start <= range.start => buf.line_to_byte(buf.byte_to_line(r.start)),
            _ => 0,
        };
        // Parsing from a block start is a fresh start, except after a link
        // reference definition on the line just before: the window line is
        // then a paragraph continuation (the definition-only part of a
        // paragraph produces no block). Take such definitions in, and the
        // block whose range they fall in.
        while start > 0 {
            let prev_line = buf.byte_to_line(start - 1);
            let prev_start = buf.line_to_byte(prev_line);
            if buf.line(prev_line).trim().is_empty() {
                break;
            }
            let covering = self
                .refs
                .count_starting_before(prev_start, true)
                .checked_sub(1)
                .and_then(|i| self.refs.get(i))
                .filter(|(r, _)| r.end > prev_start);
            let Some((def, _)) = covering else { break };
            let mut new_start = def.start;
            if first > 0
                && let Some((r, _)) = self.blocks.get(first - 1)
                && r.end > new_start
            {
                first -= 1;
                new_start = new_start.min(r.start);
            }
            start = buf.line_to_byte(buf.byte_to_line(new_start));
        }
        // Old blocks after the edit whose boundary may end the window: the
        // block after the last touched one, then doubling distances.
        let mut candidates = Vec::new();
        let mut j = self.block_at(range.end).map_or(0, |m| m + 1);
        let mut step = 1;
        while j < n {
            let (r, data) = self.blocks.get(j).expect("index in range");
            candidates.push(Candidate {
                old_index: j,
                bytes: shift(r.start, delta)..shift(r.end, delta),
                kind: data.kind.clone(),
            });
            j += step;
            step *= 2;
        }
        Plan { first, start, candidates, old_count: n, new_len: buf.len_bytes(), generation: self.generation + 1 }
    }

    /// Splice a window result in. `delta` is the length change still to be
    /// applied after the window (0 when it was already shifted); `edited`
    /// is the changed range in old coordinates, to keep the generation of
    /// blocks the reparse reproduced unchanged.
    fn apply_result(&mut self, result: WindowResult, delta: isize, edited: Range<usize>) {
        self.generation += 1;
        let generation = self.generation;
        let WindowResult { start, end, old_blocks, mut blocks, refs } = result;
        // Old blocks in the window that the edit did not touch keep their
        // generation when reproduced exactly.
        let mut kept: HashMap<(usize, usize), (BlockKind, u64)> = HashMap::new();
        for (r, data) in self.blocks.iter_from(old_blocks.start).take(old_blocks.len()) {
            let mapped = if r.end <= edited.start {
                r
            } else if r.start >= edited.end {
                shift(r.start, delta)..shift(r.end, delta)
            } else {
                continue;
            };
            kept.insert((mapped.start, mapped.end), (data.kind.clone(), data.generation));
        }
        for (r, data) in &mut blocks {
            data.generation = match kept.get(&(r.start, r.end)) {
                Some((kind, g)) if *kind == data.kind => *g,
                _ => generation,
            };
        }
        let new_count = blocks.len();
        self.blocks.splice(old_blocks.clone(), blocks, delta);

        // Link reference definitions starting in the window. `end` is in
        // new coordinates and the definitions tree still in old ones.
        let old_end = if end == usize::MAX { end } else { shift(end, -delta) };
        let ref_old = self.refs.count_starting_before(start, false)..self.refs.count_starting_before(old_end, false);
        let removed: Vec<&RefDef> = self.refs.iter_from(ref_old.start).take(ref_old.len()).map(|(_, d)| d).collect();
        let changed = removed.len() != refs.len() || removed.iter().zip(&refs).any(|(a, (_, b))| *a != b);
        self.refs.splice(ref_old, refs, delta);
        let mut labels = Vec::new();
        if changed {
            let old_table = std::mem::take(&mut self.ref_table);
            self.rebuild_ref_table();
            labels.extend(old_table.iter().filter(|(k, v)| self.ref_table.get(*k) != Some(v)).map(|(k, _)| k.clone()));
            labels.extend(self.ref_table.keys().filter(|k| !old_table.contains_key(*k)).cloned());
            labels.sort();
        }
        self.last_change = Change { blocks: old_blocks.start..old_blocks.start + new_count, labels };
    }

    /// Move blocks and definitions through an edit without reparsing (the
    /// touched ones are stale until the pending reparse lands).
    fn shift_only(&mut self, applied: &AppliedEdit) {
        let delta = applied.edit.delta();
        let (range, mapped) = touched(&self.blocks, applied);
        self.blocks.splice(range, mapped, delta);
        let (range, mapped) = touched(&self.refs, applied);
        let refs_changed = !range.is_empty();
        self.refs.splice(range, mapped, delta);
        if refs_changed {
            self.rebuild_ref_table();
        }
        self.generation += 1;
        self.last_change = Change::default();
    }

    fn rebuild_ref_table(&mut self) {
        self.ref_table.clear();
        for (_, def) in self.refs.iter_from(0) {
            self.ref_table.entry(def.label.clone()).or_insert_with(|| def.link.clone());
        }
    }
}

/// The entries of `tree` an edit touches, mapped through it.
fn touched<T: Clone>(tree: &Tree<T>, applied: &AppliedEdit) -> (Range<usize>, Vec<(Range<usize>, T)>) {
    let r = &applied.edit.range;
    let first = tree.count_starting_before(r.start, true).saturating_sub(1);
    let end = tree.count_starting_before(r.end, true).max(first);
    let mapped = tree
        .iter_from(first)
        .take(end - first)
        .scan(0, |prev_end, (b, v)| {
            // Keep the mapped ranges ordered and disjoint.
            let start = applied.map_offset(b.start, false).max(*prev_end);
            let end = applied.map_offset(b.end, true);
            *prev_end = (*prev_end).max(end);
            Some((start..end, v.clone()))
        })
        .filter(|(b, _)| b.start < b.end)
        .collect();
    (first..end, mapped)
}

/// Parse windows of `plan` until one ends on an old block boundary.
/// Returns `None` (nothing parsed) when the next window to try is larger
/// than `threshold`.
fn run_plan(plan: &Plan, buf: &dyn TextBuffer, threshold: Option<usize>) -> Option<WindowResult> {
    let too_big = |end: usize| threshold.is_some_and(|t| end - plan.start > t);
    for c in &plan.candidates {
        if too_big(c.bytes.end) {
            return None;
        }
        let text = buf.slice(plan.start..c.bytes.end);
        let (blocks, refs) = parse_window(&text, plan.start, plan.generation);
        if blocks.last().is_some_and(|(r, data)| *r == c.bytes && data.kind == c.kind) {
            return Some(WindowResult {
                start: plan.start,
                end: c.bytes.end,
                old_blocks: plan.first..c.old_index + 1,
                blocks,
                refs,
            });
        }
    }
    if too_big(plan.new_len) {
        return None;
    }
    let text = buf.slice(plan.start..plan.new_len);
    let (blocks, refs) = parse_window(&text, plan.start, plan.generation);
    Some(WindowResult { start: plan.start, end: usize::MAX, old_blocks: plan.first..plan.old_count, blocks, refs })
}

/// A view of all blocks; cheap to copy. Blocks are returned by value with
/// absolute byte ranges.
#[derive(Debug, Clone, Copy, Default)]
pub struct Blocks<'a> {
    index: Option<&'a BlockIndex>,
}

impl<'a> Blocks<'a> {
    pub fn len(&self) -> usize {
        self.index.map_or(0, BlockIndex::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<Block> {
        self.index?.get(i)
    }

    pub fn first(&self) -> Option<Block> {
        self.get(0)
    }

    pub fn last(&self) -> Option<Block> {
        self.len().checked_sub(1).and_then(|i| self.get(i))
    }

    pub fn iter(&self) -> BlockIter<'a> {
        BlockIter { inner: self.index.map(|index| index.blocks.iter_from(0)) }
    }
}

impl<'a> IntoIterator for Blocks<'a> {
    type Item = Block;
    type IntoIter = BlockIter<'a>;

    fn into_iter(self) -> BlockIter<'a> {
        self.iter()
    }
}

/// Iterator over [`Blocks`].
pub struct BlockIter<'a> {
    inner: Option<TreeIter<'a, BlockData>>,
}

impl Iterator for BlockIter<'_> {
    type Item = Block;

    fn next(&mut self) -> Option<Block> {
        self.inner.as_mut()?.next().map(to_block)
    }
}

fn to_block((bytes, data): (Range<usize>, &BlockData)) -> Block {
    Block { kind: data.kind.clone(), bytes, generation: data.generation }
}

/// A reparse to run off the UI thread (`tokio::task::spawn_blocking`).
pub struct ParseJob {
    snapshot: Arc<dyn TextBuffer>,
    plan: Plan,
    pending: Range<usize>,
    version: u64,
    seq: u64,
}

impl ParseJob {
    pub fn run(self) -> ParseResult {
        let window = run_plan(&self.plan, self.snapshot.as_ref(), None).expect("no threshold, so always a result");
        ParseResult { window, pending: self.pending, version: self.version, seq: self.seq }
    }
}

impl std::fmt::Debug for ParseJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseJob").field("version", &self.version).field("start", &self.plan.start).finish()
    }
}

/// The output of a [`ParseJob`].
#[derive(Debug)]
pub struct ParseResult {
    window: WindowResult,
    pending: Range<usize>,
    version: u64,
    seq: u64,
}

#[derive(Debug, Clone)]
struct Candidate {
    old_index: usize,
    /// Its range after the edit.
    bytes: Range<usize>,
    kind: BlockKind,
}

#[derive(Debug, Clone)]
struct Plan {
    /// First old block replaced.
    first: usize,
    /// Window start (a line start), the same before and after the edit.
    start: usize,
    candidates: Vec<Candidate>,
    old_count: usize,
    new_len: usize,
    generation: u64,
}

#[derive(Debug)]
struct WindowResult {
    start: usize,
    /// End of the window after the edit; `usize::MAX` for the end of the
    /// document.
    end: usize,
    /// Old block indices replaced.
    old_blocks: Range<usize>,
    blocks: Vec<(Range<usize>, BlockData)>,
    refs: Vec<(Range<usize>, RefDef)>,
}

fn shift(offset: usize, delta: isize) -> usize {
    offset.wrapping_add_signed(delta)
}

type Parsed = (Vec<(Range<usize>, BlockData)>, Vec<(Range<usize>, RefDef)>);

/// Top-level blocks and reference definitions of `src`, with ranges shifted
/// by `base`.
fn parse_window(src: &str, base: usize, generation: u64) -> Parsed {
    let mut blocks = Vec::new();
    let mut depth = 0usize;
    let parser = Parser::new_ext(src, markdown_options());
    let mut refs: Vec<(Range<usize>, RefDef)> = parser
        .reference_definitions()
        .iter()
        .map(|(label, def)| {
            let link = LinkRef { dest: def.dest.to_string(), title: def.title.as_ref().map(|t| t.to_string()) };
            (base + def.span.start..base + def.span.end, RefDef { label: normalize_label(label), link })
        })
        .collect();
    refs.sort_by_key(|(r, _)| r.start);
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0
                    && let Some(kind) = block_kind(&tag)
                {
                    let end = trim_trailing_newlines(src, range.clone());
                    blocks.push((base + range.start..base + end, BlockData { kind, generation }));
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            Event::Rule if depth == 0 => {
                let end = trim_trailing_newlines(src, range.clone());
                blocks.push((base + range.start..base + end, BlockData { kind: BlockKind::ThematicBreak, generation }));
            }
            _ => {}
        }
    }
    (blocks, refs)
}

fn trim_trailing_newlines(src: &str, range: Range<usize>) -> usize {
    range.start + src[range].trim_end_matches(['\n', '\r']).len()
}

fn block_kind(tag: &Tag<'_>) -> Option<BlockKind> {
    use pulldown_cmark::CodeBlockKind;
    Some(match tag {
        Tag::Paragraph => BlockKind::Paragraph,
        Tag::Heading { level, .. } => BlockKind::Heading { level: *level as u8 },
        Tag::BlockQuote(_) => BlockKind::BlockQuote,
        Tag::List(start) => BlockKind::List { ordered: start.is_some() },
        Tag::CodeBlock(CodeBlockKind::Indented) => BlockKind::CodeBlock { fenced: false, info: String::new() },
        Tag::CodeBlock(CodeBlockKind::Fenced(info)) => match info.split_whitespace().next() {
            Some("mermaid") => BlockKind::Mermaid,
            Some("math") => BlockKind::Math,
            _ => BlockKind::CodeBlock { fenced: true, info: info.to_string() },
        },
        Tag::HtmlBlock => BlockKind::HtmlBlock,
        Tag::Table(_) => BlockKind::Table,
        Tag::FootnoteDefinition(_) => BlockKind::FootnoteDefinition,
        _ => return None,
    })
}

/// CommonMark label matching: case-folded, inner whitespace collapsed.
fn normalize_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for word in label.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().flat_map(char::to_lowercase));
    }
    out
}

/// One entry of the document outline (SPEC 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    pub level: u8,
    pub title: String,
    pub byte: usize,
}

/// Headings in document order.
pub fn outline(index: &BlockIndex, buf: &dyn TextBuffer) -> Vec<OutlineEntry> {
    index
        .blocks()
        .iter()
        .filter_map(|b| match b.kind {
            BlockKind::Heading { level } => {
                let src = buf.slice(b.bytes.clone());
                let lines: Vec<&str> = src.lines().collect();
                let title = if lines.len() > 1 {
                    // Setext: every line but the underline.
                    lines[..lines.len() - 1].iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ")
                } else {
                    atx_title(lines.first().copied().unwrap_or(""))
                };
                Some(OutlineEntry { level, title, byte: b.bytes.start })
            }
            _ => None,
        })
        .collect()
}

/// Headings of a document without a block index (large-file mode, SPEC 8),
/// found by a line scan that skips fenced code. Meant to run on a worker
/// over a snapshot. Only ATX headings are found.
pub fn scan_outline(buf: &dyn TextBuffer) -> Vec<OutlineEntry> {
    let Some(lines) = buf.len_lines() else { return Vec::new() };
    let mut fence: Option<(u8, usize)> = None;
    let mut out = Vec::new();
    for l in 0..lines {
        let line = buf.line(l);
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        let first = trimmed.as_bytes().first().copied().unwrap_or(0);
        if first == b'`' || first == b'~' {
            let run = trimmed.bytes().take_while(|c| *c == first).count();
            match fence {
                Some((ch, len)) if ch == first && run >= len && trimmed[run..].trim().is_empty() => fence = None,
                None if run >= 3 && !(first == b'`' && trimmed[run..].contains('`')) => fence = Some((first, run)),
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let hashes = trimmed.bytes().take_while(|c| *c == b'#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].chars().next().is_none_or(|c| c == ' ' || c == '\t') {
            out.push(OutlineEntry {
                level: hashes as u8,
                title: atx_title(trimmed),
                byte: buf.line_to_byte(l) + line.len() - trimmed.len(),
            });
        }
    }
    out
}

/// The title of an ATX heading line: without the `#` runs and attributes.
fn atx_title(line: &str) -> String {
    let mut t = line.trim().trim_start_matches('#').trim();
    if t.ends_with('}')
        && let Some(open) = t.rfind('{')
        && (open == 0 || t[..open].ends_with([' ', '\t']))
    {
        t = t[..open].trim_end();
    }
    let without = t.trim_end_matches('#');
    if without.is_empty() || without.ends_with([' ', '\t']) {
        t = without.trim_end();
    }
    t.to_string()
}

// ---------------------------------------------------------------------------
// Tree storage

/// Target number of entries per leaf.
const LEAF: usize = 64;

#[derive(Debug, Clone)]
struct Entry<T> {
    /// Offset from the leaf's base (the end of the previous leaf's last
    /// entry, or 0).
    start: usize,
    len: usize,
    value: T,
}

#[derive(Debug, Clone)]
struct Leaf<T> {
    entries: Vec<Entry<T>>,
    /// From the leaf base to the end of its last entry.
    span: usize,
}

/// An ordered sequence of non-overlapping byte ranges with values, stored
/// relative to their leaf so shifting everything after an edit only touches
/// one leaf and two Fenwick trees.
#[derive(Debug, Clone)]
struct Tree<T> {
    leaves: Vec<Leaf<T>>,
    spans: Fenwick,
    counts: Fenwick,
    len: usize,
}

impl<T> Default for Tree<T> {
    fn default() -> Self {
        Self { leaves: Vec::new(), spans: Fenwick::default(), counts: Fenwick::default(), len: 0 }
    }
}

impl<T: Clone> Tree<T> {
    fn from_entries(entries: Vec<(Range<usize>, T)>) -> Self {
        let mut tree = Self { leaves: chunk(entries, 0), ..Self::default() };
        tree.rebuild();
        tree
    }

    fn len(&self) -> usize {
        self.len
    }

    fn rebuild(&mut self) {
        self.spans = Fenwick::from_values(self.leaves.iter().map(|l| l.span));
        self.counts = Fenwick::from_values(self.leaves.iter().map(|l| l.entries.len()));
        self.len = self.leaves.iter().map(|l| l.entries.len()).sum();
    }

    /// Leaf holding entry `i` and the position in it.
    fn locate(&self, i: usize) -> Option<(usize, usize)> {
        if i >= self.len {
            return None;
        }
        let leaf = self.counts.search(i);
        Some((leaf, i - self.counts.prefix(leaf)))
    }

    fn get(&self, i: usize) -> Option<(Range<usize>, &T)> {
        let (leaf, k) = self.locate(i)?;
        let base = self.spans.prefix(leaf);
        let e = &self.leaves[leaf].entries[k];
        Some((base + e.start..base + e.start + e.len, &e.value))
    }

    /// Number of entries starting before `pos` (or at it, if `inclusive`).
    fn count_starting_before(&self, pos: usize, inclusive: bool) -> usize {
        if self.leaves.is_empty() {
            return 0;
        }
        let leaf = self.spans.search(pos).min(self.leaves.len() - 1);
        let base = self.spans.prefix(leaf);
        let rel = pos - base;
        let k = self.leaves[leaf].entries.partition_point(|e| if inclusive { e.start <= rel } else { e.start < rel });
        self.counts.prefix(leaf) + k
    }

    fn iter_from(&self, i: usize) -> TreeIter<'_, T> {
        match self.locate(i) {
            Some((leaf, k)) => TreeIter { tree: self, leaf, k, base: self.spans.prefix(leaf) },
            None => TreeIter { tree: self, leaf: self.leaves.len(), k: 0, base: 0 },
        }
    }

    /// Replace entries `range` (indices) with `new` (absolute ranges after
    /// the edit) and shift the entries after `range` by `delta`.
    fn splice(&mut self, range: Range<usize>, new: Vec<(Range<usize>, T)>, delta: isize) {
        if self.leaves.is_empty() {
            *self = Self::from_entries(new);
            return;
        }
        let last = self.leaves.len() - 1;
        let leaf_of = |i: usize| self.locate(i).map_or(last, |(l, _)| l);
        let la = leaf_of(range.start.min(self.len - 1));
        // Also take the leaf of the first entry after `range`, so its base
        // is recomputed; leaves after it keep their relative offsets.
        let lb = leaf_of(range.end).max(la);
        let base = self.spans.prefix(la);
        let mut i = self.counts.prefix(la);
        let mut flat: Vec<(Range<usize>, T)> = Vec::new();
        let mut new = Some(new);
        let mut leaf_base = base;
        for leaf in &self.leaves[la..=lb] {
            for e in &leaf.entries {
                let abs = leaf_base + e.start..leaf_base + e.start + e.len;
                if i == range.start {
                    flat.extend(new.take().into_iter().flatten());
                }
                if i < range.start {
                    flat.push((abs, e.value.clone()));
                } else if i >= range.end {
                    flat.push((shift(abs.start, delta)..shift(abs.end, delta), e.value.clone()));
                }
                i += 1;
            }
            leaf_base += leaf.span;
        }
        flat.extend(new.take().into_iter().flatten());
        let old_count = lb - la + 1;
        let leaves = chunk(flat, base);
        let same_shape = leaves.len() == old_count;
        self.leaves.splice(la..=lb, leaves);
        if same_shape {
            for l in la..=lb {
                let leaf = &self.leaves[l];
                self.spans.set(l, leaf.span);
                self.counts.set(l, leaf.entries.len());
            }
            self.len = self.counts.prefix(self.leaves.len());
        } else {
            self.rebuild();
        }
    }
}

/// Split absolute entries into leaves, the first based at `base`.
fn chunk<T>(entries: Vec<(Range<usize>, T)>, base: usize) -> Vec<Leaf<T>> {
    let total = entries.len();
    if total == 0 {
        return Vec::new();
    }
    let n_leaves = if total <= 2 * LEAF { 1 } else { total.div_ceil(LEAF) };
    let per = total.div_ceil(n_leaves);
    let mut leaves = Vec::with_capacity(n_leaves);
    let mut base = base;
    let mut iter = entries.into_iter().peekable();
    while iter.peek().is_some() {
        let mut entries = Vec::with_capacity(per);
        for (r, value) in iter.by_ref().take(per) {
            entries.push(Entry { start: r.start - base, len: r.end - r.start, value });
        }
        let span = entries.last().map_or(0, |e| e.start + e.len);
        base += span;
        leaves.push(Leaf { entries, span });
    }
    leaves
}

struct TreeIter<'a, T> {
    tree: &'a Tree<T>,
    leaf: usize,
    k: usize,
    base: usize,
}

impl<'a, T> Iterator for TreeIter<'a, T> {
    type Item = (Range<usize>, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let leaf = self.tree.leaves.get(self.leaf)?;
            if let Some(e) = leaf.entries.get(self.k) {
                self.k += 1;
                return Some((self.base + e.start..self.base + e.start + e.len, &e.value));
            }
            self.base += leaf.span;
            self.leaf += 1;
            self.k = 0;
        }
    }
}

/// Fenwick (binary indexed) tree of non-negative values.
#[derive(Debug, Clone, Default)]
struct Fenwick {
    tree: Vec<usize>,
    values: Vec<usize>,
}

impl Fenwick {
    fn from_values(values: impl Iterator<Item = usize>) -> Self {
        let values: Vec<usize> = values.collect();
        let mut tree = values.clone();
        for i in 0..tree.len() {
            let parent = i | (i + 1);
            if parent < tree.len() {
                tree[parent] += tree[i];
            }
        }
        Self { tree, values }
    }

    /// Sum of the first `n` values.
    fn prefix(&self, n: usize) -> usize {
        let mut sum = 0;
        let mut i = n;
        while i > 0 {
            sum += self.tree[i - 1];
            i &= i - 1;
        }
        sum
    }

    fn set(&mut self, i: usize, value: usize) {
        let old = std::mem::replace(&mut self.values[i], value);
        let mut j = i;
        while j < self.tree.len() {
            self.tree[j] = self.tree[j].wrapping_sub(old).wrapping_add(value);
            j |= j + 1;
        }
    }

    /// The number of leading values whose running sum stays `<= target`.
    fn search(&self, target: usize) -> usize {
        let mut pos = 0;
        let mut rem = target;
        let mut step = self.tree.len().next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= self.tree.len() && self.tree[next - 1] <= rem {
                pos = next;
                rem -= self.tree[next - 1];
            }
            step >>= 1;
        }
        pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RopeBuffer;
    use crate::edit::{Edit, EditOrigin, TxnId};

    #[test]
    fn finds_top_level_blocks() {
        let buf =
            RopeBuffer::from_text("# Title\n\nPara one\nstill one\n\n- a\n- b\n\n---\n\n```mermaid\ngraph\n```\n");
        let index = BlockIndex::parse(&buf);
        let kinds: Vec<_> = index.blocks().iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Heading { level: 1 },
                BlockKind::Paragraph,
                BlockKind::List { ordered: false },
                BlockKind::ThematicBreak,
                BlockKind::Mermaid,
            ]
        );
        assert_eq!(buf.slice(index.blocks().get(1).unwrap().bytes), "Para one\nstill one");
        assert_eq!(index.block_at(12), Some(1));
        assert_eq!(outline(&index, &buf)[0].title, "Title");
    }

    pub(crate) fn edit(buf: &mut RopeBuffer, index: &mut BlockIndex, range: Range<usize>, text: &str) {
        let applied = buf.apply(&Edit { range, text: text.into(), origin: EditOrigin::User, txn: TxnId(0) });
        index.on_edit(&applied, buf);
    }

    pub(crate) fn assert_matches_full(index: &BlockIndex, buf: &RopeBuffer) {
        let full = BlockIndex::parse(buf);
        let a: Vec<_> = index.blocks().iter().map(|b| (b.kind, b.bytes)).collect();
        let b: Vec<_> = full.blocks().iter().map(|b| (b.kind, b.bytes)).collect();
        assert_eq!(a, b, "text: {:?}", buf.text());
        let mut ra: Vec<_> = index.link_refs().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let mut rb: Vec<_> = full.link_refs().map(|(k, v)| (k.to_string(), v.clone())).collect();
        ra.sort_by(|x, y| x.0.cmp(&y.0));
        rb.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(ra, rb, "refs, text: {:?}", buf.text());
    }

    #[test]
    fn typing_keeps_untouched_generations() {
        let mut buf = RopeBuffer::from_text("# A\n\npara\n\n- x\n\nlast\n");
        let mut index = BlockIndex::parse(&buf);
        let before: Vec<_> = index.blocks().iter().collect();
        edit(&mut buf, &mut index, 9..9, "!");
        assert_matches_full(&index, &buf);
        let after: Vec<_> = index.blocks().iter().collect();
        assert_eq!(after[0].generation, before[0].generation);
        assert_ne!(after[1].generation, before[1].generation);
        assert_eq!(after[2].generation, before[2].generation);
        assert_eq!(after[3].bytes, before[3].bytes.start + 1..before[3].bytes.end + 1);
        assert_eq!(index.last_change().blocks, 0..3);
    }

    #[test]
    fn opening_a_fence_swallows_the_rest() {
        let mut buf = RopeBuffer::from_text("a\n\nb\n\nc\n\nd\n\ne\n");
        let mut index = BlockIndex::parse(&buf);
        edit(&mut buf, &mut index, 3..3, "```\n");
        assert_matches_full(&index, &buf);
        assert_eq!(index.len(), 2);
        edit(&mut buf, &mut index, 3..7, "");
        assert_matches_full(&index, &buf);
        assert_eq!(index.len(), 5);
    }

    #[test]
    fn link_refs_side_table() {
        let mut buf = RopeBuffer::from_text("[Foo  Bar]: /one\n\ntext [foo bar]\n\n[foo bar]: /two\n");
        let mut index = BlockIndex::parse(&buf);
        assert_eq!(index.link_ref("FOO bar").unwrap().dest, "/one");
        assert_eq!(index.len(), 1);
        edit(&mut buf, &mut index, 0..18, "");
        assert_eq!(index.link_ref("foo bar").unwrap().dest, "/two");
        assert_eq!(index.last_change().labels, vec!["foo bar".to_string()]);
        assert_matches_full(&index, &buf);
    }

    #[test]
    fn large_windows_become_jobs() {
        let body: String = (0..10000).map(|i| format!("para {i}\n\n")).collect();
        let mut buf = RopeBuffer::from_text(&format!("x\n\n{body}"));
        let mut index = BlockIndex::parse(&buf);
        edit(&mut buf, &mut index, 3..3, "```\n");
        assert!(index.is_pending());
        assert_eq!(index.len(), 10001, "old blocks are kept, shifted");
        let job = index.take_parse_job(&buf).unwrap();
        assert!(index.take_parse_job(&buf).is_none());
        // An edit while the job runs makes its result stale.
        edit(&mut buf, &mut index, 0..1, "y");
        let result = job.run();
        assert!(!index.finish_parse(result, &buf));
        assert!(index.is_pending());
        let job = index.take_parse_job(&buf).unwrap();
        assert!(index.finish_parse(job.run(), &buf));
        assert!(!index.is_pending());
        assert_matches_full(&index, &buf);
        edit(&mut buf, &mut index, 3..7, "");
        index.flush(&buf);
        assert_matches_full(&index, &buf);
    }

    #[test]
    fn deferred_index_parses_in_a_job() {
        let mut buf = RopeBuffer::from_text("# a\n\nb\n");
        let mut index = BlockIndex::deferred(&buf);
        assert!(index.is_pending() && index.is_empty());
        edit(&mut buf, &mut index, 0..0, "x\n\n");
        let job = index.take_parse_job(&buf).unwrap();
        assert!(index.finish_parse(job.run(), &buf));
        assert_matches_full(&index, &buf);
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn tree_lookups_across_leaves() {
        let text: String = (0..1000).map(|i| format!("p{i}\n\n")).collect();
        let mut buf = RopeBuffer::from_text(&text);
        let mut index = BlockIndex::parse(&buf);
        assert_eq!(index.len(), 1000);
        let b = index.get(700).unwrap();
        assert_eq!(buf.slice(b.bytes.clone()), "p700");
        assert_eq!(index.block_at(b.bytes.start), Some(700));
        assert_eq!(index.block_at(b.bytes.end + 1), Some(700));
        let viewport: Vec<_> = index.blocks_in(b.bytes.start..b.bytes.start + 14).collect();
        assert_eq!(viewport.len(), 3);
        edit(&mut buf, &mut index, 12..12, "x\n\ny\n\n");
        assert_matches_full(&index, &buf);
        assert_eq!(buf.slice(index.get(702).unwrap().bytes), "p700");
    }

    #[test]
    fn scan_outline_skips_fences() {
        let buf = RopeBuffer::from_text("# One #\n```\n# no\n```\n  ## Two {#t}\n#nope\n");
        let entries = scan_outline(&buf);
        assert_eq!(
            entries,
            vec![
                OutlineEntry { level: 1, title: "One".into(), byte: 0 },
                OutlineEntry { level: 2, title: "Two".into(), byte: 23 },
            ]
        );
        let index = BlockIndex::parse(&buf);
        let titles: Vec<_> = outline(&index, &buf).into_iter().map(|e| e.title).collect();
        assert_eq!(titles, vec!["One", "Two"]);
    }
}
