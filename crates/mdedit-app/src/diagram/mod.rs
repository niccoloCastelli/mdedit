//! Visual editor for Mermaid diagrams, in its own window.
//!
//! The "Edit diagram" button of a rendered diagram (View, Split and Read
//! modes) sends `mdedit_view::Action::EditDiagram` with the block's range;
//! [`App::open_diagram`] opens one window per block. Flowcharts get the
//! canvas ([`canvas`]); other diagram types get their source with a live
//! preview. Edits go through [`Flowchart::apply`], which rewrites only the
//! statements involved, so the user's own formatting survives. Saving puts
//! the new code back between the fences as one undoable edit.
//!
//! Node positions on the canvas are the editor's own: Mermaid has no syntax
//! for them, and the rendered diagram is always laid out automatically.

pub mod canvas;
mod panel;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use iced::widget::text_editor;
use iced::{Point, Size, Task, Vector, window};
use mdedit_core::{Edit, EditOrigin};
use mdedit_mermaid::flowchart::{self, Direction, EdgeStyle, Flowchart, Op, Shape};

use crate::app::{App, Message};
use crate::tab::TabId;
use canvas::{CanvasMsg, Sel};

pub const LABEL_INPUT: &str = "diagram-label";
pub const EDGE_LABEL_INPUT: &str = "diagram-edge-label";

#[derive(Debug, Clone)]
pub enum DiagramMsg {
    Canvas(CanvasMsg),
    Direction(Direction),
    Shape(Shape),
    Label(String),
    EdgeLabel(String),
    EdgeStyle(EdgeStyle),
    /// Back to the canvas after typing a label.
    LabelDone,
    AddNode,
    Delete,
    Undo,
    Redo,
    AutoLayout,
    Fit,
    Code(text_editor::Action),
    Save {
        close: bool,
    },
    CloseRequested,
    Discard,
    KeepEditing,
    Key(Key, Modifiers),
}

/// Where the code sits in the document.
#[derive(Debug, Clone)]
struct Block {
    /// The whole fenced block as last read from (or written to) the doc.
    range: Range<usize>,
    text: String,
    /// The code between the fences, relative to `text`.
    code: Range<usize>,
    /// What precedes the fence on its line (indent, `> `), repeated on
    /// every code line.
    prefix: String,
}

impl Block {
    /// Split a fenced block (`text`, at doc offset `start`) into fences and
    /// code; `prefix` is the text before the fence on its line.
    fn new(text: String, range: Range<usize>, prefix: String) -> (Self, String) {
        let code_start = text.find('\n').map_or(text.len(), |p| p + 1);
        let body = &text[code_start..];
        let trimmed = body.trim_end_matches(['\n', '\r']);
        let last_start = trimmed.rfind('\n').map_or(0, |p| p + 1);
        let last = trimmed[last_start..].trim_start_matches(prefix.trim_end()).trim();
        let fence = text[..code_start].trim_start_matches(prefix.as_str()).trim_start();
        let fence_char = fence.chars().next().unwrap_or('`');
        let closed = last.len() >= 3 && last.chars().all(|c| c == fence_char);
        let code_end = if closed { code_start + last_start } else { text.len() };
        let code = strip_prefix_lines(&text[code_start..code_end], &prefix);
        (Self { range, text, code: code_start..code_end, prefix }, code)
    }

    /// The doc text for `code` between this block's fences.
    fn code_text(&self, code: &str) -> String {
        let mut code = code.to_string();
        if self.code.end < self.text.len() && !code.is_empty() && !code.ends_with('\n') {
            code.push('\n');
        }
        if self.prefix.is_empty() {
            return code;
        }
        code.split_inclusive('\n')
            .map(|l| {
                if l.trim().is_empty() {
                    format!("{}{}", self.prefix.trim_end(), l.trim_start_matches([' ', '\t']))
                } else {
                    format!("{}{l}", self.prefix)
                }
            })
            .collect()
    }
}

fn strip_prefix_lines(code: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return code.to_string();
    }
    code.split_inclusive('\n')
        .map(|l| l.strip_prefix(prefix).or_else(|| l.strip_prefix(prefix.trim_end())).unwrap_or(l))
        .collect()
}

pub struct Editor {
    pub window: window::Id,
    pub tab: TabId,
    block: Block,
    /// Code as in the document.
    saved: String,
    pub source: String,
    /// `None` for diagram types without a canvas (source and preview only).
    pub model: Option<Flowchart>,
    undo: Vec<String>,
    redo: Vec<String>,
    /// Consecutive edits with the same key (typing a label) are one undo
    /// step.
    coalesce: Option<String>,
    pub positions: HashMap<String, Point>,
    pub sizes: HashMap<String, Size>,
    pub selection: Sel,
    pub label: String,
    pub edge_label: String,
    pub code: text_editor::Content,
    pub pan: Vector,
    pub zoom: f32,
    canvas_size: Size,
    fitted: bool,
    pub confirm_close: bool,
    pub error: Option<String>,
    pub style: Arc<mdedit_mermaid::Style>,
    pub doc_title: String,
}

impl Editor {
    fn new(window: window::Id, tab: TabId, block: Block, code: String, style: Arc<mdedit_mermaid::Style>) -> Self {
        let mut ed = Self {
            window,
            tab,
            block,
            saved: code.clone(),
            source: code.clone(),
            model: None,
            undo: Vec::new(),
            redo: Vec::new(),
            coalesce: None,
            positions: HashMap::new(),
            sizes: HashMap::new(),
            selection: Sel::None,
            label: String::new(),
            edge_label: String::new(),
            code: text_editor::Content::with_text(&code),
            pan: Vector::ZERO,
            zoom: 1.0,
            canvas_size: Size::ZERO,
            fitted: false,
            confirm_close: false,
            error: None,
            style,
            doc_title: String::new(),
        };
        ed.reparse();
        ed
    }

    pub fn dirty(&self) -> bool {
        self.source != self.saved
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Parse the source again; keep positions of known nodes, place new
    /// ones where the renderer would, shifted to line up with the rest.
    fn reparse(&mut self) {
        self.model = if flowchart::is_flowchart(&self.source) { Flowchart::parse(&self.source).ok() } else { None };
        let Some(model) = &self.model else { return };
        let boxes = mdedit_mermaid::node_boxes(&self.source, &self.style).unwrap_or_default();
        let center = |b: &mdedit_mermaid::NodeBox| Point::new(b.x + b.width / 2.0, b.y + b.height / 2.0);
        // Offset between the renderer's layout and ours, from a node both know.
        let shift = model
            .nodes
            .iter()
            .find_map(|n| Some(*self.positions.get(&n.id)? - center(boxes.get(&n.id)?)))
            .unwrap_or(Vector::ZERO);
        let mut next = self.positions.values().fold(0.0f32, |m, p| m.max(p.y)) + 80.0;
        for n in &model.nodes {
            let size = boxes.get(&n.id).map_or_else(
                || Size::new((n.label.chars().count() as f32 * 7.5 + 32.0).max(60.0), 44.0),
                |b| Size::new(b.width, b.height),
            );
            self.sizes.insert(n.id.clone(), size);
            if !self.positions.contains_key(&n.id) {
                let p = boxes.get(&n.id).map(|b| center(b) + shift).unwrap_or_else(|| {
                    next += 80.0;
                    Point::new(0.0, next)
                });
                self.positions.insert(n.id.clone(), p);
            }
        }
        match &self.selection {
            Sel::Node(id) if model.node(id).is_none() => self.selection = Sel::None,
            Sel::Edge(i) if *i >= model.edges.len() => self.selection = Sel::None,
            _ => {}
        }
        self.sync_inputs();
    }

    /// Fill the label inputs from the selection.
    fn sync_inputs(&mut self) {
        let Some(model) = &self.model else { return };
        match &self.selection {
            Sel::Node(id) => self.label = model.node(id).map(|n| n.label.clone()).unwrap_or_default(),
            Sel::Edge(i) => self.edge_label = model.edges[*i].label.clone().unwrap_or_default(),
            Sel::None => {}
        }
    }

    fn set_source(&mut self, src: String, coalesce: Option<String>, sync_code: bool) {
        if src == self.source {
            return;
        }
        if coalesce.is_none() || coalesce != self.coalesce {
            self.undo.push(std::mem::take(&mut self.source));
        }
        self.redo.clear();
        self.coalesce = coalesce;
        self.source = src;
        if sync_code {
            self.code = text_editor::Content::with_text(&self.source);
        }
        self.error = None;
        self.reparse_keep_label();
    }

    /// Reparse without overwriting the label being typed.
    fn reparse_keep_label(&mut self) {
        let (label, edge_label) = (self.label.clone(), self.edge_label.clone());
        self.reparse();
        if self.coalesce.as_deref().is_some_and(|c| c.starts_with("label:")) {
            self.label = label;
        }
        if self.coalesce.as_deref().is_some_and(|c| c.starts_with("edge-label:")) {
            self.edge_label = edge_label;
        }
    }

    fn apply(&mut self, op: &Op, coalesce: Option<String>) {
        let Some(model) = &self.model else { return };
        match model.apply(op) {
            Ok(src) => self.set_source(src, coalesce, true),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.source, prev));
            self.after_history();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.source, next));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.coalesce = None;
        self.code = text_editor::Content::with_text(&self.source);
        self.reparse();
    }

    /// Add a node at `at` (diagram coordinates), optionally connected from
    /// `from`, as one undo step; selects it.
    fn add_node(&mut self, at: Point, from: Option<String>) -> Option<String> {
        let model = self.model.as_ref()?;
        let id = model.fresh_id();
        let shape = Shape::Rect;
        let mut src = match model.apply(&Op::AddNode { id: id.clone(), label: "New node".into(), shape }) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(e.to_string());
                return None;
            }
        };
        if let Some(from) = from {
            let next = Flowchart::parse(&src).ok()?;
            src = next.apply(&Op::AddEdge { from, to: id.clone(), label: None }).ok()?;
        }
        self.positions.insert(id.clone(), at);
        self.set_source(src, None, true);
        self.selection = Sel::Node(id.clone());
        self.sync_inputs();
        Some(id)
    }

    /// A free spot for a new node: after the selected node in the flow
    /// direction, else the middle of the view.
    fn new_node_spot(&self) -> Point {
        if let (Sel::Node(id), Some(model)) = (&self.selection, &self.model)
            && let Some(p) = self.positions.get(id)
        {
            let s = self.sizes.get(id).copied().unwrap_or(Size::new(120.0, 44.0));
            let gap = 70.0;
            let step = match model.direction {
                Direction::TopDown => Vector::new(0.0, s.height + gap),
                Direction::BottomUp => Vector::new(0.0, -(s.height + gap)),
                Direction::LeftRight => Vector::new(s.width + gap, 0.0),
                Direction::RightLeft => Vector::new(-(s.width + gap), 0.0),
            };
            let mut at = *p + step;
            // Step sideways past nodes already there.
            let side = if model.direction.is_vertical() {
                Vector::new(s.width + 30.0, 0.0)
            } else {
                Vector::new(0.0, s.height + 30.0)
            };
            while self.positions.values().any(|q| q.distance(at) < 40.0) {
                at += side;
            }
            return at;
        }
        let c = Point::new(self.canvas_size.width / 2.0, self.canvas_size.height / 2.0);
        Point::new((c.x - self.pan.x) / self.zoom, (c.y - self.pan.y) / self.zoom)
    }

    fn auto_layout(&mut self) {
        self.positions.clear();
        self.reparse();
        self.fit();
    }

    fn fit(&mut self) {
        let Some(model) = &self.model else { return };
        if self.canvas_size.width <= 0.0 {
            return;
        }
        let board = canvas::Board {
            model,
            positions: &self.positions,
            sizes: &self.sizes,
            selection: &self.selection,
            pan: self.pan,
            zoom: self.zoom,
            colors: canvas::Colors::new(&mdedit_theme::Theme::ember_light()),
            font: iced::Font::DEFAULT,
        };
        let Some(b) = board.content_bounds() else {
            self.zoom = 1.0;
            self.pan = Vector::new(self.canvas_size.width / 2.0, self.canvas_size.height / 2.0);
            return;
        };
        let margin = 48.0;
        let zx = (self.canvas_size.width - 2.0 * margin) / b.width.max(1.0);
        let zy = (self.canvas_size.height - 2.0 * margin) / b.height.max(1.0);
        self.zoom = zx.min(zy).clamp(canvas::MIN_ZOOM, 1.25);
        let c = b.center();
        self.pan = Vector::new(
            self.canvas_size.width / 2.0 - c.x * self.zoom,
            self.canvas_size.height / 2.0 - c.y * self.zoom,
        );
    }

    fn delete_selection(&mut self) {
        let op = match &self.selection {
            Sel::Node(id) => Op::DeleteNode(id.clone()),
            Sel::Edge(i) => Op::DeleteEdge(*i),
            Sel::None => return,
        };
        self.apply(&op, None);
        self.selection = Sel::None;
    }

    /// Handle a message; tasks are focus changes inside this window.
    fn update(&mut self, msg: DiagramMsg) -> Task<DiagramMsg> {
        match msg {
            DiagramMsg::Canvas(m) => return self.on_canvas(m),
            DiagramMsg::Direction(d) => self.apply(&Op::SetDirection(d), None),
            DiagramMsg::Shape(shape) => {
                if let Sel::Node(id) = &self.selection {
                    let id = id.clone();
                    self.apply(&Op::SetShape { id, shape }, None);
                }
            }
            DiagramMsg::Label(text) => {
                self.label = text.clone();
                if let Sel::Node(id) = &self.selection {
                    let id = id.clone();
                    if !text.trim().is_empty() {
                        self.apply(&Op::SetLabel { id: id.clone(), label: text }, Some(format!("label:{id}")));
                    }
                }
            }
            DiagramMsg::EdgeLabel(text) => {
                self.edge_label = text.clone();
                if let Sel::Edge(i) = self.selection {
                    self.apply(&Op::SetEdgeLabel { edge: i, label: Some(text) }, Some(format!("edge-label:{i}")));
                }
            }
            DiagramMsg::EdgeStyle(style) => {
                if let Sel::Edge(i) = self.selection {
                    self.apply(&Op::SetEdgeStyle { edge: i, style }, None);
                }
            }
            DiagramMsg::LabelDone => {
                self.coalesce = None;
                self.sync_inputs();
                return iced::widget::operation::focus(canvas_focus_sink());
            }
            DiagramMsg::AddNode => {
                let at = self.new_node_spot();
                let from = match &self.selection {
                    Sel::Node(id) => Some(id.clone()),
                    _ => None,
                };
                if self.add_node(at, from).is_some() {
                    return focus_label();
                }
            }
            DiagramMsg::Delete => self.delete_selection(),
            DiagramMsg::Undo => self.undo(),
            DiagramMsg::Redo => self.redo(),
            DiagramMsg::AutoLayout => self.auto_layout(),
            DiagramMsg::Fit => self.fit(),
            DiagramMsg::Code(action) => {
                let edit = action.is_edit();
                self.code.perform(action);
                if edit {
                    let text = self.code.text();
                    self.set_source(text, Some("code".into()), false);
                }
            }
            DiagramMsg::KeepEditing => self.confirm_close = false,
            DiagramMsg::Key(key, m) => return self.on_key(key, m),
            DiagramMsg::Save { .. } | DiagramMsg::CloseRequested | DiagramMsg::Discard => {}
        }
        Task::none()
    }

    fn on_canvas(&mut self, m: CanvasMsg) -> Task<DiagramMsg> {
        match m {
            CanvasMsg::Select(sel) => {
                if sel != self.selection {
                    self.coalesce = None;
                }
                self.selection = sel;
                self.sync_inputs();
            }
            CanvasMsg::Move(id, p) => {
                self.positions.insert(id, p);
            }
            CanvasMsg::Connect(from, to) => {
                self.apply(&Op::AddEdge { from, to, label: None }, None);
                if let Some(model) = &self.model {
                    self.selection = Sel::Edge(model.edges.len().saturating_sub(1));
                    self.sync_inputs();
                }
            }
            CanvasMsg::ConnectNew(from, at) => {
                if self.add_node(at, Some(from)).is_some() {
                    return focus_label();
                }
            }
            CanvasMsg::AddAt(at) => {
                if self.add_node(at, None).is_some() {
                    return focus_label();
                }
            }
            CanvasMsg::EditLabel(id) => {
                self.selection = Sel::Node(id);
                self.sync_inputs();
                return focus_label();
            }
            CanvasMsg::EditEdgeLabel(i) => {
                self.selection = Sel::Edge(i);
                self.sync_inputs();
                return Task::batch([
                    iced::widget::operation::focus(EDGE_LABEL_INPUT),
                    iced::widget::operation::select_all(EDGE_LABEL_INPUT),
                ]);
            }
            CanvasMsg::View(pan, zoom) => {
                self.pan = pan;
                self.zoom = zoom;
            }
            CanvasMsg::Resized(size) => {
                let first = self.canvas_size.width <= 0.0;
                // Keep the view centred on the same point.
                if !first {
                    self.pan += Vector::new(
                        (size.width - self.canvas_size.width) / 2.0,
                        (size.height - self.canvas_size.height) / 2.0,
                    );
                }
                self.canvas_size = size;
                if !self.fitted {
                    self.fitted = true;
                    self.fit();
                }
            }
        }
        Task::none()
    }

    fn on_key(&mut self, key: Key, m: Modifiers) -> Task<DiagramMsg> {
        match key.as_ref() {
            Key::Named(Named::Delete | Named::Backspace) if !m.command() => self.delete_selection(),
            Key::Named(Named::Escape) => {
                if self.confirm_close {
                    self.confirm_close = false;
                } else {
                    self.selection = Sel::None;
                }
            }
            Key::Character("z") if m.command() && m.shift() => self.redo(),
            Key::Character("z") if m.command() => self.undo(),
            Key::Character("y") if m.command() => self.redo(),
            Key::Character("0") if m.command() => self.fit(),
            Key::Character("s") if m.command() => return Task::done(DiagramMsg::Save { close: false }),
            Key::Named(Named::Enter) if m.command() => return Task::done(DiagramMsg::Save { close: true }),
            Key::Character("n") if m.command() => return Task::done(DiagramMsg::AddNode),
            _ => {}
        }
        Task::none()
    }
}

fn focus_label() -> Task<DiagramMsg> {
    Task::batch([iced::widget::operation::focus(LABEL_INPUT), iced::widget::operation::select_all(LABEL_INPUT)])
}

/// An id no widget has: focusing it takes the keyboard away from the label
/// inputs, so Delete and the shortcuts reach the canvas again.
fn canvas_focus_sink() -> &'static str {
    "diagram-canvas"
}

impl App {
    /// The Edit button of a diagram: open (or raise) its editor window.
    pub(crate) fn open_diagram(&mut self, tab: TabId, range: Range<usize>) -> Task<Message> {
        if let Some(ed) = self.diagrams.iter().find(|e| e.tab == tab && e.block.range.start == range.start) {
            return window::gain_focus(ed.window);
        }
        let Some(t) = self.tabs.iter().find(|t| t.id == tab) else { return Task::none() };
        let buf = t.doc.buffer();
        let range = range.start.min(buf.len_bytes())..range.end.min(buf.len_bytes());
        let text = buf.slice(range.clone()).into_owned();
        let line_start = buf.line_to_byte(buf.byte_to_line(range.start));
        let prefix = buf.slice(line_start..range.start).into_owned();
        let doc_title = t.title();
        let (block, code) = Block::new(text, range, prefix);
        let style = Arc::new(mdedit_mermaid::Style::from_theme(&self.theme));
        let (id, open) = window::open(diagram_window_settings());
        let mut ed = Editor::new(id, tab, block, code, style);
        ed.doc_title = doc_title;
        self.diagrams.push(ed);
        open.discard()
    }

    pub(crate) fn diagram_index(&self, window: window::Id) -> Option<usize> {
        self.diagrams.iter().position(|e| e.window == window)
    }

    pub(crate) fn on_diagram(&mut self, window: window::Id, msg: DiagramMsg) -> Task<Message> {
        let Some(i) = self.diagram_index(window) else { return Task::none() };
        match msg {
            DiagramMsg::Save { close } => self.save_diagram(i, close),
            DiagramMsg::CloseRequested => {
                let ed = &mut self.diagrams[i];
                if ed.dirty() {
                    ed.confirm_close = true;
                    return window::gain_focus(window);
                }
                self.diagrams.remove(i);
                window::close(window)
            }
            DiagramMsg::Discard => {
                self.diagrams.remove(i);
                window::close(window)
            }
            msg => {
                let task = self.diagrams[i].update(msg);
                task.map(move |m| Message::Diagram(window, m))
            }
        }
    }

    /// Write the editor's code back into its block, as one undo step.
    fn save_diagram(&mut self, i: usize, close: bool) -> Task<Message> {
        let window = self.diagrams[i].window;
        let tab_id = self.diagrams[i].tab;
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            self.diagrams[i].error = Some("The document was closed. Copy the source to keep your changes.".into());
            return Task::none();
        };
        let ed = &mut self.diagrams[i];
        if !ed.dirty() {
            ed.confirm_close = false;
            return if close {
                self.diagrams.remove(i);
                window::close(window)
            } else {
                Task::none()
            };
        }
        if !tab.editable() {
            ed.error = Some("The document is read-only.".into());
            return Task::none();
        }
        // The block may have moved (edits above it) or changed.
        let buf = tab.doc.buffer();
        let len = buf.len_bytes();
        let r = ed.block.range.clone();
        let at = if r.end <= len && buf.slice(r.clone()) == ed.block.text {
            Some(r.start)
        } else {
            buf.text().find(&ed.block.text)
        };
        let Some(start) = at else {
            ed.error = Some("This diagram was changed in the document since the editor opened.".into());
            ed.confirm_close = false;
            return Task::none();
        };
        let code_text = ed.block.code_text(&ed.source);
        let edit_range = start + ed.block.code.start..start + ed.block.code.end;
        let txn = tab.doc.next_txn();
        tab.apply(&Edit { range: edit_range, text: code_text.clone(), origin: EditOrigin::Command, txn });
        // Remember the block as it is now in the doc.
        let mut text = ed.block.text.clone();
        text.replace_range(ed.block.code.clone(), &code_text);
        let code_end = ed.block.code.start + code_text.len();
        ed.block = Block {
            range: start..start + text.len(),
            text,
            code: ed.block.code.start..code_end,
            prefix: ed.block.prefix.clone(),
        };
        ed.saved = ed.source.clone();
        ed.error = None;
        ed.confirm_close = false;
        if close {
            self.diagrams.remove(i);
            return window::close(window);
        }
        Task::none()
    }

    pub(crate) fn diagram_title(&self, window: window::Id) -> String {
        match self.diagram_index(window).map(|i| &self.diagrams[i]) {
            Some(ed) => format!("{}Edit diagram - {} - mdedit", if ed.dirty() { "● " } else { "" }, ed.doc_title),
            None => "mdedit".into(),
        }
    }

    /// Diagram windows follow theme changes.
    pub(crate) fn restyle_diagrams(&mut self) {
        let style = Arc::new(mdedit_mermaid::Style::from_theme(&self.theme));
        for ed in &mut self.diagrams {
            ed.style = Arc::clone(&style);
        }
    }

    pub(crate) fn diagram_view(&self, window: window::Id) -> iced::Element<'_, Message> {
        match self.diagram_index(window) {
            Some(i) => panel::view(&self.diagrams[i], &self.theme).map(move |m| Message::Diagram(window, m)),
            None => iced::widget::Space::new().into(),
        }
    }

    /// Re-render previews while the diagram worker is busy.
    pub(crate) fn diagram_subscription(&self) -> Option<iced::Subscription<Message>> {
        (!self.diagrams.is_empty() && mdedit_view::diagrams::cache().is_loading())
            .then(|| iced::time::every(std::time::Duration::from_millis(60)).map(|_| Message::DiagramTick))
    }
}

fn diagram_window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(1280.0, 820.0),
        min_size: Some(Size::new(860.0, 540.0)),
        exit_on_close_request: false,
        ..crate::window_settings()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_split_and_rebuild() {
        let text = "```mermaid\nflowchart LR\n  A --> B\n```".to_string();
        let (b, code) = Block::new(text.clone(), 10..10 + text.len(), String::new());
        assert_eq!(code, "flowchart LR\n  A --> B\n");
        assert_eq!(&b.text[b.code.clone()], code);
        assert_eq!(b.code_text("graph TD\nX"), "graph TD\nX\n");
    }

    #[test]
    fn indented_and_quoted_blocks_keep_their_prefix() {
        let text = "```mermaid\n   graph TD\n   A --> B\n\n   ```".to_string();
        let (b, code) = Block::new(text.clone(), 0..text.len(), "   ".into());
        assert_eq!(code, "graph TD\nA --> B\n\n");
        assert_eq!(b.code_text("graph LR\n\nA"), "   graph LR\n\n   A\n");
        let text = "```mermaid\n> graph TD\n> A\n> ```".to_string();
        let (b, code) = Block::new(text.clone(), 0..text.len(), "> ".into());
        assert_eq!(code, "graph TD\nA\n");
        assert_eq!(b.code_text("graph LR\n"), "> graph LR\n");
    }

    #[test]
    fn unclosed_block_takes_the_rest() {
        let text = "~~~mermaid\ngraph TD\nA".to_string();
        let (b, code) = Block::new(text.clone(), 0..text.len(), String::new());
        assert_eq!(code, "graph TD\nA");
        assert_eq!(b.code_text("graph LR"), "graph LR");
    }

    fn editor(src: &str) -> Editor {
        let text = format!("```mermaid\n{src}```");
        let (block, code) = Block::new(text.clone(), 0..text.len(), String::new());
        let style = Arc::new(mdedit_mermaid::Style::from_theme(&mdedit_theme::Theme::ember_light()));
        Editor::new(window::Id::unique(), 1, block, code, style)
    }

    #[test]
    fn editing_through_messages_with_undo() {
        let mut ed = editor("flowchart LR\n  A[Start] --> B\n");
        assert!(ed.positions.contains_key("A") && ed.positions.contains_key("B"));
        let _ = ed.update(DiagramMsg::Canvas(CanvasMsg::Select(Sel::Node("A".into()))));
        assert_eq!(ed.label, "Start");
        for typed in ["S", "Se", "Set"] {
            let _ = ed.update(DiagramMsg::Label(typed.into()));
        }
        assert_eq!(ed.source, "flowchart LR\n  A[Set] --> B\n");
        let _ = ed.update(DiagramMsg::Shape(Shape::Diamond));
        assert_eq!(ed.source, "flowchart LR\n  A{Set} --> B\n");
        // Typing the label was one step.
        let _ = ed.update(DiagramMsg::Undo);
        assert_eq!(ed.source, "flowchart LR\n  A[Set] --> B\n");
        let _ = ed.update(DiagramMsg::Undo);
        assert_eq!(ed.source, "flowchart LR\n  A[Start] --> B\n");
        assert!(!ed.dirty());
        let _ = ed.update(DiagramMsg::Redo);
        assert!(ed.dirty());
    }

    #[test]
    fn connect_to_empty_space_adds_a_connected_node() {
        let mut ed = editor("flowchart TD\n  A --> B\n");
        let _ = ed.update(DiagramMsg::Canvas(CanvasMsg::ConnectNew("B".into(), Point::new(300.0, 300.0))));
        assert_eq!(ed.source, "flowchart TD\n  A --> B\n  N1[New node]\n  B --> N1\n");
        assert_eq!(ed.selection, Sel::Node("N1".into()));
        assert_eq!(ed.positions["N1"], Point::new(300.0, 300.0));
        let _ = ed.update(DiagramMsg::Delete);
        assert_eq!(ed.source, "flowchart TD\n  A --> B\n");
        let _ = ed.update(DiagramMsg::Undo);
        assert!(ed.source.contains("B --> N1"));
    }

    #[test]
    fn source_typing_reparses() {
        let mut ed = editor("flowchart TD\n  A --> B\n");
        ed.code = text_editor::Content::with_text("flowchart TD\n  A --> B\n  B --> C\n");
        // Simulate an edit action: insert at the end.
        ed.code.perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
        let _ = ed.update(DiagramMsg::Code(text_editor::Action::Edit(text_editor::Edit::Insert('x'))));
        assert!(ed.model.as_ref().unwrap().node("C").is_some());
        assert!(ed.positions.contains_key("C"));
    }

    #[test]
    fn other_diagram_types_have_no_canvas() {
        let ed = editor("sequenceDiagram\n  A->>B: hi\n");
        assert!(ed.model.is_none());
    }
}
