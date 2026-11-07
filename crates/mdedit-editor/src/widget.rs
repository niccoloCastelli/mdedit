//! The Edit-mode widget itself: layout of the visible lines, drawing, and
//! input handling. See the crate docs for the contract with the app.

use std::cell::Cell;
use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::input_method::{self, InputMethod};
use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{Alignment, LineHeight, Paragraph as _, Renderer as _, Shaping, Text, Wrapping};
use iced::advanced::widget::{self, Tree, operation, tree};
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget, mouse, renderer};
use iced::keyboard::{self, key};
use iced::{Border, Element, Event, Length, Pixels, Point, Rectangle, Size, alignment, window};
use mdedit_core::highlight::{LineState, StyledRange, highlight_line, highlight_lines};
use mdedit_core::io::LineEnding;
use mdedit_core::parse::BlockKind;
use mdedit_core::{AppliedEdit, Document, Edit, EditOrigin, Selection, TextBuffer, TxnId};
use mdedit_theme::{Theme, space};

use crate::cache::{LineCache, ShapedLine};
use crate::geometry;
use crate::height_map::HeightMap;
use crate::motion;
use crate::style::{self, LineKind, Look};
use crate::{Action, EditorState, HighlightKind, stats};

/// Space above the first line.
const PAD_TOP: f32 = space::SPACE_6;
/// Edits since the last sync above which the height map is rebuilt instead
/// of patched.
const MAX_INCREMENTAL_EDITS: usize = 64;

/// The Edit-mode widget. [`crate::editor`] is the usual way to build it.
pub struct Editor<'a, Message> {
    doc: &'a Document,
    state: &'a EditorState,
    look: Look,
    on_action: Box<dyn Fn(Action) -> Message + 'a>,
    id: Option<widget::Id>,
}

impl<'a, Message> Editor<'a, Message> {
    pub fn new(
        doc: &'a Document,
        state: &'a EditorState,
        theme: &Theme,
        on_action: impl Fn(Action) -> Message + 'a,
    ) -> Self {
        Self {
            doc,
            state,
            look: Look::new(theme, state.monospace, state.zoom),
            on_action: Box::new(on_action),
            id: None,
        }
    }

    /// Widget id, for `iced::widget::operation::focus`.
    pub fn id(mut self, id: impl Into<widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }
}

#[derive(Debug, Clone, Copy)]
struct Focus {
    updated_at: Instant,
    now: Instant,
    window_focused: bool,
}

impl Focus {
    fn now() -> Self {
        let now = Instant::now();
        Self { updated_at: now, now, window_focused: true }
    }

    fn caret_visible(&self) -> bool {
        self.window_focused
            && ((self.now - self.updated_at).as_millis() / space::CURSOR_BLINK_MS as u128).is_multiple_of(2)
    }
}

/// Where things go inside the widget bounds (relative coordinates).
#[derive(Debug, Clone, Copy, Default)]
struct Geom {
    height: f32,
    text_x: f32,
    text_w: f32,
    /// Right edge of the line numbers, when shown.
    numbers_right: Option<f32>,
}

impl Geom {
    fn compute(size: Size, state: &EditorState, look: &Look, lines: usize) -> Self {
        let pad = if size.width >= mdedit_theme::typography::MEASURE + 2.0 * space::SPACE_6 {
            space::SPACE_6
        } else {
            space::SPACE_4
        };
        let gutter = state.line_numbers.then(|| {
            let digits = lines.max(1).ilog10() as f32 + 1.0;
            digits.max(3.0) * look.number.size * 0.6 + space::SPACE_4
        });
        let left_min = pad + gutter.unwrap_or(0.0);
        let mut text_w = (size.width - 2.0 * pad).max(48.0);
        if !state.full_width {
            text_w = text_w.min(mdedit_theme::typography::MEASURE);
        }
        let mut text_x = (size.width - text_w) / 2.0;
        if text_x < left_min {
            text_x = left_min;
            text_w = text_w.min((size.width - left_min - pad).max(48.0));
        }
        Self { height: size.height, text_x, text_w, numbers_right: gutter.map(|_| text_x - space::SPACE_4) }
    }
}

#[derive(Debug, Clone)]
struct VisibleLine {
    line: usize,
    /// Byte offset of the line start.
    start: usize,
    /// Length of the line text (without line ending).
    len: usize,
    /// Top, relative to the widget top.
    y: f32,
    shaped: Rc<ShapedLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragKind {
    Char,
    Word,
    Line,
}

#[derive(Debug, Clone)]
struct Drag {
    kind: DragKind,
    /// What the initial click selected (a point, a word or a line).
    origin: std::ops::Range<usize>,
    /// Last pointer position (absolute), for autoscroll.
    position: Point,
}

/// Widget state kept in the iced tree between frames.
struct State {
    // Document sync.
    doc_id: usize,
    version: u64,
    look_generation: u64,
    heights: HeightMap,
    cache: LineCache,
    // Scroll: the line at the top of the viewport and how far the viewport
    // top is below that line's top. Re-measuring lines above the anchor
    // never moves what is on screen.
    anchor_line: usize,
    anchor_delta: f64,
    /// `EditorState::scroll_y` as last accepted from the app.
    external_scroll: f32,
    /// The last scroll value we published, which the app may echo back.
    echo: Option<f32>,
    report_scroll: bool,
    /// `EditorState::scroll_to_byte` as last seen.
    scroll_to_byte: Option<usize>,
    geom: Geom,
    visible: Vec<VisibleLine>,
    last_head: Option<usize>,
    // Input.
    focus: Option<Focus>,
    last_click: Option<mouse::Click>,
    drag: Option<Drag>,
    goal_x: Option<f32>,
    preedit: Option<input_method::Preedit>,
    /// An `Edit` was published and the app has not rebuilt the view yet.
    pending_edit: bool,
    /// The selection we last published, until the app rebuilds the view.
    local_selection: Option<Selection>,
    queued: VecDeque<Event>,
    /// Shift is held (for shift-click).
    shift: bool,
    /// Ctrl (Cmd on macOS) is held: the wheel zooms (the app handles it)
    /// instead of scrolling.
    command: bool,
    /// The last `SelectionRect` published.
    sel_rect: Option<Rectangle>,
    /// The last `CaretRect` published.
    caret_pub: Option<Rectangle>,
    // Measurements.
    keystroke_started: Cell<Option<(Instant, u64)>>,
    layout_time: Cell<Duration>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            doc_id: 0,
            version: 0,
            look_generation: 0,
            heights: HeightMap::default(),
            cache: LineCache::default(),
            anchor_line: 0,
            anchor_delta: 0.0,
            external_scroll: 0.0,
            echo: None,
            report_scroll: false,
            scroll_to_byte: None,
            geom: Geom::default(),
            visible: Vec::new(),
            last_head: None,
            // The editor is the main thing in its window, so it starts focused.
            focus: Some(Focus::now()),
            last_click: None,
            drag: None,
            goal_x: None,
            preedit: None,
            pending_edit: false,
            local_selection: None,
            queued: VecDeque::new(),
            shift: false,
            command: false,
            sel_rect: None,
            caret_pub: None,
            keystroke_started: Cell::new(None),
            layout_time: Cell::new(Duration::ZERO),
        }
    }
}

impl operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focus.is_some()
    }

    fn focus(&mut self) {
        self.focus = Some(Focus::now());
    }

    fn unfocus(&mut self) {
        self.focus = None;
        self.drag = None;
    }
}

fn doc_id(doc: &Document) -> usize {
    doc.buffer() as *const dyn TextBuffer as *const () as usize
}

/// Line breaks in `s`, counted the way ropey does with `cr_lines`.
fn newlines(s: &str) -> usize {
    let b = s.as_bytes();
    b.iter().enumerate().filter(|&(i, &c)| c == b'\n' || (c == b'\r' && b.get(i + 1) != Some(&b'\n'))).count()
}

/// Byte length of every line (including its line ending), in one pass.
fn line_lengths(buf: &dyn TextBuffer, lines: usize) -> Vec<usize> {
    let mut lens = Vec::with_capacity(lines);
    let mut current = 0usize;
    for chunk in buf.chunks(0..buf.len_bytes()) {
        let mut rest = chunk;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            lens.push(current + i + 1);
            current = 0;
            rest = &rest[i + 1..];
        }
        current += rest.len();
    }
    lens.push(current);
    if lens.len() != lines {
        // Lone CRs count as line breaks too; take the slow path.
        lens = (0..lines).map(|l| motion::line_range_full(buf, l).len()).collect();
    }
    lens
}

/// Whether `line` is code (fenced or indented code block), from the block
/// index (none in large-file mode).
fn line_kind(doc: &Document, line: usize) -> LineKind {
    let Some(blocks) = doc.blocks() else { return LineKind::Prose };
    let start = doc.buffer().line_to_byte(line);
    let Some(block) = blocks.block_at(start).and_then(|i| blocks.get(i)) else { return LineKind::Prose };
    if start > block.bytes.end {
        return LineKind::Prose;
    }
    match block.kind {
        BlockKind::CodeBlock { .. } | BlockKind::Math | BlockKind::Mermaid => LineKind::Code,
        _ => LineKind::Prose,
    }
}

/// Syntax tokens for a run of lines, computed once per layout pass.
#[derive(Debug, Default)]
struct Tokens {
    start: usize,
    lines: Vec<Vec<StyledRange>>,
}

impl Tokens {
    fn compute(doc: &Document, lines: std::ops::Range<usize>) -> Self {
        let buf = doc.buffer();
        let lines = lines.start..lines.end.min(motion::line_count(buf));
        let tokens = match doc.blocks() {
            Some(index) => highlight_lines(index, buf, lines.clone()),
            None => {
                // Large-file mode: line-local tokenizer, state carried from
                // the first line of the window.
                let mut state = LineState::default();
                lines.clone().map(|l| highlight_line(&buf.line(l), &mut state)).collect()
            }
        };
        Self { start: lines.start, lines: tokens }
    }

    fn get(&self, line: usize) -> Option<&[StyledRange]> {
        self.lines.get(line.checked_sub(self.start)?).map(Vec::as_slice)
    }
}

fn shape_text(text: &str, tokens: &[StyledRange], kind: LineKind, look: &Look, width: f32) -> ShapedLine {
    let (spans, code_ranges) = style::spans(text, tokens, kind, look);
    let metrics = look.metrics(kind);
    let paragraph = Paragraph::with_spans(Text {
        content: spans.as_slice(),
        bounds: Size::new(width, f32::INFINITY),
        size: Pixels(metrics.size),
        line_height: LineHeight::Absolute(Pixels(metrics.line_height)),
        font: look.base_font(kind),
        align_x: Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::WordOrGlyph,
    });
    let height = paragraph.min_bounds().height.max(metrics.line_height);
    ShapedLine { paragraph, height, kind, code_ranges }
}

/// The shaped paragraph of `line`, from the cache when possible. Tokens
/// come from `window` when it covers the line.
fn shape(
    cache: &mut LineCache,
    doc: &Document,
    look: &Look,
    width: f32,
    line: usize,
    window: Option<&Tokens>,
) -> Rc<ShapedLine> {
    let text = doc.buffer().line(line);
    let single;
    let tokens = match window.and_then(|w| w.get(line)) {
        Some(t) => t,
        None => {
            single = Tokens::compute(doc, line..line + 1);
            single.get(line).unwrap_or(&[])
        }
    };
    let kind = line_kind(doc, line);
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    for t in tokens {
        (t.range.start, t.range.end, t.token).hash(&mut h);
    }
    kind.hash(&mut h);
    width.to_bits().hash(&mut h);
    look.generation.hash(&mut h);
    cache.get_or_shape(h.finish(), || shape_text(&text, tokens, kind, look, width))
}

impl State {
    fn scroll(&self) -> f64 {
        PAD_TOP as f64 + self.heights.offset_of(self.anchor_line) + self.anchor_delta
    }

    fn max_scroll(&self) -> f64 {
        let bottom = (self.geom.height * 0.4) as f64;
        (PAD_TOP as f64 + self.heights.total_height() + bottom - self.geom.height as f64).max(0.0)
    }

    fn set_scroll(&mut self, y: f64) {
        let y = y.clamp(0.0, self.max_scroll());
        let (line, top) = self.heights.line_at(y - PAD_TOP as f64);
        self.anchor_line = line;
        self.anchor_delta = y - PAD_TOP as f64 - top;
    }

    fn scroll_by(&mut self, dy: f64) {
        self.set_scroll(self.scroll() + dy);
        self.report_scroll = true;
    }

    /// Bring the height map in step with the document.
    fn sync(&mut self, doc: &Document, look: &Look) {
        let buf = doc.buffer();
        let Some(lines) = buf.len_lines() else {
            // Large file still indexing: show nothing until the count is known.
            self.heights = HeightMap::new([look.metrics(LineKind::Prose).line_height]);
            self.version = u64::MAX;
            return;
        };
        let id = doc_id(doc);
        let same_doc = id == self.doc_id && look.generation == self.look_generation;
        if same_doc && buf.version() == self.version && self.heights.len() == lines {
            return;
        }
        if same_doc && buf.version() > self.version && self.version != u64::MAX {
            let edits: Vec<&AppliedEdit> = doc.log().since(self.version).map(Iterator::collect).unwrap_or_default();
            let contiguous = edits.first().is_some_and(|e| e.version == self.version + 1)
                && edits.last().is_some_and(|e| e.version == buf.version());
            if contiguous && edits.len() <= MAX_INCREMENTAL_EDITS {
                self.apply_edits(buf, &edits, look);
                self.version = buf.version();
                if self.heights.len() == lines {
                    return;
                }
            }
        }
        self.rebuild(doc, look, id, lines);
    }

    fn rebuild(&mut self, doc: &Document, look: &Look, id: usize, lines: usize) {
        let buf = doc.buffer();
        let width = self.geom.text_w.max(48.0);
        let lens = line_lengths(buf, lines);
        self.heights = HeightMap::new(lens.into_iter().map(|len| look.estimate(len, width)));
        if id != self.doc_id {
            self.anchor_line = 0;
            self.anchor_delta = 0.0;
            self.last_head = None;
        }
        self.anchor_line = self.anchor_line.min(lines - 1);
        self.doc_id = id;
        self.version = buf.version();
        self.look_generation = look.generation;
    }

    /// Patch the height map for `edits` (oldest first, already applied to
    /// `buf`): the touched lines become estimates, added or removed lines
    /// are spliced in or out.
    fn apply_edits(&mut self, buf: &dyn TextBuffer, edits: &[&AppliedEdit], look: &Look) {
        let line_height = look.metrics(LineKind::Prose).line_height;
        for (i, e) in edits.iter().enumerate() {
            // Line of the edit start in the text right after edit `i`: map the
            // offset to the current text, then undo the line count changes
            // that later edits made before it.
            let mut offset = e.edit.range.start;
            let mut correction: isize = 0;
            for later in &edits[i + 1..] {
                let r = &later.edit.range;
                if r.end < offset || (r.end == offset && r.start < r.end) {
                    correction += newlines(&later.edit.text) as isize - newlines(&later.inverse.text) as isize;
                }
                offset = later.map_offset(offset, false);
            }
            let line = (buf.byte_to_line(offset.min(buf.len_bytes())) as isize - correction).max(0) as usize;
            let removed = newlines(&e.inverse.text);
            let added = newlines(&e.edit.text);
            let end = (line + removed + 1).min(self.heights.len());
            self.heights.splice(line..end, std::iter::repeat_n(line_height, added + 1));
            if self.anchor_line > line + removed {
                self.anchor_line = self.anchor_line + added - removed;
            } else if self.anchor_line > line {
                self.anchor_line = line + (self.anchor_line - line).min(added);
            }
        }
        self.anchor_line = self.anchor_line.min(self.heights.len().saturating_sub(1));
    }

    /// Apply a scroll offset set by the app (session restore, Split sync).
    fn accept_external_scroll(&mut self, scroll_y: f32) {
        if scroll_y == self.external_scroll {
            return;
        }
        self.external_scroll = scroll_y;
        if self.echo.take() == Some(scroll_y) {
            return;
        }
        self.set_scroll(scroll_y as f64);
    }

    /// Source position at the top of the viewport: the start byte of the
    /// top line and how far into its height the viewport top is.
    fn top_anchor(&self, buf: &dyn TextBuffer) -> (usize, f32) {
        let y = self.scroll() - PAD_TOP as f64;
        if y <= 0.0 || self.heights.is_empty() {
            return (0, 0.0);
        }
        let (line, top) = self.heights.line_at(y);
        let h = self.heights.height(line).max(1.0) as f64;
        let line = line.min(motion::line_count(buf) - 1);
        (buf.line_to_byte(line), ((y - top) / h).clamp(0.0, 1.0) as f32)
    }

    /// Put the line holding `byte` at the top of the viewport, when the app
    /// sets a new `scroll_to_byte`.
    fn accept_scroll_to_byte(&mut self, buf: &dyn TextBuffer, target: Option<usize>) {
        if target == self.scroll_to_byte {
            return;
        }
        self.scroll_to_byte = target;
        if let Some(byte) = target {
            let line = buf.byte_to_line(byte.min(buf.len_bytes())).min(self.heights.len().saturating_sub(1));
            self.set_scroll(PAD_TOP as f64 + self.heights.offset_of(line));
        }
    }

    /// Scroll so the caret at `head` is on screen.
    fn follow(&mut self, doc: &Document, look: &Look, head: usize) {
        let buf = doc.buffer();
        let (line, col) = motion::line_col(buf, head);
        let shaped = shape(&mut self.cache, doc, look, self.geom.text_w, line, None);
        self.heights.set_measured(line, shaped.height);
        let (_, row) = geometry::caret(&shaped.paragraph, col, look.metrics(shaped.kind).line_height);
        let top = PAD_TOP as f64 + self.heights.offset_of(line) + row.top as f64;
        let bottom = top + row.height as f64;
        let view = self.geom.height as f64;
        let margin = (row.height as f64).min(view / 4.0);
        let scroll = self.scroll();
        if top < scroll + margin.min(top) {
            self.set_scroll(top - margin);
            self.report_scroll = true;
        } else if bottom > scroll + view - margin {
            self.set_scroll(bottom + margin - view);
            self.report_scroll = true;
        }
    }

    /// Shape and position the lines in the viewport plus one screen of
    /// overscan on each side.
    fn prepare(&mut self, doc: &Document, look: &Look) {
        let buf = doc.buffer();
        let n = self.heights.len();
        let view = self.geom.height;
        let over = view.max(200.0);
        let width = self.geom.text_w;
        // Tokens for the lines the estimate says will be laid out (a few
        // extra in case measured heights come out smaller).
        let scroll = self.scroll().clamp(0.0, self.max_scroll()) - PAD_TOP as f64;
        let first = self.heights.line_at(scroll - over as f64).0.min(self.anchor_line);
        let last = self.heights.line_at(scroll + (view + over) as f64).0.max(self.anchor_line);
        let tokens = Tokens::compute(doc, first.saturating_sub(8)..last + 16);
        for _ in 0..2 {
            let scroll = self.scroll();
            let clamped = scroll.clamp(0.0, self.max_scroll());
            if clamped != scroll {
                self.set_scroll(clamped);
            }
            self.anchor_line = self.anchor_line.min(n - 1);
            self.visible.clear();
            let anchor_y = (-self.anchor_delta) as f32;

            let mut y = anchor_y;
            let mut line = self.anchor_line;
            while line < n && y < view + over {
                let shaped = shape(&mut self.cache, doc, look, width, line, Some(&tokens));
                self.heights.set_measured(line, shaped.height);
                let start = buf.line_to_byte(line);
                let len = buf.line(line).len();
                let h = shaped.height;
                self.visible.push(VisibleLine { line, start, len, y, shaped });
                y += h;
                line += 1;
            }

            let mut above = Vec::new();
            let mut y = anchor_y;
            let mut line = self.anchor_line;
            while line > 0 && y > -over {
                line -= 1;
                let shaped = shape(&mut self.cache, doc, look, width, line, Some(&tokens));
                self.heights.set_measured(line, shaped.height);
                y -= shaped.height;
                let start = buf.line_to_byte(line);
                let len = buf.line(line).len();
                above.push(VisibleLine { line, start, len, y, shaped });
            }
            above.reverse();
            self.visible.splice(0..0, above);

            let scroll = self.scroll();
            if (0.0..=self.max_scroll()).contains(&scroll) {
                break;
            }
        }
    }

    /// Document offset under an absolute point, clamped to the shaped lines.
    fn hit(&self, bounds: Rectangle, p: Point) -> Option<usize> {
        let first = self.visible.first()?;
        let last = self.visible.last()?;
        let y = p.y - bounds.y;
        let v = self.visible.iter().find(|v| y >= v.y && y < v.y + v.shaped.height).unwrap_or(if y < first.y {
            first
        } else {
            last
        });
        let x = p.x - bounds.x - self.geom.text_x;
        let local_y = (y - v.y).clamp(0.0, v.shaped.height - 0.5);
        Some(v.start + geometry::hit(&v.shaped.paragraph, x, local_y, v.len))
    }

    fn visible_line(&self, line: usize) -> Option<&VisibleLine> {
        let i = self.visible.binary_search_by_key(&line, |v| v.line).ok()?;
        self.visible.get(i)
    }

    /// Caret rectangle relative to the widget, if its line is shaped.
    fn caret_rect(&self, buf: &dyn TextBuffer, look: &Look, head: usize) -> Option<Rectangle> {
        let (line, col) = motion::line_col(buf, head);
        let v = self.visible_line(line)?;
        let (x, row) = geometry::caret(&v.shaped.paragraph, col, look.metrics(v.shaped.kind).line_height);
        Some(Rectangle::new(
            Point::new(self.geom.text_x + x - space::CURSOR_WIDTH / 2.0, v.y + row.top),
            Size::new(space::CURSOR_WIDTH, row.height),
        ))
    }
}

impl<Message> Editor<'_, Message> {
    /// Window-space bounding box of the visible selection, clipped to the
    /// widget (the same rectangles `draw` fills).
    fn selection_rect(&self, st: &State, bounds: Rectangle) -> Option<Rectangle> {
        let sel = self.selection(st).range();
        if sel.is_empty() || st.focus.is_none() {
            return None;
        }
        let lines = motion::line_count(self.doc.buffer());
        let newline_w = self.look.metrics(LineKind::Prose).size * 0.4;
        let mut out: Option<Rectangle> = None;
        for v in &st.visible {
            let line_end = v.start + v.len;
            if v.y + v.shaped.height < 0.0 || v.y > bounds.height || sel.start > line_end || sel.end <= v.start {
                continue;
            }
            let s = sel.start.max(v.start) - v.start;
            let e = sel.end.min(line_end) - v.start;
            let newline = sel.end > line_end && v.line + 1 < lines;
            for (x, top, w, rh) in geometry::selection_rects(&v.shaped.paragraph, s, e, newline, newline_w) {
                let r =
                    Rectangle::new(Point::new(bounds.x + st.geom.text_x + x, bounds.y + v.y + top), Size::new(w, rh));
                out = Some(out.map_or(r, |o| o.union(&r)));
            }
        }
        out.and_then(|r| r.intersection(&bounds))
    }

    fn selection(&self, st: &State) -> Selection {
        st.local_selection.unwrap_or(self.state.selection)
    }

    fn line_ending(&self) -> &'static str {
        match self.doc.format.line_ending {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }

    fn emit(&self, st: &mut State, shell: &mut Shell<'_, Message>, actions: Vec<Action>) {
        for action in actions {
            match &action {
                Action::Edit(_) => {
                    st.pending_edit = true;
                    st.goal_x = None;
                    if stats::enabled() && st.keystroke_started.get().is_none() {
                        st.keystroke_started.set(Some((Instant::now(), self.doc.buffer().version() + 1)));
                    }
                }
                Action::Select(s) => st.local_selection = Some(*s),
                Action::Scroll(y) => st.echo = Some(*y),
                _ => {}
            }
            let scrolled = matches!(action, Action::Scroll(_));
            shell.publish((self.on_action)(action));
            if scrolled {
                let (byte, fraction) = st.top_anchor(self.doc.buffer());
                shell.publish((self.on_action)(Action::TopAnchor { byte, fraction }));
            }
        }
    }

    fn replace(&self, sel: Selection, text: &str) -> Vec<Action> {
        let range = sel.range();
        let start = range.start;
        vec![
            Action::Edit(Edit { range, text: text.to_owned(), origin: EditOrigin::User, txn: TxnId(0) }),
            Action::Select(Selection::cursor(start + text.len())),
        ]
    }

    fn delete(&self, range: std::ops::Range<usize>) -> Vec<Action> {
        if range.is_empty() {
            return Vec::new();
        }
        let start = range.start;
        vec![Action::Edit(Edit::delete(range, EditOrigin::User, TxnId(0))), Action::Select(Selection::cursor(start))]
    }

    fn shape(&self, st: &mut State, line: usize) -> Rc<ShapedLine> {
        shape(&mut st.cache, self.doc, &self.look, st.geom.text_w, line, None)
    }

    /// Move `rows` visual rows up (negative) or down from `head`, keeping
    /// the goal column.
    fn vertical(&self, st: &mut State, head: usize, rows: isize) -> usize {
        let buf = self.doc.buffer();
        let n = motion::line_count(buf);
        let lh = |s: &ShapedLine| self.look.metrics(s.kind).line_height;
        let (mut line, col) = motion::line_col(buf, head);
        let mut shaped = self.shape(st, line);
        let mut layout = geometry::rows(&shaped.paragraph, lh(&shaped));
        let mut row = geometry::row_of(&layout, col);
        let x = st.goal_x.unwrap_or_else(|| geometry::caret(&shaped.paragraph, col, lh(&shaped)).0);
        st.goal_x = Some(x);
        let mut remaining = rows;
        while remaining != 0 {
            if remaining < 0 {
                if row > 0 {
                    row -= 1;
                } else if line > 0 {
                    line -= 1;
                    shaped = self.shape(st, line);
                    layout = geometry::rows(&shaped.paragraph, lh(&shaped));
                    row = layout.len() - 1;
                } else {
                    return 0;
                }
                remaining += 1;
            } else {
                if row + 1 < layout.len() {
                    row += 1;
                } else if line + 1 < n {
                    line += 1;
                    shaped = self.shape(st, line);
                    layout = geometry::rows(&shaped.paragraph, lh(&shaped));
                    row = 0;
                } else {
                    return buf.len_bytes();
                }
                remaining -= 1;
            }
        }
        let len = buf.line(line).len();
        buf.line_to_byte(line) + geometry::index_at_x(&shaped.paragraph, &layout, row, x, len)
    }

    /// Start or end of the visual row holding `head`; pressing again at a
    /// wrapped row's start goes to the logical line start.
    fn row_edge(&self, st: &mut State, head: usize, end: bool) -> usize {
        let buf = self.doc.buffer();
        let (line, col) = motion::line_col(buf, head);
        let base = buf.line_to_byte(line);
        let text = buf.line(line);
        let shaped = self.shape(st, line);
        let rows = geometry::rows(&shaped.paragraph, self.look.metrics(shaped.kind).line_height);
        let r = geometry::row_of(&rows, col);
        if end {
            let row_end = rows[r].end.min(text.len());
            if r + 1 == rows.len() {
                return base + text.len();
            }
            // Stay before the space the row wrapped at.
            let trimmed = text[..row_end].trim_end_matches(' ').len().max(rows[r].start);
            base + if trimmed < row_end { trimmed } else { row_end }
        } else if col == rows[r].start && r > 0 {
            base
        } else {
            base + rows[r].start
        }
    }

    /// Handle a key press; `None` means "not ours" (left uncaptured).
    fn key(
        &self,
        st: &mut State,
        key: &keyboard::Key,
        physical: key::Physical,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
    ) -> Option<Vec<Action>> {
        let buf = self.doc.buffer();
        let sel = self.selection(st);
        let head = sel.head;
        let shift = modifiers.shift();
        let to = |pos: usize| {
            vec![Action::Select(if shift {
                Selection { anchor: sel.anchor, head: pos }
            } else {
                Selection::cursor(pos)
            })]
        };

        if modifiers.command() && !modifiers.alt() {
            return match key.to_latin(physical) {
                Some('c') if !shift => {
                    (!sel.is_empty()).then(|| vec![Action::Copy(buf.slice(sel.range()).into_owned())]).or(Some(vec![]))
                }
                Some('x') if !shift => {
                    let mut actions = Vec::new();
                    if !sel.is_empty() {
                        actions.push(Action::Copy(buf.slice(sel.range()).into_owned()));
                        actions.extend(self.delete(sel.range()));
                    }
                    Some(actions)
                }
                Some('v') if !shift => Some(vec![Action::Paste]),
                Some('a') if !shift => Some(vec![Action::Select(Selection { anchor: 0, head: buf.len_bytes() })]),
                Some('z') if shift => Some(vec![Action::Redo]),
                Some('z') => Some(vec![Action::Undo]),
                Some('y') if !shift => Some(vec![Action::Redo]),
                _ => self.named_key(st, key, modifiers, sel, &to),
            };
        }
        if let Some(actions) = self.named_key(st, key, modifiers, sel, &to) {
            return Some(actions);
        }
        let text = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))?;
        let _ = head;
        Some(self.replace(sel, text))
    }

    fn named_key(
        &self,
        st: &mut State,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
        sel: Selection,
        to: &dyn Fn(usize) -> Vec<Action>,
    ) -> Option<Vec<Action>> {
        let keyboard::Key::Named(named) = key.as_ref() else { return None };
        let buf = self.doc.buffer();
        let head = sel.head;
        let shift = modifiers.shift();
        let jump = modifiers.jump();
        let line_cmd = modifiers.macos_command();
        let page_rows = ((st.geom.height / self.look.metrics(LineKind::Prose).line_height).floor() as isize - 1).max(1);
        Some(match named {
            key::Named::Enter if !modifiers.command() => self.replace(sel, self.line_ending()),
            key::Named::Tab if modifiers.is_empty() => self.replace(sel, "\t"),
            key::Named::Backspace => {
                if !sel.is_empty() {
                    self.delete(sel.range())
                } else if jump {
                    self.delete(motion::prev_word(buf, head)..head)
                } else if line_cmd {
                    self.delete(buf.line_to_byte(buf.byte_to_line(head))..head)
                } else {
                    self.delete(motion::prev_grapheme(buf, head)..head)
                }
            }
            key::Named::Delete => {
                if !sel.is_empty() {
                    self.delete(sel.range())
                } else if jump {
                    self.delete(head..motion::next_word(buf, head))
                } else {
                    self.delete(head..motion::next_grapheme(buf, head))
                }
            }
            key::Named::ArrowLeft => {
                st.goal_x = None;
                if !sel.is_empty() && !shift {
                    to(sel.range().start)
                } else if line_cmd {
                    to(self.row_edge(st, head, false))
                } else if jump {
                    to(motion::prev_word(buf, head))
                } else {
                    to(motion::prev_grapheme(buf, head))
                }
            }
            key::Named::ArrowRight => {
                st.goal_x = None;
                if !sel.is_empty() && !shift {
                    to(sel.range().end)
                } else if line_cmd {
                    to(self.row_edge(st, head, true))
                } else if jump {
                    to(motion::next_word(buf, head))
                } else {
                    to(motion::next_grapheme(buf, head))
                }
            }
            key::Named::ArrowUp if line_cmd => to(0),
            key::Named::ArrowDown if line_cmd => to(buf.len_bytes()),
            key::Named::ArrowUp => to(self.vertical(st, head, -1)),
            key::Named::ArrowDown => to(self.vertical(st, head, 1)),
            key::Named::Home if modifiers.command() => to(0),
            key::Named::End if modifiers.command() => to(buf.len_bytes()),
            key::Named::Home => {
                st.goal_x = None;
                to(self.row_edge(st, head, false))
            }
            key::Named::End => {
                st.goal_x = None;
                to(self.row_edge(st, head, true))
            }
            key::Named::PageUp | key::Named::PageDown => {
                let dir = if named == key::Named::PageUp { -1 } else { 1 };
                let pos = self.vertical(st, head, dir * page_rows);
                let dy = page_rows as f64 * self.look.metrics(LineKind::Prose).line_height as f64;
                st.scroll_by(dir as f64 * dy);
                let mut actions = to(pos);
                actions.push(Action::Scroll(st.scroll() as f32));
                st.report_scroll = false;
                actions
            }
            _ => return None,
        })
    }

    fn selection_for_drag(&self, drag: &Drag, pos: usize) -> Selection {
        let buf = self.doc.buffer();
        let target = match drag.kind {
            DragKind::Char => pos..pos,
            DragKind::Word => motion::word_at(buf, pos),
            DragKind::Line => motion::line_range_full(buf, buf.byte_to_line(pos)),
        };
        if target.start < drag.origin.start {
            Selection { anchor: drag.origin.end, head: target.start }
        } else {
            Selection { anchor: drag.origin.start, head: target.end.max(drag.origin.end) }
        }
    }

    /// Handle one event. Returns true if it was captured.
    fn handle(
        &self,
        st: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        let buf = self.doc.buffer();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(p) = cursor.position_over(bounds) else {
                    if st.focus.is_some() {
                        st.focus = None;
                        st.drag = None;
                        shell.request_redraw();
                    }
                    return false;
                };
                st.focus = Some(Focus::now());
                let Some(pos) = st.hit(bounds, p) else { return true };
                let click = mouse::Click::new(p, mouse::Button::Left, st.last_click);
                st.last_click = Some(click);
                let in_gutter = st.geom.numbers_right.is_some_and(|r| p.x - bounds.x <= r + space::SPACE_2);
                let kind = match click.kind() {
                    _ if in_gutter => DragKind::Line,
                    mouse::click::Kind::Single => DragKind::Char,
                    mouse::click::Kind::Double => DragKind::Word,
                    mouse::click::Kind::Triple => DragKind::Line,
                };
                let sel = self.selection(st);
                let shift = st.shift;
                let selection = if shift && kind == DragKind::Char {
                    Selection { anchor: sel.anchor, head: pos }
                } else {
                    let origin = match kind {
                        DragKind::Char => pos..pos,
                        DragKind::Word => motion::word_at(buf, pos),
                        DragKind::Line => motion::line_range_full(buf, buf.byte_to_line(pos)),
                    };
                    Selection { anchor: origin.start, head: origin.end }
                };
                let origin = selection.range();
                st.drag = Some(Drag { kind, origin: if shift { pos..pos } else { origin }, position: p });
                st.goal_x = None;
                self.emit(st, shell, vec![Action::Select(selection)]);
                true
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                st.drag = None;
                false
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let Some(drag) = st.drag.as_mut() else { return false };
                drag.position = *position;
                let clamped = Point::new(position.x, position.y.clamp(bounds.y, bounds.y + bounds.height - 1.0));
                let Some(pos) = st.hit(bounds, clamped) else { return false };
                let drag = st.drag.clone().expect("drag checked above");
                let selection = self.selection_for_drag(&drag, pos);
                if selection != self.selection(st) {
                    self.emit(st, shell, vec![Action::Select(selection)]);
                }
                if position.y < bounds.y || position.y > bounds.y + bounds.height {
                    shell.request_redraw();
                }
                true
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) && !st.command => {
                let lh = self.look.metrics(LineKind::Prose).line_height as f64;
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => -*y as f64 * 3.0 * lh,
                    mouse::ScrollDelta::Pixels { y, .. } => -*y as f64,
                };
                let before = st.scroll();
                st.scroll_by(dy);
                st.report_scroll = false;
                if st.scroll() != before {
                    let y = st.scroll() as f32;
                    self.emit(st, shell, vec![Action::Scroll(y)]);
                    shell.invalidate_layout();
                }
                true
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, text, .. }) => {
                if st.focus.is_none() {
                    return false;
                }
                if st.pending_edit {
                    st.queued.push_back(event.clone());
                    shell.request_redraw();
                    return true;
                }
                let Some(actions) = self.key(st, key, *physical_key, *modifiers, text.as_deref()) else {
                    return false;
                };
                if let Some(f) = &mut st.focus {
                    f.updated_at = Instant::now();
                }
                if !matches!(key.as_ref(), keyboard::Key::Named(key::Named::ArrowUp | key::Named::ArrowDown))
                    && !matches!(key.as_ref(), keyboard::Key::Named(key::Named::PageUp | key::Named::PageDown))
                {
                    st.goal_x = None;
                }
                self.emit(st, shell, actions);
                shell.request_redraw();
                true
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                st.shift = m.shift();
                st.command = m.command();
                false
            }
            Event::InputMethod(ime) => {
                if st.focus.is_none() {
                    return false;
                }
                match ime {
                    input_method::Event::Opened => st.preedit = Some(input_method::Preedit::new()),
                    input_method::Event::Closed => st.preedit = None,
                    input_method::Event::Preedit(content, selection) => {
                        st.preedit = Some(input_method::Preedit {
                            content: content.clone(),
                            selection: selection.clone(),
                            text_size: Some(Pixels(self.look.metrics(LineKind::Prose).size)),
                        });
                    }
                    input_method::Event::Commit(text) => {
                        if st.pending_edit {
                            st.queued.push_back(event.clone());
                        } else {
                            let sel = self.selection(st);
                            let actions = self.replace(sel, text);
                            self.emit(st, shell, actions);
                        }
                    }
                }
                shell.request_redraw();
                true
            }
            _ => false,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Editor<'_, Message> {
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
        // A rebuild means the app has processed the actions we published.
        let st = tree.state.downcast_mut::<State>();
        st.pending_edit = false;
        st.local_selection = None;
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        let started = Instant::now();
        let size = limits.width(Length::Fill).height(Length::Fill).max();
        let st = tree.state.downcast_mut::<State>();
        let lines = self.doc.buffer().len_lines().unwrap_or(1);
        st.geom = Geom::compute(size, self.state, &self.look, lines);
        st.sync(self.doc, &self.look);
        st.accept_external_scroll(self.state.scroll_y);
        st.accept_scroll_to_byte(self.doc.buffer(), self.state.scroll_to_byte);
        let head = self.state.selection.head.min(self.doc.buffer().len_bytes());
        if st.last_head.is_some_and(|h| h != head) {
            st.follow(self.doc, &self.look, head);
        }
        st.last_head = Some(head);
        st.prepare(self.doc, &self.look);
        st.layout_time.set(started.elapsed());
        layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
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
                {
                    f.now = *now;
                    let blink = space::CURSOR_BLINK_MS as u128;
                    let wait = blink - (f.now - f.updated_at).as_millis() % blink;
                    shell.request_redraw_at(*now + Duration::from_millis(wait as u64));
                }
                // Autoscroll while dragging outside the viewport.
                if let Some(drag) = st.drag.clone() {
                    let above = bounds.y - drag.position.y;
                    let below = drag.position.y - (bounds.y + bounds.height);
                    let dist = above.max(below);
                    if dist > 0.0 {
                        let step = (dist as f64 / 4.0).clamp(2.0, 60.0);
                        st.scroll_by(if above > 0.0 { -step } else { step });
                        let edge = if above > 0.0 { bounds.y } else { bounds.y + bounds.height - 1.0 };
                        if let Some(pos) = st.hit(bounds, Point::new(drag.position.x, edge)) {
                            let selection = self.selection_for_drag(&drag, pos);
                            if selection != self.selection(st) {
                                self.emit(st, shell, vec![Action::Select(selection)]);
                            }
                        }
                        shell.invalidate_layout();
                        shell.request_redraw();
                    }
                }
                if std::mem::take(&mut st.report_scroll) {
                    let y = st.scroll() as f32;
                    self.emit(st, shell, vec![Action::Scroll(y)]);
                }
                let ime = match (&st.focus, st.caret_rect(self.doc.buffer(), &self.look, self.selection(st).head)) {
                    (Some(f), Some(rect)) if f.window_focused => InputMethod::Enabled {
                        cursor: rect + iced::Vector::new(bounds.x, bounds.y),
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
                if self.state.report_caret {
                    let caret = st
                        .focus
                        .as_ref()
                        .and(st.caret_rect(self.doc.buffer(), &self.look, self.selection(st).head))
                        .map(|r| r + iced::Vector::new(bounds.x, bounds.y));
                    if caret != st.caret_pub {
                        st.caret_pub = caret;
                        shell.publish((self.on_action)(Action::CaretRect(caret)));
                    }
                } else {
                    st.caret_pub = None;
                }
            }
            _ => {}
        }

        // Replay key presses queued behind an edit the app has now applied.
        while !st.pending_edit
            && let Some(queued) = st.queued.pop_front()
        {
            self.handle(st, &queued, bounds, cursor, shell);
        }
        if !st.queued.is_empty() {
            shell.request_redraw();
        }

        if self.handle(st, event, bounds, cursor, shell) {
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
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let started = Instant::now();
        let st = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let colors = self.look.colors;
        let buf = self.doc.buffer();
        let selection = self.selection(st);
        let sel = selection.range();
        let lines = motion::line_count(buf);
        let quad = |r: &mut iced::Renderer, x: f32, y: f32, w: f32, h: f32, color: iced::Color, border: Border| {
            r.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(Point::new(x, y), Size::new(w, h)),
                    border,
                    ..renderer::Quad::default()
                },
                color,
            );
        };
        let newline_w = self.look.metrics(LineKind::Prose).size * 0.4;

        renderer.with_layer(bounds, |r| {
            quad(r, bounds.x, bounds.y, bounds.width, bounds.height, colors.bg, Border::default());
            for v in &st.visible {
                let h = v.shaped.height;
                if v.y + h < 0.0 || v.y > bounds.height {
                    continue;
                }
                let ox = bounds.x + st.geom.text_x;
                let oy = bounds.y + v.y;
                let line_end = v.start + v.len;
                let p = &v.shaped.paragraph;

                // Search and comment highlights.
                let hl = &self.state.highlights;
                let first = hl.partition_point(|x| x.range.end <= v.start);
                for x in hl[first..].iter().take_while(|x| x.range.start <= line_end) {
                    let (s, e) = (x.range.start.max(v.start) - v.start, x.range.end.min(line_end) - v.start);
                    let (color, border) = match x.kind {
                        HighlightKind::SearchMatch => (colors.warning_bg, Border::default()),
                        HighlightKind::CurrentSearchMatch => {
                            (colors.accent_soft, Border { color: colors.accent, width: 1.0, radius: 2.0.into() })
                        }
                        HighlightKind::Comment | HighlightKind::ActiveComment => (colors.comment, Border::default()),
                    };
                    let active = x.kind == HighlightKind::ActiveComment;
                    for (x, w, top, rh) in geometry::span_rects(p, s, e) {
                        quad(r, ox + x, oy + top, w, rh, color, border);
                        if active {
                            quad(r, ox + x, oy + top + rh - 2.0, w, 2.0, colors.comment_marker, Border::default());
                        }
                    }
                }

                // Inline code background.
                for c in &v.shaped.code_ranges {
                    for (x, w, top, rh) in geometry::span_rects(p, c.start, c.end) {
                        let radius = Border { radius: space::RADIUS_SM.into(), ..Border::default() };
                        quad(r, ox + x - 2.0, oy + top + 2.0, w + 4.0, rh - 4.0, colors.code_bg, radius);
                    }
                }

                // Selection.
                if !sel.is_empty() && sel.start <= line_end && sel.end > v.start {
                    let s = sel.start.max(v.start) - v.start;
                    let e = sel.end.min(line_end) - v.start;
                    let newline = sel.end > line_end && v.line + 1 < lines;
                    for (x, top, w, rh) in geometry::selection_rects(p, s, e, newline, newline_w) {
                        quad(r, ox + x, oy + top, w, rh, colors.selection, Border::default());
                    }
                }

                r.fill_paragraph(p, Point::new(ox, oy), colors.text, bounds);

                if let Some(right) = st.geom.numbers_right {
                    let n = self.look.number;
                    let first_row = self.look.metrics(v.shaped.kind).line_height;
                    let current = buf.byte_to_line(selection.head.min(buf.len_bytes())) == v.line;
                    r.fill_text(
                        Text {
                            content: (v.line + 1).to_string(),
                            bounds: Size::new(right.max(1.0), n.line_height),
                            size: Pixels(n.size),
                            line_height: LineHeight::Absolute(Pixels(n.line_height)),
                            font: self.look.mono_font,
                            align_x: Alignment::Right,
                            align_y: alignment::Vertical::Top,
                            shaping: Shaping::Basic,
                            wrapping: Wrapping::None,
                        },
                        Point::new(bounds.x + right, oy + (first_row - n.line_height) / 2.0),
                        if current { colors.text } else { colors.muted },
                        bounds,
                    );
                }
            }

            if let Some(f) = &st.focus
                && (f.caret_visible() || st.preedit.is_some())
                && let Some(c) = st.caret_rect(buf, &self.look, selection.head)
            {
                quad(r, bounds.x + c.x, bounds.y + c.y, c.width, c.height, colors.accent, Border::default());
            }
        });

        if let Some((t, version)) = st.keystroke_started.get()
            && buf.version() >= version
        {
            stats::record_keystroke(t.elapsed());
            st.keystroke_started.set(None);
        }
        stats::record_frame(st.layout_time.get() + started.elapsed());
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) { mouse::Interaction::Text } else { mouse::Interaction::default() }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let st = tree.state.downcast_mut::<State>();
        operation.focusable(self.id.as_ref(), layout.bounds(), st);
    }
}

impl<'a, Message: 'a> From<Editor<'a, Message>> for Element<'a, Message> {
    fn from(editor: Editor<'a, Message>) -> Self {
        Element::new(editor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdedit_core::RopeBuffer;

    fn doc_of(lines: usize, parse: bool) -> Document {
        let mut text = String::new();
        for i in 0..lines {
            if i % 10 == 0 {
                text.push_str(&format!("## Heading {i}\n"));
            } else {
                text.push_str(
                    "Some *prose* with `code` and a [link](https://example.com) that is long enough to wrap \
                               around once or twice in a 720 px column of body text.\n",
                );
            }
        }
        Document::with_buffer(Box::new(RopeBuffer::from_text(&text)), parse)
    }

    fn laid_out(doc: &Document, state: &EditorState) -> State {
        let look = Look::new(&Theme::ember_light(), state.monospace, 1.0);
        let geom = Geom::compute(Size::new(1000.0, 800.0), state, &look, motion::line_count(doc.buffer()));
        let mut st = State { geom, ..State::default() };
        st.sync(doc, &look);
        st.prepare(doc, &look);
        st
    }

    #[test]
    fn lays_out_only_the_viewport() {
        let doc = doc_of(20_000, false);
        let st = laid_out(&doc, &EditorState::default());
        assert_eq!(st.heights.len(), 20_001);
        // Viewport plus one screen of overscan below (nothing above line 0).
        assert!(st.visible.len() > 10 && st.visible.len() < 200, "{}", st.visible.len());
        assert_eq!(st.visible[0].line, 0);
        assert!(st.visible.windows(2).all(|w| (w[1].y - (w[0].y + w[0].shaped.height)).abs() < 0.01));
    }

    #[test]
    fn scroll_anchor_survives_measurement() {
        let doc = doc_of(5_000, false);
        let look = Look::new(&Theme::ember_light(), false, 1.0);
        let mut st = laid_out(&doc, &EditorState::default());
        st.set_scroll(100_000.0);
        st.prepare(&doc, &look);
        let line = st.anchor_line;
        let top = st.visible.iter().find(|v| v.line == line).map(|v| v.y).unwrap();
        // Measure a block of lines far above: the offset of the anchor
        // changes, what is on screen does not.
        for l in 0..200 {
            st.heights.set_measured(l, 100.0);
        }
        st.prepare(&doc, &look);
        assert_eq!(st.anchor_line, line);
        let top2 = st.visible.iter().find(|v| v.line == line).map(|v| v.y).unwrap();
        assert_eq!(top, top2);
    }

    #[test]
    fn top_anchor_round_trips_through_scroll_to_byte() {
        let doc = doc_of(5_000, false);
        let look = Look::new(&Theme::ember_light(), false, 1.0);
        let buf = doc.buffer();
        let mut st = laid_out(&doc, &EditorState::default());
        assert_eq!(st.top_anchor(buf), (0, 0.0));
        let target = buf.line_to_byte(1234);
        st.accept_scroll_to_byte(buf, Some(target));
        st.prepare(&doc, &look);
        assert_eq!(st.top_anchor(buf), (target, 0.0));
        // Half a line further down: same line, fraction about one half.
        let h = st.heights.height(1234) as f64;
        st.scroll_by(h / 2.0);
        let (byte, fraction) = st.top_anchor(buf);
        assert_eq!(byte, target);
        assert!((fraction - 0.5).abs() < 0.01, "{fraction}");
        // The same value again does not jump back.
        st.accept_scroll_to_byte(buf, Some(target));
        assert_eq!(st.top_anchor(buf).0, target);
        assert!(st.top_anchor(buf).1 > 0.4);
    }

    #[test]
    fn edits_patch_the_height_map() {
        let mut doc = doc_of(3_000, true);
        let look = Look::new(&Theme::ember_light(), false, 1.0);
        let mut st = laid_out(&doc, &EditorState::default());
        let at = doc.buffer().line_to_byte(1500);
        doc.apply(&Edit::insert(at, "a\nb\nc\n", EditOrigin::User, TxnId(0)));
        doc.apply(&Edit::delete(0..doc.buffer().line_to_byte(2), EditOrigin::User, TxnId(0)));
        doc.apply(&Edit::insert(5, "x\r\ny", EditOrigin::User, TxnId(0)));
        st.sync(&doc, &look);
        assert_eq!(st.heights.len(), doc.buffer().len_lines().unwrap());
        assert_eq!(st.version, doc.buffer().version());
    }

    /// Per-keystroke widget work on a ~10 MB document: apply the edit, sync
    /// the height map, follow the caret and lay out the viewport. Run with
    /// `cargo test --release -p mdedit-editor -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_typing_10mb() {
        for parse in [false, true] {
            let mut doc = doc_of(65_000, parse);
            let look = Look::new(&Theme::ember_light(), false, 1.0);
            let state = EditorState::default();
            let mut st = laid_out(&doc, &state);
            let mut pos = doc.buffer().line_to_byte(30_000) + 10;
            st.follow(&doc, &look, pos);
            st.prepare(&doc, &look);
            let mut samples = Vec::new();
            let mut applies = Vec::new();
            for i in 0..400 {
                let text = if i % 40 == 39 { "\n" } else { "x" };
                let applying = Instant::now();
                doc.apply(&Edit::insert(pos, text, EditOrigin::User, TxnId(0)));
                applies.push(applying.elapsed());
                pos += text.len();
                let started = Instant::now();
                st.sync(&doc, &look);
                st.follow(&doc, &look, pos);
                st.prepare(&doc, &look);
                samples.push(started.elapsed());
                if started.elapsed() > Duration::from_millis(2) {
                    eprintln!("slow keystroke {i}: {:?}", started.elapsed());
                }
            }
            let s = stats::Summary::of(&samples).unwrap();
            eprintln!("typing, widget work, {} bytes, parse={parse}: {s}", doc.buffer().len_bytes());
            eprintln!("typing, Document::apply, parse={parse}: {}", stats::Summary::of(&applies).unwrap());
            let mut frames = Vec::new();
            for _ in 0..400 {
                let started = Instant::now();
                st.scroll_by(120.0);
                st.prepare(&doc, &look);
                frames.push(started.elapsed());
                if started.elapsed() > Duration::from_millis(2) {
                    eprintln!("slow frame {}: {:?}", frames.len(), started.elapsed());
                }
            }
            eprintln!("scroll frame layout, parse={parse}: {}", stats::Summary::of(&frames).unwrap());
        }
    }
}
