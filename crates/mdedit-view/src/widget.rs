//! The View-mode iced widget: block height map, lazy per-block layout,
//! reveal of the block under the cursor, input and drawing.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use iced::advanced::clipboard::Kind as ClipboardKind;
use iced::advanced::image::{self as adv_image, Renderer as _};
use iced::advanced::input_method::{self, InputMethod};
use iced::advanced::layout::{self as adv_layout, Layout};
use iced::advanced::svg::{self as adv_svg, Renderer as _};
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::widget::{self as adv_widget, operation};
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget, mouse, renderer};
use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::{Border, Color, Element, Event, Length, Point, Rectangle, Shadow, Size, Vector, window};
use mdedit_core::highlight::highlight_lines;
use mdedit_core::{Block, BlockKind, Document, Edit, EditOrigin, Selection, TxnId};
use mdedit_editor::HeightMap;
use mdedit_render::{IndexRefs, LayoutContext, layout_block_with};
use mdedit_theme::space::{self, SPACE_4, SPACE_6};
use mdedit_theme::{Theme, typography};

use crate::edit;
use crate::images;
use crate::layout::{self, BlockLayout, Ctx, Item, TableOp, TextItem};
use crate::style::Look;
use crate::table::Grid;
use crate::{Action, SyncAnchor, ViewState, line_start};

/// Layouts kept per widget.
const CACHE_CAP: usize = 512;
/// Extra px laid out below the viewport.
const OVERSCAN: f32 = 400.0;
/// The document may scroll until its end is this far up the viewport.
const END_SLACK: f32 = 0.6;

pub struct View<'a, Message> {
    doc: &'a Document,
    state: &'a ViewState,
    look: Look,
    on_action: Box<dyn Fn(Action) -> Message + 'a>,
    id: Option<adv_widget::Id>,
}

impl<Message> View<'_, Message> {
    /// Window-space bounding box of the visible selection, clipped to the
    /// widget (the same rectangles `draw` fills).
    fn selection_rect(&self, st: &State, bounds: Rectangle) -> Option<Rectangle> {
        let sel = self.selection(st).range();
        if sel.is_empty() || st.focus.is_none() || self.doc.is_large() {
            return None;
        }
        let mut out: Option<Rectangle> = None;
        for v in &st.visible {
            let origin = Vector::new(bounds.x + st.col_x, bounds.y + v.y);
            let laid = v.to_laid(sel.start)..v.to_laid(sel.end);
            for item in &v.layout.items {
                let Item::Text(t) = item else { continue };
                let Some(src) = t.source_range() else { continue };
                if laid.start > src.end || laid.end < src.start {
                    continue;
                }
                let o = Vector::new(origin.x + t.pos.x, origin.y + t.pos.y);
                for tr in t.text_ranges(laid.clone()) {
                    for rect in t.range_rects(tr.start, tr.end) {
                        let r = rect + o;
                        out = Some(out.map_or(r, |a| a.union(&r)));
                    }
                }
            }
        }
        out.and_then(|r| r.intersection(&bounds))
    }
}

impl<'a, Message> View<'a, Message> {
    pub fn new(
        doc: &'a Document,
        state: &'a ViewState,
        theme: &'a Theme,
        on_action: impl Fn(Action) -> Message + 'a,
    ) -> Self {
        Self { doc, state, look: Look::new(theme), on_action: Box::new(on_action), id: None }
    }

    /// Widget id, for `iced::widget::operation::focus`.
    pub fn id(mut self, id: impl Into<adv_widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    fn editable(&self) -> bool {
        !self.state.read_only && !self.doc.format.read_only
    }

    fn zoom(&self) -> f32 {
        if self.state.zoom > 0.0 { self.state.zoom } else { 1.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockId {
    generation: u64,
    len: usize,
}

struct Visible {
    index: usize,
    block: Block,
    /// Top of the block relative to the viewport.
    y: f32,
    layout: Rc<BlockLayout>,
}

impl Visible {
    /// Shift from layout-time source offsets to current ones.
    fn delta(&self) -> isize {
        self.block.bytes.start as isize - self.layout.laid_start as isize
    }

    fn to_current(&self, offset: usize) -> usize {
        (offset as isize + self.delta()).max(0) as usize
    }

    fn to_laid(&self, offset: usize) -> usize {
        (offset as isize - self.delta()).max(0) as usize
    }
}

#[derive(Debug, Clone, Copy)]
struct Focus {
    updated_at: Instant,
    now: Instant,
    window_focused: bool,
}

impl Focus {
    fn caret_visible(&self) -> bool {
        let blink = space::CURSOR_BLINK_MS as u128;
        (self.now.saturating_duration_since(self.updated_at).as_millis() / blink).is_multiple_of(2)
    }
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Select { anchor: usize, position: Point },
    Scrollbar { grab: f32 },
}

#[derive(Default)]
struct State {
    ids: Vec<BlockId>,
    heights: HeightMap,
    synced: Option<(u64, u64, f32, u64)>,
    anchor: usize,
    anchor_off: f32,
    external_scroll: f32,
    echo: Option<f32>,
    report_scroll: bool,
    sync_applied: Option<SyncAnchor>,
    cache: HashMap<u64, (Rc<BlockLayout>, u64)>,
    tick: u64,
    visible: Vec<Visible>,
    size: Size,
    col_x: f32,
    width: f32,
    wide: f32,
    focus: Option<Focus>,
    drag: Option<Drag>,
    goal_x: Option<f32>,
    pending_edit: bool,
    queued: VecDeque<Event>,
    local_selection: Option<Selection>,
    last_head: Option<usize>,
    image_gen: u64,
    hover: Option<Point>,
    clicks: Option<(Instant, Point, u8)>,
    modifiers: Modifiers,
    preedit: Option<input_method::Preedit>,
    /// The last `SelectionRect` published.
    sel_rect: Option<Rectangle>,
}

impl State {
    fn scroll(&self) -> f64 {
        if self.heights.is_empty() {
            return 0.0;
        }
        self.heights.offset_of(self.anchor.min(self.heights.len() - 1)) + self.anchor_off as f64
    }

    fn max_scroll(&self) -> f64 {
        (self.heights.total_height() - (self.size.height * (1.0 - END_SLACK)) as f64).max(0.0)
    }

    fn set_scroll(&mut self, y: f64) {
        let y = y.clamp(0.0, self.max_scroll());
        if self.heights.is_empty() {
            self.anchor = 0;
            self.anchor_off = 0.0;
            return;
        }
        let (i, top) = self.heights.line_at(y);
        self.anchor = i;
        self.anchor_off = (y - top) as f32;
    }

    fn scroll_by(&mut self, dy: f64) {
        let before = self.scroll();
        self.set_scroll(before + dy);
        if self.scroll() != before {
            self.report_scroll = true;
        }
    }

    fn accept_external_scroll(&mut self, y: f32) {
        if y == self.external_scroll {
            return;
        }
        self.external_scroll = y;
        if self.echo.take() == Some(y) {
            return;
        }
        self.set_scroll(y as f64);
    }

    fn visible_at(&self, y: f32) -> Option<&Visible> {
        self.visible
            .iter()
            .find(|v| v.y <= y && y < v.y + v.layout.height)
            .or_else(|| if y < self.visible.first()?.y { self.visible.first() } else { self.visible.last() })
    }
}

/// The blocks the view shows: the index's, or one block covering the text
/// when there is none (empty document).
impl operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focus.is_some()
    }

    fn focus(&mut self) {
        let now = Instant::now();
        self.focus = Some(Focus { updated_at: now, now, window_focused: true });
    }

    fn unfocus(&mut self) {
        self.focus = None;
        self.drag = None;
    }
}

fn blocks_of(doc: &Document) -> Vec<Block> {
    let mut out: Vec<Block> = doc.blocks().map(|i| i.blocks().iter().collect()).unwrap_or_default();
    if out.is_empty() {
        out.push(Block { kind: BlockKind::Paragraph, bytes: 0..doc.buffer().len_bytes(), generation: 0 });
    }
    out
}

/// See [`Frame::footnotes`].
fn footnote_key(doc: &Document, blocks: &[Block]) -> u64 {
    let buf = doc.buffer();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for b in blocks.iter().filter(|b| b.kind == BlockKind::FootnoteDefinition) {
        let head = buf.line(buf.byte_to_line(b.bytes.start));
        head.split("]:").next().hash(&mut h);
    }
    h.finish()
}

fn estimate(block: &Block) -> f32 {
    let len = block.bytes.len() as f32;
    let rows = (len / 80.0).ceil().max(1.0);
    let margin = SPACE_4;
    match block.kind {
        BlockKind::Heading { .. } => 48.0 + margin,
        BlockKind::CodeBlock { .. } | BlockKind::Math | BlockKind::Mermaid => rows * 22.0 + 2.0 * SPACE_4 + margin,
        BlockKind::ThematicBreak => SPACE_6 * 2.0,
        _ => rows * typography::BODY.line_height + margin,
    }
}

fn kind_tag(kind: &BlockKind) -> u8 {
    match kind {
        BlockKind::Paragraph => 0,
        BlockKind::Heading { level } => 10 + level,
        BlockKind::BlockQuote => 1,
        BlockKind::List { .. } => 2,
        BlockKind::CodeBlock { .. } => 3,
        BlockKind::HtmlBlock => 4,
        BlockKind::Table => 5,
        BlockKind::ThematicBreak => 6,
        BlockKind::FootnoteDefinition => 7,
        BlockKind::Math => 8,
        BlockKind::Mermaid => 9,
    }
}

/// Everything per frame that block layout depends on.
struct Frame<'b> {
    blocks: &'b [Block],
    selection: Selection,
    /// Block whose source is shown, and the source range shown.
    reveal: Option<(usize, Range<usize>)>,
    zoom: f32,
    /// Hash of the footnote labels in order: footnote numbers come from it,
    /// so blocks that mention footnotes are laid out again when it changes.
    footnotes: u64,
}

impl<'a, Message> View<'a, Message> {
    fn selection(&self, st: &State) -> Selection {
        let len = self.doc.buffer().len_bytes();
        let s = st.local_selection.unwrap_or(self.state.selection);
        Selection { anchor: s.anchor.min(len), head: s.head.min(len) }
    }

    fn eol(&self) -> &'static str {
        self.doc.format.line_ending.as_str()
    }

    /// The block to reveal for a cursor at `head`, and the source it shows:
    /// the block's lines, extended over the blank lines after it when the
    /// cursor sits there.
    fn reveal_for(&self, blocks: &[Block], head: usize) -> Option<(usize, Range<usize>)> {
        if !self.editable() {
            return None;
        }
        let buf = self.doc.buffer();
        let mut i = blocks.partition_point(|b| b.bytes.start <= head).saturating_sub(1);
        if let Some(next) = blocks.get(i + 1)
            && head >= line_start(buf, next.bytes.start)
        {
            i += 1;
        }
        let block = &blocks[i];
        if block.kind == BlockKind::Table && head >= block.bytes.start && head <= block.bytes.end {
            // Tables stay grids; the active cell shows its source.
            return None;
        }
        let start = if head < block.bytes.start && i == 0 { 0 } else { line_start(buf, block.bytes.start) };
        let end = if head > block.bytes.end {
            match blocks.get(i + 1) {
                Some(next) => line_start(buf, next.bytes.start).saturating_sub(1).max(block.bytes.end),
                None => buf.len_bytes(),
            }
        } else {
            block.bytes.end
        };
        // Strip a final CR so the range ends on a line content boundary.
        let end = if end > start && end < buf.len_bytes() && buf.slice(end - 1..end) == "\r" { end - 1 } else { end };
        Some((i, start.min(head)..end.max(head)))
    }

    fn sync(&self, st: &mut State, blocks: &[Block]) {
        let ids: Vec<BlockId> =
            blocks.iter().map(|b| BlockId { generation: b.generation, len: b.bytes.len() }).collect();
        if ids == st.ids && st.heights.len() == ids.len() {
            return;
        }
        let old = std::mem::take(&mut st.ids);
        let prefix = old.iter().zip(&ids).take_while(|(a, b)| a == b).count();
        let max_suffix = old.len().min(ids.len()) - prefix;
        let suffix = old.iter().rev().zip(ids.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
        if st.heights.len() != old.len() {
            st.heights = HeightMap::new(blocks.iter().map(estimate));
        } else {
            let estimates: Vec<f32> = blocks[prefix..ids.len() - suffix].iter().map(estimate).collect();
            st.heights.splice(prefix..old.len() - suffix, estimates);
        }
        // Keep the scroll anchor on the same block.
        if old.is_empty() {
            st.anchor = 0;
        } else if st.anchor >= old.len() - suffix {
            st.anchor = (st.anchor + ids.len()).saturating_sub(old.len());
        } else if st.anchor >= prefix {
            st.anchor = prefix;
        }
        st.anchor = st.anchor.min(ids.len().saturating_sub(1));
        st.ids = ids;
    }

    fn block_layout(
        &self,
        st: &mut State,
        frame: &Frame<'_>,
        i: usize,
        refs: &mut Option<IndexRefs<'a>>,
    ) -> Rc<BlockLayout> {
        let doc: &'a Document = self.doc;
        let buf = doc.buffer();
        let block = &frame.blocks[i];
        let prev_heading = i.checked_sub(1).and_then(|p| match frame.blocks[p].kind {
            BlockKind::Heading { level } => Some(level),
            _ => None,
        });
        let top = layout::top_margin(&block.kind, i == 0, prev_heading);
        let reveal = frame.reveal.as_ref().filter(|(r, _)| *r == i).map(|(_, range)| range.clone());
        let head = frame.selection.head;
        let table_cursor =
            (block.kind == BlockKind::Table && self.editable() && head >= block.bytes.start && head <= block.bytes.end)
                .then_some(head);

        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.look.generation.hash(&mut h);
        st.width.to_bits().hash(&mut h);
        st.wide.to_bits().hash(&mut h);
        frame.zoom.to_bits().hash(&mut h);
        kind_tag(&block.kind).hash(&mut h);
        top.to_bits().hash(&mut h);
        self.editable().hash(&mut h);
        let shown = reveal.clone().unwrap_or(line_start(buf, block.bytes.start)..block.bytes.end);
        (shown.start as isize - block.bytes.start as isize).hash(&mut h);
        table_cursor.map(|c| c - block.bytes.start).hash(&mut h);
        reveal.is_some().hash(&mut h);
        let mut footnotes = block.kind == BlockKind::FootnoteDefinition;
        let mut last = 0u8;
        for chunk in buf.chunks(shown.clone()) {
            footnotes |= (last == b'[' && chunk.first() == Some(&b'^')) || chunk.windows(2).any(|w| w == b"[^");
            last = chunk.last().copied().unwrap_or(last);
            chunk.hash(&mut h);
        }
        if footnotes {
            frame.footnotes.hash(&mut h);
        }
        let key = h.finish();
        st.tick += 1;
        let image_gen = images::cache().generation();
        if let Some((layout, used)) = st.cache.get_mut(&key)
            && layout.image_gen.is_none_or(|g| g == image_gen)
        {
            *used = st.tick;
            return Rc::clone(layout);
        }

        let ctx = Ctx {
            look: &self.look,
            zoom: frame.zoom,
            width: st.width,
            wide: st.wide,
            doc_path: self.doc.path.as_deref(),
            buf,
            cursor: table_cursor,
            editable: self.editable(),
            image_gen,
        };
        let mut laid = if let Some(range) = reveal {
            let first = buf.byte_to_line(range.start);
            let last = buf.byte_to_line(range.end);
            let tokens = match self.doc.blocks() {
                Some(index) => highlight_lines(index, buf, first..last + 1),
                None => Vec::new(),
            };
            let lines: Vec<(String, usize, Vec<mdedit_core::highlight::StyledRange>)> = (first..=last)
                .enumerate()
                .map(|(n, l)| {
                    (buf.line(l).into_owned(), buf.line_to_byte(l), tokens.get(n).cloned().unwrap_or_default())
                })
                .collect();
            let code = matches!(block.kind, BlockKind::CodeBlock { .. } | BlockKind::Math | BlockKind::Mermaid);
            let mut laid = layout::layout_source(&ctx, &lines, code, top);
            if block.kind == BlockKind::Paragraph && lines.iter().any(|(l, ..)| l.contains("![")) {
                let node = mdedit_render::layout_block(block, buf).node;
                layout::append_images(&ctx, &mut laid, &node);
            }
            laid
        } else {
            let refs: &IndexRefs<'_> = match (refs.as_ref(), doc.blocks()) {
                (Some(r), _) => r,
                (None, Some(index)) => refs.insert(IndexRefs::new(index, buf)),
                (None, None) => {
                    let node = mdedit_render::layout_block(block, buf).node;
                    return Rc::new(layout::layout_node(&ctx, &node, top, block.bytes.start));
                }
            };
            let lctx = LayoutContext { links: Some(refs), renderers: &[] };
            let node = layout_block_with(block, buf, &lctx).node;
            layout::layout_node(&ctx, &node, top, block.bytes.start)
        };
        laid.laid_start = block.bytes.start;
        let laid = Rc::new(laid);
        if st.cache.len() >= CACHE_CAP {
            let mut ages: Vec<(u64, u64)> = st.cache.iter().map(|(k, (_, u))| (*u, *k)).collect();
            ages.sort_unstable();
            for (_, k) in ages.iter().take(CACHE_CAP / 4) {
                st.cache.remove(k);
            }
        }
        st.cache.insert(key, (Rc::clone(&laid), st.tick));
        laid
    }

    /// Lay out the blocks in the viewport and update the height map.
    fn prepare(&self, st: &mut State) {
        let blocks = blocks_of(self.doc);
        self.sync(st, &blocks);
        let selection = self.selection(st);
        let frame = Frame {
            blocks: &blocks,
            selection,
            reveal: self.reveal_for(&blocks, selection.head),
            zoom: self.zoom(),
            footnotes: footnote_key(self.doc, &blocks),
        };
        let mut refs = None;
        let height = st.size.height;
        for _attempt in 0..3 {
            // Move the anchor up while the viewport top is above it.
            while st.anchor_off < 0.0 && st.anchor > 0 {
                st.anchor -= 1;
                let l = self.block_layout(st, &frame, st.anchor, &mut refs);
                st.heights.set_measured(st.anchor, l.height);
                st.anchor_off += l.height;
            }
            if st.anchor == 0 && st.anchor_off < 0.0 {
                st.anchor_off = 0.0;
            }
            st.visible.clear();
            let mut i = st.anchor;
            let mut y = -st.anchor_off;
            while i < blocks.len() && y < height + OVERSCAN {
                let l = self.block_layout(st, &frame, i, &mut refs);
                st.heights.set_measured(i, l.height);
                if y + l.height <= 0.0 && i + 1 < blocks.len() {
                    // Entirely above the viewport: move the anchor past it.
                    st.anchor = i + 1;
                    st.anchor_off = -(y + l.height);
                } else {
                    st.visible.push(Visible { index: i, block: blocks[i].clone(), y, layout: l });
                }
                y += st.heights.height(i);
                i += 1;
            }
            // Don't scroll past the end slack.
            let max = st.max_scroll();
            if st.scroll() > max + 0.5 {
                st.set_scroll(max);
                continue;
            }
            break;
        }
    }

    /// Scroll so the caret is in view.
    fn follow(&self, st: &mut State, head: usize) {
        let blocks = blocks_of(self.doc);
        let i = blocks.partition_point(|b| b.bytes.start <= head).saturating_sub(1);
        let inside = st.visible.iter().any(|v| v.index == i && v.y >= -v.layout.height && v.y < st.size.height);
        if !inside {
            st.anchor = i;
            st.anchor_off = -st.size.height / 3.0;
            st.report_scroll = true;
            self.prepare(st);
        }
        if let Some(c) = self.caret_rect(st, head) {
            let margin = 24.0;
            if c.y < margin {
                st.scroll_by((c.y - margin) as f64);
                self.prepare(st);
            } else if c.y + c.height > st.size.height - margin {
                st.scroll_by((c.y + c.height - st.size.height + margin) as f64);
                self.prepare(st);
            }
        }
    }

    /// Caret rectangle in widget coordinates.
    fn caret_rect(&self, st: &State, head: usize) -> Option<Rectangle> {
        for v in &st.visible {
            let laid = v.to_laid(head);
            for t in v.layout.texts() {
                let Some(range) = t.source_range() else { continue };
                if laid < range.start || laid > range.end {
                    continue;
                }
                // Only items that show this position editable: revealed
                // blocks and the active table cell.
                let source_mode = v.layout.revealed || t.map.iter().all(|m| m.exact) && v.layout.table.is_some();
                if !source_mode {
                    continue;
                }
                if let Some(off) = t.text_at_source(laid) {
                    let c = t.caret(off);
                    return Some(Rectangle::new(
                        Point::new(st.col_x + t.pos.x + c.x, v.y + t.pos.y + c.y),
                        Size::new(space::CURSOR_WIDTH, c.height),
                    ));
                }
            }
            if let Some(t) = &v.layout.table
                && let Some(cell) = t.cells.iter().find(|c| c.content.is_empty() && c.content.start == laid)
            {
                let h = typography::BODY.line_height * self.zoom();
                return Some(Rectangle::new(
                    Point::new(st.col_x + cell.rect.x + SPACE_4, v.y + cell.rect.y + (cell.rect.height - h) / 2.0),
                    Size::new(space::CURSOR_WIDTH, h),
                ));
            }
        }
        None
    }

    /// Source offset under a point in widget coordinates.
    fn hit(&self, st: &State, p: Point) -> Option<usize> {
        let v = st.visible_at(p.y)?;
        let local = Point::new(p.x - st.col_x, p.y - v.y);
        // Empty table cells have no text to hit.
        if let Some(t) = &v.layout.table
            && let Some(cell) = t.cells.iter().find(|c| c.rect.contains(local))
            && cell.content.is_empty()
        {
            return Some(v.to_current(cell.content.start));
        }
        let mut best: Option<(f32, &TextItem)> = None;
        for t in v.layout.texts() {
            if t.map.is_empty()
                || t.source_range().is_none_or(|r| r.is_empty() && t.text.is_empty() && !v.layout.revealed)
            {
                continue;
            }
            let b = t.bounds();
            let dy = if local.y < b.y {
                b.y - local.y
            } else if local.y > b.y + b.height {
                local.y - b.y - b.height
            } else {
                0.0
            };
            let dx = if local.x < b.x {
                b.x - local.x
            } else if local.x > b.x + b.width.max(1.0) {
                local.x - b.x - b.width
            } else {
                0.0
            };
            let d = dy * 1000.0 + dx;
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, t));
            }
        }
        match best {
            Some((_, t)) => {
                let off = t.hit(Point::new(local.x - t.pos.x, local.y - t.pos.y));
                Some(v.to_current(t.to_source(off)?))
            }
            None => Some(v.block.bytes.start),
        }
    }

    fn emit(&self, st: &mut State, shell: &mut Shell<'_, Message>, actions: Vec<Action>) {
        for action in actions {
            match &action {
                Action::Edit(_) => st.pending_edit = true,
                Action::Select(s) => {
                    st.local_selection = Some(*s);
                    if let Some(f) = &mut st.focus {
                        f.updated_at = Instant::now();
                    }
                }
                Action::Scroll(y) => st.echo = Some(*y),
                Action::OpenLink(_) | Action::SelectionRect(_) => {}
            }
            shell.publish((self.on_action)(action));
        }
    }

    fn replace(&self, range: Range<usize>, text: &str, caret: usize) -> Vec<Action> {
        vec![
            Action::Edit(Edit { range, text: text.to_owned(), origin: EditOrigin::User, txn: TxnId(0) }),
            Action::Select(Selection::cursor(caret)),
        ]
    }

    /// The table cell holding `head`, from the visible layouts.
    fn table_at(&self, st: &State, head: usize) -> Option<(usize, layout::Cell)> {
        for (vi, v) in st.visible.iter().enumerate() {
            let Some(t) = &v.layout.table else { continue };
            let laid = v.to_laid(head);
            if laid < t.source.start || laid > t.source.end {
                continue;
            }
            if let Some(cell) = t.cells.iter().find(|c| c.content.start <= laid && laid <= c.content.end) {
                return Some((vi, cell.clone()));
            }
        }
        None
    }

    fn table_grid(&self, v: &Visible) -> Option<Grid> {
        let t = v.layout.table.as_ref()?;
        let buf = self.doc.buffer();
        let mut rows = vec![Vec::new(); t.rows];
        for c in &t.cells {
            let r = v.to_current(c.content.start)..v.to_current(c.content.end);
            rows[c.row].push(buf.slice(r).into_owned());
        }
        Some(Grid { rows, align: t.align.clone() })
    }

    fn table_op(&self, st: &State, vi: usize, op: TableOp, row: usize, col: usize) -> Vec<Action> {
        let v = &st.visible[vi];
        let (Some(t), Some(mut grid)) = (&v.layout.table, self.table_grid(v)) else { return Vec::new() };
        let Some((r, c)) = grid.apply(op, row, col) else { return Vec::new() };
        let (md, offsets) = grid.to_markdown(self.eol());
        let range = v.to_current(t.source.start)..v.to_current(t.source.end);
        let caret = range.start + offsets[r][c] + grid.rows[r][c].len();
        self.replace(range, &md, caret)
    }

    /// Tab, Shift+Tab and Enter inside a table.
    fn table_key(&self, st: &State, named: Named, shift: bool, head: usize) -> Option<Vec<Action>> {
        let (vi, cell) = self.table_at(st, head)?;
        let v = &st.visible[vi];
        let t = v.layout.table.as_ref()?;
        let find = |r: usize, c: usize| t.cells.iter().find(|x| x.row == r && x.col == c);
        let select_cell = |cell: &layout::Cell| {
            let range = v.to_current(cell.content.start)..v.to_current(cell.content.end);
            vec![Action::Select(Selection { anchor: range.start, head: range.end })]
        };
        match named {
            Named::Tab => {
                let idx = cell.row * t.cols + cell.col;
                let next = if shift { idx.checked_sub(1)? } else { idx + 1 };
                match find(next / t.cols, next % t.cols) {
                    Some(c) => Some(select_cell(c)),
                    // Tab in the last cell adds a row.
                    None if !shift => Some(self.table_op(st, vi, TableOp::RowBelow, cell.row, 0)),
                    None => Some(Vec::new()),
                }
            }
            Named::Enter => match find(cell.row + 1, cell.col) {
                Some(c) => Some(select_cell(c)),
                None => Some(self.table_op(st, vi, TableOp::RowBelow, cell.row, cell.col)),
            },
            _ => None,
        }
    }

    /// Move the caret one visual row up or down from `head`.
    fn vertical(&self, st: &mut State, head: usize, down: bool) -> usize {
        let Some(c) = self.caret_rect(st, head) else { return head };
        let x = *st.goal_x.get_or_insert(c.x);
        let y = if down { c.y + c.height + 1.0 } else { c.y - 1.0 };
        if y < 0.0 {
            st.scroll_by(y as f64 - 40.0);
            self.prepare(st);
            return self.vertical_after_scroll(st, head, x, down);
        }
        if y > st.size.height {
            st.scroll_by((y - st.size.height) as f64 + 40.0);
            self.prepare(st);
            return self.vertical_after_scroll(st, head, x, down);
        }
        self.step_from(st, head, x, y, down)
    }

    fn vertical_after_scroll(&self, st: &mut State, head: usize, x: f32, down: bool) -> usize {
        let Some(c) = self.caret_rect(st, head) else { return head };
        let y = if down { c.y + c.height + 1.0 } else { c.y - 1.0 };
        self.step_from(st, head, x, y, down)
    }

    fn step_from(&self, st: &State, head: usize, x: f32, y: f32, down: bool) -> usize {
        match self.hit(st, Point::new(x, y)) {
            Some(p) if p != head => p,
            _ => {
                if down {
                    self.doc.buffer().len_bytes()
                } else {
                    0
                }
            }
        }
    }

    fn handle_key(
        &self,
        st: &mut State,
        key: &Key,
        modifiers: Modifiers,
        text: Option<&str>,
        clipboard: &mut dyn Clipboard,
    ) -> Option<Vec<Action>> {
        let buf = self.doc.buffer();
        let sel = self.selection(st);
        let range = sel.range();
        let head = sel.head;
        let cmd = modifiers.command();
        let shift = modifiers.shift();
        let to = |pos: usize| {
            vec![Action::Select(if shift {
                Selection { anchor: sel.anchor, head: pos }
            } else {
                Selection::cursor(pos)
            })]
        };
        let delete = |r: Range<usize>| {
            if r.is_empty() {
                return Vec::new();
            }
            let start = r.start;
            vec![Action::Edit(Edit::delete(r, EditOrigin::User, TxnId(0))), Action::Select(Selection::cursor(start))]
        };
        if cmd
            && !modifiers.alt()
            && let Key::Character(c) = key.as_ref()
        {
            return match c.to_lowercase().as_str() {
                "a" => Some(vec![Action::Select(Selection { anchor: 0, head: buf.len_bytes() })]),
                "c" | "x" => {
                    if range.is_empty() {
                        return Some(Vec::new());
                    }
                    clipboard.write(ClipboardKind::Standard, buf.slice(range.clone()).into_owned());
                    Some(if c.eq_ignore_ascii_case("x") { delete(range) } else { Vec::new() })
                }
                "v" => {
                    let text = clipboard.read(ClipboardKind::Standard)?;
                    let start = range.start;
                    Some(self.replace(range, &text, start + text.len()))
                }
                _ => None,
            };
        }
        if let Key::Named(named) = key.as_ref() {
            if !matches!(named, Named::ArrowUp | Named::ArrowDown | Named::PageUp | Named::PageDown) {
                st.goal_x = None;
            }
            if matches!(named, Named::Tab | Named::Enter)
                && !cmd
                && let Some(actions) = self.table_key(st, named, shift, head)
            {
                return Some(actions);
            }
            let actions = match named {
                Named::Enter if cmd => return None,
                Named::Enter if shift => {
                    let eol = self.eol();
                    self.replace(range.clone(), eol, range.start + eol.len())
                }
                Named::Enter => {
                    let blocks = blocks_of(self.doc);
                    let i = blocks.partition_point(|b| b.bytes.start <= head).saturating_sub(1);
                    let in_code = head <= blocks[i].bytes.end
                        && matches!(blocks[i].kind, BlockKind::CodeBlock { .. } | BlockKind::Math | BlockKind::Mermaid);
                    let (r, text, caret) = edit::enter(buf, range, self.eol(), in_code);
                    self.replace(r, &text, caret)
                }
                Named::Tab if !cmd => {
                    // Indent or outdent a list item; elsewhere Tab is not ours.
                    let line = edit::line_range(buf, head);
                    let text = buf.slice(line.clone());
                    if !edit::line_prefix(&text).container || text.trim_start().starts_with('>') {
                        return None;
                    }
                    if shift {
                        let n = text.bytes().take(4).take_while(|&b| b == b' ').count().min(2);
                        if n == 0 {
                            return Some(Vec::new());
                        }
                        let caret = head.saturating_sub(n).max(line.start);
                        vec![
                            Action::Edit(Edit::delete(line.start..line.start + n, EditOrigin::User, TxnId(0))),
                            Action::Select(Selection::cursor(caret)),
                        ]
                    } else {
                        self.replace(line.start..line.start, "  ", head + 2)
                    }
                }
                Named::Backspace if !range.is_empty() => delete(range),
                Named::Backspace if cmd => delete(edit::word_left(buf, head)..head),
                Named::Backspace => delete(edit::prev_grapheme(buf, head)..head),
                Named::Delete if !range.is_empty() => delete(range),
                Named::Delete if cmd => delete(head..edit::word_right(buf, head)),
                Named::Delete => delete(head..edit::next_grapheme(buf, head)),
                Named::ArrowLeft if !shift && !range.is_empty() => to(range.start),
                Named::ArrowRight if !shift && !range.is_empty() => to(range.end),
                Named::ArrowLeft if cmd => to(edit::word_left(buf, head)),
                Named::ArrowRight if cmd => to(edit::word_right(buf, head)),
                Named::ArrowLeft => to(edit::prev_grapheme(buf, head)),
                Named::ArrowRight => to(edit::next_grapheme(buf, head)),
                Named::ArrowUp if cmd => to(0),
                Named::ArrowDown if cmd => to(buf.len_bytes()),
                Named::ArrowUp => to(self.vertical(st, head, false)),
                Named::ArrowDown => to(self.vertical(st, head, true)),
                Named::Home if cmd => to(0),
                Named::End if cmd => to(buf.len_bytes()),
                Named::Home | Named::End => {
                    let home = named == Named::Home;
                    let line = edit::line_range(buf, head);
                    let pos = self.row_edge(st, head, home).unwrap_or(if home { line.start } else { line.end });
                    to(pos)
                }
                Named::PageUp | Named::PageDown => {
                    let dy = st.size.height * 0.9 * if named == Named::PageUp { -1.0 } else { 1.0 };
                    let caret = self.caret_rect(st, head);
                    st.scroll_by(dy as f64);
                    self.prepare(st);
                    match caret {
                        Some(c) => to(self.hit(st, Point::new(c.x, c.y + c.height / 2.0)).unwrap_or(head)),
                        None => Vec::new(),
                    }
                }
                Named::Escape => return None,
                Named::Space => {
                    let start = range.start;
                    self.replace(range, " ", start + 1)
                }
                _ => return None,
            };
            return Some(actions);
        }
        let text = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))?;
        if cmd || modifiers.control() && !modifiers.alt() {
            return None;
        }
        let start = range.start;
        Some(self.replace(range, text, start + text.len()))
    }

    /// Start or end of the visual row holding `head`.
    fn row_edge(&self, st: &State, head: usize, home: bool) -> Option<usize> {
        for v in &st.visible {
            let laid = v.to_laid(head);
            for t in v.layout.texts() {
                let Some(r) = t.source_range() else { continue };
                if laid < r.start || laid > r.end || !(v.layout.revealed || v.layout.table.is_some()) {
                    continue;
                }
                let off = t.text_at_source(laid)?;
                let (s, e) = t.row_bounds(off);
                let e = if e > s && t.text[..e].ends_with('\n') { e - 1 } else { e };
                let e = if e > s && e < t.text.len() && t.text[..e].ends_with(' ') { e - 1 } else { e };
                return Some(v.to_current(t.to_source(if home { s } else { e })?));
            }
        }
        None
    }

    fn handle(
        &self,
        st: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        let editable = self.editable();
        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => -*y as f64 * 3.0 * typography::BODY.line_height as f64,
                    mouse::ScrollDelta::Pixels { y, .. } => -*y as f64,
                };
                st.scroll_by(dy);
                self.prepare(st);
                if st.report_scroll {
                    st.report_scroll = false;
                    let y = st.scroll() as f32;
                    self.emit(st, shell, vec![Action::Scroll(y)]);
                }
                shell.request_redraw();
                true
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position_in(bounds) else {
                    st.focus = None;
                    return false;
                };
                let now = Instant::now();
                st.focus = Some(Focus { updated_at: now, now, window_focused: true });
                // Scrollbar.
                if let Some(thumb) = self.thumb(st)
                    && pos.x >= st.size.width - 12.0
                {
                    let grab = if thumb.contains(pos) { pos.y - thumb.y } else { thumb.height / 2.0 };
                    st.drag = Some(Drag::Scrollbar { grab });
                    self.drag_scrollbar(st, pos.y, grab);
                    shell.request_redraw();
                    return true;
                }
                if let Some(actions) = self.click_controls(st, pos, clipboard) {
                    self.emit(st, shell, actions);
                    shell.request_redraw();
                    return true;
                }
                // Read-only text still selects (and copies); it just shows no caret.
                let Some(at) = self.hit(st, pos) else { return true };
                let count = match st.clicks {
                    Some((t, p, n)) if now - t < Duration::from_millis(400) && p.distance(pos) < 4.0 => n % 3 + 1,
                    _ => 1,
                };
                st.clicks = Some((now, pos, count));
                let sel = self.selection(st);
                let selection = match count {
                    2 => {
                        let w = edit::word_at(self.doc.buffer(), at);
                        Selection { anchor: w.start, head: w.end }
                    }
                    3 => {
                        let l = edit::line_range(self.doc.buffer(), at);
                        Selection { anchor: l.start, head: l.end }
                    }
                    _ if st.modifiers.shift() => Selection { anchor: sel.anchor, head: at },
                    _ => Selection::cursor(at),
                };
                st.drag = Some(Drag::Select { anchor: selection.anchor, position: pos });
                st.goal_x = None;
                self.emit(st, shell, vec![Action::Select(selection)]);
                shell.request_redraw();
                true
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                st.hover = cursor.position_in(bounds);
                match st.drag {
                    Some(Drag::Select { anchor, .. }) => {
                        st.drag = Some(Drag::Select { anchor, position: *position - Vector::new(bounds.x, bounds.y) });
                        let local =
                            Point::new(position.x - bounds.x, (position.y - bounds.y).clamp(0.0, bounds.height));
                        if let Some(at) = self.hit(st, local) {
                            let selection = Selection { anchor, head: at };
                            if selection != self.selection(st) {
                                self.emit(st, shell, vec![Action::Select(selection)]);
                            }
                        }
                        shell.request_redraw();
                        true
                    }
                    Some(Drag::Scrollbar { grab }) => {
                        self.drag_scrollbar(st, position.y - bounds.y, grab);
                        shell.request_redraw();
                        true
                    }
                    None => {
                        if st.visible.iter().any(|v| v.layout.items.iter().any(|i| matches!(i, Item::Copy { .. })))
                            || st.visible.iter().any(|v| v.layout.table.as_ref().is_some_and(|t| !t.toolbar.is_empty()))
                        {
                            shell.request_redraw();
                        }
                        false
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => st.drag.take().is_some(),
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                st.modifiers = *m;
                false
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, text, .. }) => {
                if st.focus.is_none() || !editable && !read_only_key(key, *modifiers) {
                    return false;
                }
                if st.pending_edit {
                    st.queued.push_back(event.clone());
                    return true;
                }
                match self.handle_key(st, key, *modifiers, text.as_deref(), clipboard) {
                    Some(actions) => {
                        if let Some(f) = &mut st.focus {
                            f.updated_at = Instant::now();
                        }
                        self.emit(st, shell, actions);
                        shell.request_redraw();
                        true
                    }
                    None => false,
                }
            }
            Event::InputMethod(ime) if st.focus.is_some() && editable => match ime {
                input_method::Event::Opened | input_method::Event::Closed => {
                    st.preedit = None;
                    true
                }
                input_method::Event::Preedit(content, selection) => {
                    st.preedit = Some(input_method::Preedit {
                        content: content.clone(),
                        selection: selection.clone(),
                        text_size: None,
                    });
                    shell.request_redraw();
                    true
                }
                input_method::Event::Commit(text) => {
                    st.preedit = None;
                    if st.pending_edit {
                        return true;
                    }
                    let range = self.selection(st).range();
                    let start = range.start;
                    let actions = self.replace(range, text, start + text.len());
                    self.emit(st, shell, actions);
                    true
                }
            },
            _ => false,
        }
    }

    /// Checkboxes, copy buttons, links and the table toolbar.
    fn click_controls(&self, st: &State, pos: Point, clipboard: &mut dyn Clipboard) -> Option<Vec<Action>> {
        let editable = self.editable();
        for (vi, v) in st.visible.iter().enumerate() {
            let local = Point::new(pos.x - st.col_x, pos.y - v.y);
            if let Some(t) = &v.layout.table
                && editable
            {
                for (rect, op, _) in &t.toolbar {
                    if rect.contains(local) {
                        let head = self.selection(st).head;
                        let laid = v.to_laid(head);
                        let cell = t.cells.iter().find(|c| c.content.start <= laid && laid <= c.content.end)?;
                        return Some(self.table_op(st, vi, *op, cell.row, cell.col));
                    }
                }
            }
            if local.y < 0.0 || local.y > v.layout.height {
                continue;
            }
            for item in &v.layout.items {
                match item {
                    Item::Check { rect, checked, source } if editable && pad(*rect, 4.0).contains(local) => {
                        let r = v.to_current(source.start)..v.to_current(source.end);
                        let text = if *checked { "[ ]" } else { "[x]" };
                        let caret = self.selection(st).head;
                        return Some(vec![
                            Action::Edit(Edit { range: r, text: text.into(), origin: EditOrigin::User, txn: TxnId(0) }),
                            Action::Select(Selection::cursor(caret)),
                        ]);
                    }
                    Item::Copy { rect, code } if rect.contains(local) => {
                        clipboard.write(ClipboardKind::Standard, code.clone());
                        return Some(Vec::new());
                    }
                    Item::Text(t) if !t.links.is_empty() && (!editable || st.modifiers.command()) => {
                        let b = t.bounds();
                        if b.contains(local) {
                            let off = t.hit(Point::new(local.x - t.pos.x, local.y - t.pos.y));
                            if let Some(url) = t.link_at(off) {
                                return Some(vec![Action::OpenLink(url.to_string())]);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    fn thumb(&self, st: &State) -> Option<Rectangle> {
        let total = st.heights.total_height() as f32 + st.size.height * (1.0 - END_SLACK);
        let h = st.size.height;
        if total <= h + 1.0 {
            return None;
        }
        let th = (h * h / total).max(32.0);
        let y = (st.scroll() as f32 / (total - h)).clamp(0.0, 1.0) * (h - th);
        Some(Rectangle::new(Point::new(st.size.width - 10.0, y), Size::new(6.0, th)))
    }

    fn drag_scrollbar(&self, st: &mut State, y: f32, grab: f32) {
        let total = st.heights.total_height() as f32 + st.size.height * (1.0 - END_SLACK);
        let h = st.size.height;
        let th = (h * h / total).max(32.0);
        let frac = ((y - grab) / (h - th).max(1.0)).clamp(0.0, 1.0);
        st.set_scroll((frac * (total - h)) as f64);
        st.report_scroll = true;
        self.prepare(st);
    }

    fn apply_sync(&self, st: &mut State) {
        let Some(anchor) = self.state.sync else { return };
        if st.sync_applied == Some(anchor) {
            return;
        }
        st.sync_applied = Some(anchor);
        let blocks = blocks_of(self.doc);
        let i = blocks.partition_point(|b| b.bytes.start <= anchor.byte).saturating_sub(1);
        let block = &blocks[i];
        // Proportional position inside the block, by line.
        let buf = self.doc.buffer();
        let first = buf.byte_to_line(block.bytes.start) as f32;
        let last = buf.byte_to_line(block.bytes.end) as f32;
        let line = buf.byte_to_line(anchor.byte.min(buf.len_bytes())) as f32 + anchor.fraction;
        let frac = if anchor.byte > block.bytes.end || last <= first {
            if anchor.byte > block.bytes.end { 1.0 } else { 0.0 }
        } else {
            ((line - first) / (last - first + 1.0)).clamp(0.0, 1.0)
        };
        st.anchor = i;
        st.anchor_off = 0.0;
        self.prepare(st);
        let h = st.heights.height(i);
        let top = if i == 0 { 0.0 } else { layout::top_margin(&block.kind, false, None) };
        st.anchor_off = top + (h - top) * frac;
        st.report_scroll = true;
    }
}

fn pad(r: Rectangle, by: f32) -> Rectangle {
    Rectangle::new(Point::new(r.x - by, r.y - by), Size::new(r.width + 2.0 * by, r.height + 2.0 * by))
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for View<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn diff(&self, tree: &mut Tree) {
        // A rebuild means the app has processed what we published.
        let st = tree.state.downcast_mut::<State>();
        st.pending_edit = false;
        st.local_selection = None;
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &adv_layout::Limits) -> adv_layout::Node {
        let size = limits.width(Length::Fill).height(Length::Fill).max();
        let st = tree.state.downcast_mut::<State>();
        let zoom = self.zoom();
        let side = if size.width < 600.0 { SPACE_4 } else { SPACE_6 };
        let avail = (size.width - 2.0 * side).max(80.0);
        let measure = if self.state.full_width { avail } else { (typography::MEASURE * zoom).min(avail) };
        st.size = size;
        st.width = measure.floor();
        st.col_x = ((size.width - st.width) / 2.0).max(side).floor();
        // Wide blocks (tables, code, images) grow to the right of the column.
        st.wide = (typography::MEASURE_WIDE * zoom).min(size.width - st.col_x - side).max(st.width).floor();
        if self.doc.is_large() {
            st.visible.clear();
            return adv_layout::Node::new(size);
        }
        let blocks_len = self.doc.blocks().map_or(0, |i| i.len());
        let key = (self.doc.buffer().version(), blocks_len as u64, size.width, self.look.generation);
        if st.synced.is_some_and(|k| (k.0, k.1, k.2, k.3) != key) {
            st.goal_x = st.goal_x.filter(|_| st.synced.is_some_and(|k| k.2 == size.width));
        }
        st.synced = Some(key);
        st.accept_external_scroll(self.state.scroll_y);
        self.prepare(st);
        self.apply_sync(st);
        if st.sync_applied.is_some() {
            self.prepare(st);
        }
        let head = self.selection(st).head;
        if self.editable() && st.last_head.is_some_and(|h| h != head) {
            self.follow(st, head);
        }
        st.last_head = Some(head);
        st.image_gen = images::cache().generation();
        adv_layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        match event {
            Event::Window(window::Event::Unfocused) => {
                if let Some(f) = &mut st.focus {
                    f.window_focused = false;
                }
            }
            Event::Window(window::Event::Focused) => {
                if let Some(f) = &mut st.focus {
                    f.window_focused = true;
                    f.updated_at = Instant::now();
                    shell.request_redraw();
                }
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                if let Some(f) = &mut st.focus
                    && f.window_focused
                    && self.editable()
                {
                    f.now = *now;
                    let blink = space::CURSOR_BLINK_MS as u128;
                    let wait = blink - (f.now.saturating_duration_since(f.updated_at)).as_millis() % blink;
                    shell.request_redraw_at(*now + Duration::from_millis(wait as u64));
                }
                // Images that finished decoding need a new layout.
                let generation = images::cache().generation();
                if generation != st.image_gen {
                    st.image_gen = generation;
                    shell.invalidate_layout();
                    shell.request_redraw();
                } else if images::cache().is_loading() {
                    shell.request_redraw_at(*now + Duration::from_millis(50));
                }
                if let Some(Drag::Select { anchor, position }) = st.drag {
                    let above = -position.y;
                    let below = position.y - bounds.height;
                    let dist = above.max(below);
                    if dist > 0.0 {
                        st.scroll_by(if above > 0.0 {
                            -(dist as f64 / 4.0).clamp(2.0, 60.0)
                        } else {
                            (dist as f64 / 4.0).clamp(2.0, 60.0)
                        });
                        self.prepare(st);
                        let edge = if above > 0.0 { 0.0 } else { bounds.height - 1.0 };
                        if let Some(at) = self.hit(st, Point::new(position.x, edge)) {
                            let selection = Selection { anchor, head: at };
                            if selection != self.selection(st) {
                                self.emit(st, shell, vec![Action::Select(selection)]);
                            }
                        }
                        shell.request_redraw();
                    }
                }
                if std::mem::take(&mut st.report_scroll) {
                    let y = st.scroll() as f32;
                    self.emit(st, shell, vec![Action::Scroll(y)]);
                }
                let head = self.selection(st).head;
                let ime = match (&st.focus, self.caret_rect(st, head)) {
                    (Some(f), Some(rect)) if f.window_focused && self.editable() => InputMethod::Enabled {
                        cursor: rect + Vector::new(bounds.x, bounds.y),
                        purpose: input_method::Purpose::Normal,
                        preedit: st.preedit.as_ref().map(input_method::Preedit::as_ref),
                    },
                    _ => InputMethod::Disabled,
                };
                shell.request_input_method(&ime);
                let rect = self.selection_rect(st, bounds);
                if rect != st.sel_rect {
                    st.sel_rect = rect;
                    shell.publish((self.on_action)(Action::SelectionRect(rect)));
                }
            }
            _ => {}
        }

        // Replay keys queued behind an edit the app has now applied.
        while !st.pending_edit
            && let Some(queued) = st.queued.pop_front()
        {
            self.handle(st, &queued, bounds, cursor, clipboard, shell);
        }
        if !st.queued.is_empty() {
            shell.request_redraw();
        }
        if self.handle(st, event, bounds, cursor, clipboard, shell) {
            shell.capture_event();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let c = self.look.colors;
        let quad = |r: &mut iced::Renderer, rect: Rectangle, color: Color, border: Border| {
            r.fill_quad(renderer::Quad { bounds: rect, border, ..renderer::Quad::default() }, color);
        };
        let selection = self.selection(st);
        let sel = selection.range();
        let hover = cursor.position_in(bounds);

        renderer.with_layer(bounds, |r| {
            quad(r, bounds, c.bg, Border::default());
            if self.doc.is_large() {
                return;
            }
            for v in &st.visible {
                let origin = Vector::new(bounds.x + st.col_x, bounds.y + v.y);
                let hovered = hover.is_some_and(|p| p.y >= v.y && p.y < v.y + v.layout.height);
                for item in &v.layout.items {
                    match item {
                        Item::Quad { rect, color, border } => quad(r, *rect + origin, *color, *border),
                        Item::Image { rect, handle, .. } => {
                            let image = adv_image::Image {
                                border_radius: space::RADIUS_LG.into(),
                                ..adv_image::Image::new(handle.clone())
                            };
                            r.draw_image(image, *rect + origin, bounds);
                        }
                        Item::Check { rect, checked, .. } => {
                            let rect = *rect + origin;
                            let border = Border { color: c.accent, width: 1.5, radius: space::RADIUS_SM.into() };
                            quad(r, rect, if *checked { c.accent } else { Color::TRANSPARENT }, border);
                            if *checked && let Some(icon) = mdedit_theme::icon("check") {
                                let svg = adv_svg::Svg::new(adv_svg::Handle::from_memory(icon)).color(c.on_accent);
                                r.draw_svg(svg, pad(rect, -1.5), bounds);
                            }
                        }
                        Item::Copy { rect, .. } => {
                            if hovered
                                && self.editable() | self.state.read_only
                                && let Some(icon) = mdedit_theme::icon("copy")
                            {
                                let rect = *rect + origin;
                                let over = hover.is_some_and(|p| rect.contains(p + Vector::new(bounds.x, bounds.y)));
                                let color = if over { c.text } else { c.muted };
                                let svg = adv_svg::Svg::new(adv_svg::Handle::from_memory(icon)).color(color);
                                r.draw_svg(svg, pad(rect, -2.0), bounds);
                            }
                        }
                        Item::Text(t) => {
                            let o = Vector::new(origin.x + t.pos.x, origin.y + t.pos.y);
                            for (rect, color, border) in &t.under {
                                quad(r, *rect + o, *color, *border);
                            }
                            if !sel.is_empty() {
                                let laid = v.to_laid(sel.start)..v.to_laid(sel.end);
                                if let Some(src) = t.source_range()
                                    && laid.start <= src.end
                                    && laid.end >= src.start
                                {
                                    for tr in t.text_ranges(laid) {
                                        for rect in t.range_rects(tr.start, tr.end) {
                                            quad(r, rect + o, c.selection, Border::default());
                                        }
                                    }
                                }
                            }
                            r.fill_paragraph(&t.para, Point::new(o.x, o.y), t.color, bounds);
                            for (rect, color) in &t.over {
                                quad(r, *rect + o, *color, Border::default());
                            }
                        }
                    }
                }
            }
        });
        // Table toolbars float over the text, so they need their own layer.
        renderer.with_layer(bounds, |r| {
            for v in &st.visible {
                let Some(t) = &v.layout.table else { continue };
                let Some(first) = t.toolbar.first() else { continue };
                let last = t.toolbar.last().map_or(first.0, |x| x.0);
                let origin = Vector::new(bounds.x + st.col_x, bounds.y + v.y);
                let frame = Rectangle::new(
                    Point::new(first.0.x - 2.0, first.0.y - 2.0),
                    Size::new(last.x + last.width - first.0.x + 4.0, first.0.height + 4.0),
                );
                r.fill_quad(
                    renderer::Quad {
                        bounds: frame + origin,
                        border: Border { color: c.border, width: 1.0, radius: space::RADIUS_MD.into() },
                        shadow: Shadow {
                            color: Color { a: if self.look.dark { 0.45 } else { 0.10 }, ..Color::BLACK },
                            offset: Vector::new(0.0, 4.0),
                            blur_radius: 16.0,
                        },
                        ..renderer::Quad::default()
                    },
                    c.surface,
                );
                for (rect, _, label) in &t.toolbar {
                    let rect = *rect + origin;
                    if hover.is_some_and(|p| rect.contains(p + Vector::new(bounds.x, bounds.y))) {
                        quad(r, rect, c.accent_soft, Border { radius: space::RADIUS_SM.into(), ..Border::default() });
                    }
                    r.fill_paragraph(&label.para, label.pos + Vector::new(origin.x, origin.y), label.color, bounds);
                }
            }
            if let Some(f) = &st.focus
                && self.editable()
                && (f.caret_visible() || st.preedit.is_some())
                && let Some(caret) = self.caret_rect(st, selection.head)
            {
                quad(r, caret + Vector::new(bounds.x, bounds.y), c.accent, Border::default());
            }
            if let Some(thumb) = self.thumb(st) {
                let color = Color { a: 0.5, ..c.border_strong };
                quad(
                    r,
                    thumb + Vector::new(bounds.x, bounds.y),
                    color,
                    Border { radius: 3.0.into(), ..Border::default() },
                );
            }
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let st = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let Some(p) = cursor.position_in(bounds) else { return mouse::Interaction::default() };
        if matches!(st.drag, Some(Drag::Scrollbar { .. })) || p.x >= st.size.width - 12.0 && self.thumb(st).is_some() {
            return mouse::Interaction::default();
        }
        let editable = self.editable();
        for v in &st.visible {
            let local = Point::new(p.x - st.col_x, p.y - v.y);
            if let Some(t) = &v.layout.table
                && t.toolbar.iter().any(|(r, _, _)| r.contains(local))
            {
                return mouse::Interaction::Pointer;
            }
            if local.y < 0.0 || local.y > v.layout.height {
                continue;
            }
            for item in &v.layout.items {
                match item {
                    Item::Check { rect, .. } if editable && pad(*rect, 4.0).contains(local) => {
                        return mouse::Interaction::Pointer;
                    }
                    Item::Copy { rect, .. } if rect.contains(local) => return mouse::Interaction::Pointer,
                    Item::Text(t)
                        if !t.links.is_empty()
                            && (!editable || st.modifiers.command())
                            && t.bounds().contains(local) =>
                    {
                        let off = t.hit(Point::new(local.x - t.pos.x, local.y - t.pos.y));
                        if t.link_at(off).is_some() {
                            return mouse::Interaction::Pointer;
                        }
                    }
                    _ => {}
                }
            }
        }
        if editable { mouse::Interaction::Text } else { mouse::Interaction::default() }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn adv_widget::Operation,
    ) {
        let st = tree.state.downcast_mut::<State>();
        operation.focusable(self.id.as_ref(), layout.bounds(), st);
    }
}

impl<'a, Message: 'a> From<View<'a, Message>> for Element<'a, Message> {
    fn from(view: View<'a, Message>) -> Self {
        Element::new(view)
    }
}

/// Keys a read-only view still handles: copy and select all.
fn read_only_key(key: &Key, modifiers: keyboard::Modifiers) -> bool {
    modifiers.command()
        && !modifiers.alt()
        && matches!(key.as_ref(), Key::Character(c) if c.eq_ignore_ascii_case("c") || c.eq_ignore_ascii_case("a"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdedit_core::RopeBuffer;

    fn doc(text: &str) -> Document {
        Document::with_buffer(Box::new(RopeBuffer::from_text(text)), true)
    }

    fn prepared(doc: &Document, state: &ViewState) -> State {
        let theme = Theme::ember_light();
        let mut view = View::new(doc, state, &theme, |a| a);
        let mut st = State { size: Size::new(1000.0, 800.0), ..State::default() };
        st.width = 720.0;
        st.wide = 960.0;
        st.col_x = 140.0;
        let _ = &mut view;
        view.prepare(&mut st);
        st
    }

    #[test]
    fn lays_out_the_visible_blocks() {
        let text = include_str!("../../mdedit-export/examples/sample/sample.md");
        let d = doc(text);
        let st = prepared(&d, &ViewState { read_only: true, ..ViewState::default() });
        for v in &st.visible {
            eprintln!("{} {:?} y={} h={} items={}", v.index, v.block.kind, v.y, v.layout.height, v.layout.items.len());
        }
        assert!(st.visible.len() > 3);
        assert_eq!(st.visible[0].y, 0.0);
    }

    fn texts(st: &State, block: usize) -> Vec<String> {
        let v = st.visible.iter().find(|v| v.index == block).expect("visible block");
        v.layout.texts().map(|t| t.text.replace('\u{2060}', "")).collect()
    }

    #[test]
    fn footnotes_show_numbers() {
        let d = doc("One[^b] two[^a].\n\n[^a]: First.\n\n[^b]: Second.\n");
        let st = prepared(&d, &ViewState { read_only: true, ..ViewState::default() });
        assert_eq!(texts(&st, 0), ["One2 two1."]);
        assert_eq!(texts(&st, 1)[0], "1");
        assert_eq!(texts(&st, 2)[0], "2");
    }

    #[test]
    fn footnote_numbers_follow_renamed_definitions() {
        let mut d = doc("See[^x].\n\n[^y]: Y.\n\n[^x]: X.\n");
        let state = ViewState { read_only: true, ..ViewState::default() };
        let theme = Theme::ember_light();
        let view = View::new(&d, &state, &theme, |a| a);
        let mut st = State { size: Size::new(1000.0, 800.0), width: 720.0, wide: 960.0, ..State::default() };
        view.prepare(&mut st);
        assert_eq!(texts(&st, 0), ["See2."]);
        drop(view);
        // Swap the definitions' order: the reference block's text is
        // unchanged but its number is not.
        let len = d.buffer().len_bytes();
        let text = "[^x]: X.\n\n[^y]: Y.\n".to_string();
        d.apply(&Edit { range: 10..len, text, origin: EditOrigin::User, txn: TxnId(0) });
        let view = View::new(&d, &state, &theme, |a| a);
        view.prepare(&mut st);
        assert_eq!(texts(&st, 0), ["See1."]);
    }

    #[test]
    fn github_alerts_are_callouts() {
        let d = doc("> [!WARNING]\n> Mind the gap.\n");
        let st = prepared(&d, &ViewState { read_only: true, ..ViewState::default() });
        assert_eq!(texts(&st, 0), ["WARNING", "Mind the gap."]);
        let look = Look::new(&Theme::ember_light());
        let v = &st.visible[0];
        let first = &v.layout.items[0];
        assert!(
            matches!(first, Item::Quad { color, rect, .. } if *color == look.colors.warning_bg && rect.height > 40.0)
        );
    }

    #[test]
    fn revealed_image_stays_under_its_source() {
        let dir = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(300, 100, image::Rgba([200, 80, 40, 255])).save(dir.path().join("a.png")).unwrap();
        let mut d = doc("Intro.\n\n![pic](a.png)\n");
        d.path = Some(dir.path().join("doc.md"));
        // Caret inside the image markdown: the block shows its source.
        let state = ViewState { selection: Selection::cursor(12), ..ViewState::default() };
        let started = std::time::Instant::now();
        loop {
            let st = prepared(&d, &state);
            let v = &st.visible[1];
            assert!(v.layout.revealed);
            assert_eq!(texts(&st, 1), ["![pic](a.png)"]);
            let text_bottom = v.layout.texts().map(|t| t.bounds().y + t.bounds().height).fold(0.0, f32::max);
            if let Some(rect) = v.layout.items.iter().find_map(|i| match i {
                Item::Image { rect, .. } => Some(*rect),
                _ => None,
            }) {
                assert!(rect.y >= text_bottom && (rect.width - 300.0).abs() < 0.5, "{rect:?}");
                assert!(v.layout.height >= rect.y + rect.height);
                break;
            }
            assert!(started.elapsed().as_secs() < 10, "image never loaded");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn checkbox_click_toggles_the_task() {
        let text = "# T\n\n- [ ] one\n- [x] two\n";
        let d = doc(text);
        let state = ViewState::default();
        let st = prepared(&d, &state);
        let theme = Theme::ember_light();
        let view = View::new(&d, &state, &theme, |a| a);
        let v = &st.visible[1];
        let rect = v
            .layout
            .items
            .iter()
            .find_map(|i| match i {
                Item::Check { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("a checkbox");
        let p = Point::new(st.col_x + rect.center_x(), v.y + rect.center_y());
        let actions = view.click_controls(&st, p, &mut iced::advanced::clipboard::Null).expect("handled");
        assert!(matches!(&actions[0], Action::Edit(e) if e.range == (7..10) && e.text == "[x]"), "{actions:?}");
    }

    #[test]
    fn focus_operation_focuses_the_view() {
        use operation::Focusable;
        let mut st = State::default();
        assert!(!st.is_focused());
        st.focus();
        assert!(st.is_focused());
        st.unfocus();
        assert!(!st.is_focused());
    }
}
