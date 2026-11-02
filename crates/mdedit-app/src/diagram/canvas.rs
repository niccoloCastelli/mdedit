//! The flowchart canvas of the diagram editor: draws nodes, edges and
//! subgraphs at the editor's positions, and turns mouse input into
//! [`CanvasMsg`]s (select, drag, connect, add, pan, zoom). It holds no
//! diagram state of its own besides the gesture in progress.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::alignment::Vertical;
use iced::keyboard::Modifiers;
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Event, Frame, Geometry, LineDash, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Font, Point, Rectangle, Size, Vector};
use mdedit_mermaid::flowchart::{Direction, EdgeStyle, Flowchart, Head, Line, Shape};
use mdedit_theme::{ColorToken as T, Theme};

use crate::style::c;

/// Font size of labels on the canvas at zoom 1 (the renderer's size).
pub const LABEL_SIZE: f32 = 14.0;
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 3.0;
const HANDLE_RADIUS: f32 = 5.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Sel {
    #[default]
    None,
    Node(String),
    Edge(usize),
}

#[derive(Debug, Clone)]
pub enum CanvasMsg {
    Select(Sel),
    /// Node dragged to a new centre (diagram coordinates).
    Move(String, Point),
    Connect(String, String),
    /// Connection dropped on empty space: a new node there.
    ConnectNew(String, Point),
    AddAt(Point),
    EditLabel(String),
    EditEdgeLabel(usize),
    /// New pan and zoom.
    View(Vector, f32),
    Resized(Size),
}

#[derive(Debug, Clone, Copy)]
pub struct Colors {
    pub bg: Color,
    pub bg_subtle: Color,
    pub surface: Color,
    pub border: Color,
    pub border_strong: Color,
    pub text: Color,
    pub secondary: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_soft: Color,
}

impl Colors {
    pub fn new(t: &Theme) -> Self {
        Self {
            bg: c(t, T::Bg),
            bg_subtle: c(t, T::BgSubtle),
            surface: c(t, T::Surface),
            border: c(t, T::Border),
            border_strong: c(t, T::BorderStrong),
            text: c(t, T::Text),
            secondary: c(t, T::TextSecondary),
            muted: c(t, T::TextMuted),
            accent: c(t, T::Accent),
            accent_soft: c(t, T::AccentSoft),
        }
    }
}

/// What the canvas draws, borrowed from the editor.
pub struct Board<'a> {
    pub model: &'a Flowchart,
    pub positions: &'a HashMap<String, Point>,
    pub sizes: &'a HashMap<String, Size>,
    pub selection: &'a Sel,
    pub pan: Vector,
    pub zoom: f32,
    pub colors: Colors,
    pub font: Font,
}

#[derive(Debug)]
enum Drag {
    Node { id: String, offset: Vector, moved: bool },
    Connect { from: String, to: Point },
    Pan { last: Point },
}

#[derive(Default)]
pub struct State {
    drag: Option<Drag>,
    modifiers: Modifiers,
    last_click: Option<(Instant, Point)>,
    size: Size,
}

impl Board<'_> {
    fn to_diagram(&self, p: Point) -> Point {
        Point::new((p.x - self.pan.x) / self.zoom, (p.y - self.pan.y) / self.zoom)
    }

    pub fn size_of(&self, id: &str) -> Size {
        self.sizes.get(id).copied().unwrap_or(Size::new(120.0, 44.0))
    }

    fn center(&self, id: &str) -> Point {
        self.positions.get(id).copied().unwrap_or(Point::ORIGIN)
    }

    fn node_rect(&self, id: &str) -> Rectangle {
        let (c, s) = (self.center(id), self.size_of(id));
        Rectangle::new(Point::new(c.x - s.width / 2.0, c.y - s.height / 2.0), s)
    }

    /// Topmost node under a diagram point.
    fn node_at(&self, p: Point) -> Option<&str> {
        self.model.nodes.iter().rev().find(|n| self.node_rect(&n.id).contains(p)).map(|n| n.id.as_str())
    }

    /// Where the connection handle of a node sits: the side edges leave by.
    fn handle(&self, id: &str) -> Point {
        let r = self.node_rect(id);
        let c = r.center();
        match self.model.direction {
            Direction::TopDown => Point::new(c.x, r.y + r.height),
            Direction::BottomUp => Point::new(c.x, r.y),
            Direction::LeftRight => Point::new(r.x + r.width, c.y),
            Direction::RightLeft => Point::new(r.x, c.y),
        }
    }

    fn handle_hit(&self, id: &str, p: Point) -> bool {
        p.distance(self.handle(id)) <= (HANDLE_RADIUS + 3.0) / self.zoom
    }

    /// Polyline of each edge (diagram coordinates), parallel edges bowed
    /// apart.
    fn edge_points(&self) -> Vec<Vec<Point>> {
        let mut pairs: HashMap<(&str, &str), (usize, usize)> = HashMap::new();
        for e in &self.model.edges {
            let key = if e.from <= e.to { (e.from.as_str(), e.to.as_str()) } else { (e.to.as_str(), e.from.as_str()) };
            pairs.entry(key).or_default().1 += 1;
        }
        self.model
            .edges
            .iter()
            .map(|e| {
                let key =
                    if e.from <= e.to { (e.from.as_str(), e.to.as_str()) } else { (e.to.as_str(), e.from.as_str()) };
                let slot = pairs.get_mut(&key).expect("counted");
                let (k, n) = (slot.0, slot.1);
                slot.0 += 1;
                if e.from == e.to {
                    return self.self_loop(&e.from, k);
                }
                let (a, b) = (self.center(&e.from), self.center(&e.to));
                let d = b - a;
                let len = (d.x * d.x + d.y * d.y).sqrt().max(1.0);
                let normal = Vector::new(-d.y / len, d.x / len);
                // Same direction for both orientations of a pair.
                let sign = if e.from <= e.to { 1.0 } else { -1.0 };
                let bow = (k as f32 - (n as f32 - 1.0) / 2.0) * 28.0 * sign;
                let mid = Point::new((a.x + b.x) / 2.0 + normal.x * bow, (a.y + b.y) / 2.0 + normal.y * bow);
                let start = self.clip(&e.from, if bow == 0.0 { b } else { mid });
                let end = self.clip(&e.to, if bow == 0.0 { a } else { mid });
                if bow == 0.0 {
                    return vec![start, end];
                }
                (0..=12)
                    .map(|i| {
                        let t = i as f32 / 12.0;
                        let u = 1.0 - t;
                        Point::new(
                            u * u * start.x + 2.0 * u * t * mid.x + t * t * end.x,
                            u * u * start.y + 2.0 * u * t * mid.y + t * t * end.y,
                        )
                    })
                    .collect()
            })
            .collect()
    }

    fn self_loop(&self, id: &str, k: usize) -> Vec<Point> {
        let r = self.node_rect(id);
        let grow = 18.0 + 10.0 * k as f32;
        let (x0, x1) = (r.x + r.width * 0.6, r.x + r.width + grow);
        let (y0, y1) = (r.y + r.height * 0.3, r.y - grow);
        vec![
            Point::new(r.x + r.width, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
            Point::new(x0, r.y),
        ]
    }

    /// The point where the segment from the node's centre towards `toward`
    /// leaves its outline.
    fn clip(&self, id: &str, toward: Point) -> Point {
        let c = self.center(id);
        let s = self.size_of(id);
        let d = toward - c;
        if d.x == 0.0 && d.y == 0.0 {
            return c;
        }
        let (hw, hh) = (s.width / 2.0, s.height / 2.0);
        let shape = self.model.node(id).map_or(Shape::Rect, |n| n.shape);
        let t = match shape {
            Shape::Circle | Shape::DoubleCircle => {
                let r = hw.max(hh);
                r / (d.x * d.x + d.y * d.y).sqrt()
            }
            Shape::Diamond => 1.0 / (d.x.abs() / hw + d.y.abs() / hh),
            _ => {
                let tx = if d.x == 0.0 { f32::INFINITY } else { hw / d.x.abs() };
                let ty = if d.y == 0.0 { f32::INFINITY } else { hh / d.y.abs() };
                tx.min(ty)
            }
        };
        Point::new(c.x + d.x * t, c.y + d.y * t)
    }

    fn edge_at(&self, p: Point) -> Option<usize> {
        let tolerance = 6.0 / self.zoom;
        self.edge_points()
            .iter()
            .enumerate()
            .map(|(i, pts)| (i, pts.windows(2).map(|w| seg_distance(p, w[0], w[1])).fold(f32::INFINITY, f32::min)))
            .filter(|(_, d)| *d <= tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Bounds of everything drawn, in diagram coordinates.
    pub fn content_bounds(&self) -> Option<Rectangle> {
        let mut it = self.model.nodes.iter().map(|n| self.node_rect(&n.id));
        let first = it.next()?;
        Some(it.fold(first, |a, b| a.union(&b)))
    }

    /// Boxes of the subgraphs around their members (nested ones included).
    fn subgraph_rects(&self) -> Vec<(usize, Rectangle)> {
        let model = self.model;
        let depth = |mut i: usize| {
            let mut d = 0;
            while let Some(p) = model.subgraphs[i].parent {
                d += 1;
                i = p;
            }
            d
        };
        let inside = |node_sg: Option<usize>, sg: usize| {
            let mut cur = node_sg;
            while let Some(s) = cur {
                if s == sg {
                    return true;
                }
                cur = model.subgraphs[s].parent;
            }
            false
        };
        let mut out: Vec<(usize, Rectangle)> = (0..model.subgraphs.len())
            .filter_map(|sg| {
                let rect = model
                    .nodes
                    .iter()
                    .filter(|n| inside(n.subgraph, sg))
                    .map(|n| self.node_rect(&n.id))
                    .reduce(|a, b| a.union(&b))?;
                // Outer subgraphs get more room around nested ones.
                let max_depth = (0..model.subgraphs.len()).filter(|s| inside(Some(*s), sg)).map(depth).max();
                let extra = (max_depth.unwrap_or(0) - depth(sg)) as f32 * 14.0;
                let pad = 14.0 + extra;
                let top = 26.0 + extra * 2.0;
                Some((
                    sg,
                    Rectangle::new(
                        Point::new(rect.x - pad, rect.y - top),
                        Size::new(rect.width + 2.0 * pad, rect.height + pad + top),
                    ),
                ))
            })
            .collect();
        out.sort_by_key(|(sg, _)| depth(*sg));
        out
    }
}

fn seg_distance(p: Point, a: Point, b: Point) -> f32 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0);
    p.distance(Point::new(a.x + ab.x * t, a.y + ab.y * t))
}

impl canvas::Program<CanvasMsg> for Board<'_> {
    type State = State;

    fn update(
        &self,
        st: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<canvas::Action<CanvasMsg>> {
        use canvas::Action;
        let button = matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::ButtonReleased(_)));
        if st.size != bounds.size() && !button {
            st.size = bounds.size();
            return Some(Action::publish(CanvasMsg::Resized(bounds.size())));
        }
        match event {
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                st.modifiers = *m;
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let local = cursor.position_in(bounds)?;
                let p = self.to_diagram(local);
                if *button == mouse::Button::Middle {
                    st.drag = Some(Drag::Pan { last: local });
                    return Some(Action::capture());
                }
                if *button != mouse::Button::Left {
                    return None;
                }
                let now = Instant::now();
                let double = st.last_click.is_some_and(|(t, at)| now - t < DOUBLE_CLICK && at.distance(local) < 5.0);
                st.last_click = if double { None } else { Some((now, local)) };
                // The handle of the selected node starts a connection.
                if let Sel::Node(id) = self.selection
                    && self.handle_hit(id, p)
                {
                    st.drag = Some(Drag::Connect { from: id.clone(), to: p });
                    return Some(Action::request_redraw().and_capture());
                }
                if let Some(id) = self.node_at(p) {
                    let id = id.to_string();
                    if double {
                        st.drag = None;
                        return Some(Action::publish(CanvasMsg::EditLabel(id)).and_capture());
                    }
                    st.drag = Some(if st.modifiers.shift() {
                        Drag::Connect { from: id.clone(), to: p }
                    } else {
                        Drag::Node { id: id.clone(), offset: self.center(&id) - p, moved: false }
                    });
                    return Some(Action::publish(CanvasMsg::Select(Sel::Node(id))).and_capture());
                }
                if let Some(e) = self.edge_at(p) {
                    let msg = if double { CanvasMsg::EditEdgeLabel(e) } else { CanvasMsg::Select(Sel::Edge(e)) };
                    return Some(Action::publish(msg).and_capture());
                }
                if double {
                    return Some(Action::publish(CanvasMsg::AddAt(p)).and_capture());
                }
                st.drag = Some(Drag::Pan { last: local });
                Some(Action::publish(CanvasMsg::Select(Sel::None)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(drag) = &mut st.drag else {
                    // Hover feedback (handle, edges).
                    return cursor.is_over(bounds).then(Action::request_redraw);
                };
                let local = cursor.position_from(bounds.position())?;
                let p = self.to_diagram(local);
                match drag {
                    Drag::Node { id, offset, moved } => {
                        *moved = true;
                        Some(Action::publish(CanvasMsg::Move(id.clone(), p + *offset)).and_capture())
                    }
                    Drag::Connect { to, .. } => {
                        *to = p;
                        Some(Action::request_redraw().and_capture())
                    }
                    Drag::Pan { last } => {
                        let pan = self.pan + (local - *last);
                        *last = local;
                        Some(Action::publish(CanvasMsg::View(pan, self.zoom)).and_capture())
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(_)) => {
                let drag = st.drag.take()?;
                match drag {
                    Drag::Connect { from, to } => {
                        let msg = match self.node_at(to) {
                            Some(target) => CanvasMsg::Connect(from, target.to_string()),
                            None if self.node_rect(&from).contains(to) => return Some(Action::request_redraw()),
                            None => CanvasMsg::ConnectNew(from, to),
                        };
                        Some(Action::publish(msg).and_capture())
                    }
                    Drag::Node { moved: false, .. } | Drag::Pan { .. } | Drag::Node { .. } => {
                        Some(Action::request_redraw().and_capture())
                    }
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let local = cursor.position_in(bounds)?;
                let (dx, dy) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (x * 40.0, y * 40.0),
                    mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                };
                if st.modifiers.command() {
                    let zoom = (self.zoom * (1.0 + dy / 400.0)).clamp(MIN_ZOOM, MAX_ZOOM);
                    // Keep the point under the cursor in place.
                    let p = self.to_diagram(local);
                    let pan = Vector::new(local.x - p.x * zoom, local.y - p.y * zoom);
                    return Some(Action::publish(CanvasMsg::View(pan, zoom)).and_capture());
                }
                let (dx, dy) = if st.modifiers.shift() { (dy, dx) } else { (dx, dy) };
                Some(Action::publish(CanvasMsg::View(self.pan + Vector::new(dx, dy), self.zoom)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        st: &State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        let k = self.colors;
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), k.bg);
        // Dot grid.
        let step = 24.0 * self.zoom;
        if step >= 10.0 {
            let ox = self.pan.x.rem_euclid(step);
            let oy = self.pan.y.rem_euclid(step);
            let dots = Path::new(|b| {
                let mut y = oy;
                while y < bounds.height {
                    let mut x = ox;
                    while x < bounds.width {
                        b.rectangle(Point::new(x - 0.75, y - 0.75), Size::new(1.5, 1.5));
                        x += step;
                    }
                    y += step;
                }
            });
            frame.fill(&dots, k.border);
        }
        let hover = cursor.position_in(bounds).map(|p| self.to_diagram(p));
        let hover_node = hover.and_then(|p| self.node_at(p));
        let hover_edge = if hover_node.is_none() { hover.and_then(|p| self.edge_at(p)) } else { None };
        let connect_target = match &st.drag {
            Some(Drag::Connect { to, .. }) => self.node_at(*to),
            _ => None,
        };

        frame.with_save(|f| {
            f.translate(self.pan);
            f.scale(self.zoom);
            for (sg, rect) in self.subgraph_rects() {
                let path = Path::rounded_rectangle(rect.position(), rect.size(), 8.0.into());
                f.fill(&path, Color { a: 0.6, ..k.bg_subtle });
                f.stroke(&path, Stroke::default().with_color(k.border).with_width(1.0));
                f.fill_text(Text {
                    content: self.model.subgraphs[sg].title.clone(),
                    position: Point::new(rect.x + 10.0, rect.y + 8.0),
                    color: k.secondary,
                    size: 12.0.into(),
                    font: self.font,
                    ..Text::default()
                });
            }
            let points = self.edge_points();
            for (i, (edge, pts)) in self.model.edges.iter().zip(&points).enumerate() {
                let selected = *self.selection == Sel::Edge(i);
                draw_edge(f, pts, edge.style, selected, hover_edge == Some(i), k);
            }
            for (edge, pts) in self.model.edges.iter().zip(&points) {
                if let Some(label) = edge.label.as_deref().filter(|l| !l.is_empty()) {
                    let mid = midpoint(pts);
                    draw_edge_label(f, mid, label, self.font, k);
                }
            }
            for node in &self.model.nodes {
                let rect = self.node_rect(&node.id);
                let selected = *self.selection == Sel::Node(node.id.clone());
                let target = connect_target == Some(node.id.as_str());
                let fill = if selected || target { k.accent_soft } else { k.surface };
                let stroke = if selected || target { k.accent } else { k.border_strong };
                draw_shape(f, node.shape, rect, fill, stroke, if selected { 2.0 } else { 1.25 });
                f.fill_text(Text {
                    content: display_label(&node.label),
                    position: rect.center(),
                    max_width: (rect.width - 12.0).max(20.0),
                    color: k.text,
                    size: LABEL_SIZE.into(),
                    font: self.font,
                    align_x: Alignment::Center,
                    align_y: Vertical::Center,
                    ..Text::default()
                });
            }
            // Connection handle on the selected (or hovered) node.
            let handle_on = match self.selection {
                Sel::Node(id) => Some(id.as_str()),
                _ => hover_node,
            };
            if let Some(id) = handle_on
                && st.drag.is_none()
                && self.model.node(id).is_some()
            {
                let p = self.handle(id);
                let over = hover.is_some_and(|h| self.handle_hit(id, h));
                let r = (HANDLE_RADIUS + if over { 1.5 } else { 0.0 }) / self.zoom;
                f.fill(&Path::circle(p, r), if over { k.accent } else { k.bg });
                f.stroke(&Path::circle(p, r), Stroke::default().with_color(k.accent).with_width(1.5 / self.zoom));
            }
            if let Some(Drag::Connect { from, to }) = &st.drag {
                let start = self.clip(from, *to);
                let line = Path::line(start, *to);
                let stroke = Stroke {
                    line_dash: LineDash { segments: &[6.0, 4.0], offset: 0 },
                    ..Stroke::default().with_color(k.accent).with_width(1.5)
                };
                f.stroke(&line, stroke);
                f.fill(&Path::circle(*to, 3.0), k.accent);
            }
        });

        if self.model.nodes.is_empty() {
            frame.fill_text(Text {
                content: "Double-click to add a node".into(),
                position: Point::new(bounds.width / 2.0, bounds.height / 2.0),
                color: k.muted,
                size: 14.0.into(),
                font: self.font,
                align_x: Alignment::Center,
                align_y: Vertical::Center,
                ..Text::default()
            });
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, st: &State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        match &st.drag {
            Some(Drag::Pan { .. }) => return mouse::Interaction::Grabbing,
            Some(Drag::Node { .. }) => return mouse::Interaction::Grabbing,
            Some(Drag::Connect { .. }) => return mouse::Interaction::Crosshair,
            None => {}
        }
        let Some(local) = cursor.position_in(bounds) else { return mouse::Interaction::default() };
        let p = self.to_diagram(local);
        if let Sel::Node(id) = self.selection
            && self.handle_hit(id, p)
        {
            return mouse::Interaction::Crosshair;
        }
        if self.node_at(p).is_some() {
            return mouse::Interaction::Grab;
        }
        if self.edge_at(p).is_some() {
            return mouse::Interaction::Pointer;
        }
        mouse::Interaction::default()
    }
}

/// `<br>` tags as line breaks.
pub fn display_label(label: &str) -> String {
    label.replace("<br/>", "\n").replace("<br />", "\n").replace("<br>", "\n")
}

fn midpoint(pts: &[Point]) -> Point {
    if pts.len() == 2 {
        return Point::new((pts[0].x + pts[1].x) / 2.0, (pts[0].y + pts[1].y) / 2.0);
    }
    pts[pts.len() / 2]
}

fn draw_edge(f: &mut Frame, pts: &[Point], style: EdgeStyle, selected: bool, hovered: bool, k: Colors) {
    if pts.len() < 2 {
        return;
    }
    let color = if selected {
        k.accent
    } else if style.line == Line::Invisible {
        Color { a: 0.5, ..k.muted }
    } else if hovered {
        k.text
    } else {
        k.secondary
    };
    let width = match style.line {
        Line::Thick => 3.0,
        _ => 1.5,
    } + if selected { 1.0 } else { 0.0 };
    let dash: &[f32] = match style.line {
        Line::Dotted => &[3.0, 4.0],
        Line::Invisible => &[2.0, 6.0],
        _ => &[],
    };
    let stroke = Stroke { line_dash: LineDash { segments: dash, offset: 0 }, ..Stroke::default() }
        .with_color(color)
        .with_width(width);
    // Leave room for the arrow heads.
    let head_len = if style.line == Line::Thick { 12.0 } else { 9.0 };
    let mut body = pts.to_vec();
    let has_end = style.head != Head::None && style.line != Line::Invisible;
    let has_start = has_end && style.both;
    if has_end {
        shorten(&mut body, head_len * 0.8, false);
    }
    if has_start {
        shorten(&mut body, head_len * 0.8, true);
    }
    let path = Path::new(|b| {
        b.move_to(body[0]);
        for p in &body[1..] {
            b.line_to(*p);
        }
    });
    f.stroke(&path, stroke);
    if has_end {
        let n = pts.len();
        draw_head(f, pts[n - 1], pts[n - 2], style.head, head_len, color, k.bg);
    }
    if has_start {
        draw_head(f, pts[0], pts[1], style.head, head_len, color, k.bg);
    }
}

fn shorten(pts: &mut [Point], by: f32, at_start: bool) {
    let n = pts.len();
    let (tip, prev) = if at_start { (0, 1) } else { (n - 1, n - 2) };
    let d = pts[tip] - pts[prev];
    let len = (d.x * d.x + d.y * d.y).sqrt();
    if len > by {
        pts[tip] -= d * (by / len);
    }
}

fn draw_head(f: &mut Frame, tip: Point, from: Point, head: Head, len: f32, color: Color, bg: Color) {
    let d = tip - from;
    let l = (d.x * d.x + d.y * d.y).sqrt().max(0.001);
    let u = Vector::new(d.x / l, d.y / l);
    let n = Vector::new(-u.y, u.x);
    match head {
        Head::Arrow => {
            let base = tip - u * len;
            let w = len * 0.45;
            let tri = Path::new(|b| {
                b.move_to(tip);
                b.line_to(base + n * w);
                b.line_to(base - n * w);
                b.close();
            });
            f.fill(&tri, color);
        }
        Head::Circle => {
            let r = len * 0.35;
            let c = tip - u * r;
            f.fill(&Path::circle(c, r), bg);
            f.stroke(&Path::circle(c, r), Stroke::default().with_color(color).with_width(1.5));
        }
        Head::Cross => {
            let c = tip - u * (len * 0.4);
            let a = len * 0.35;
            let path = Path::new(|b| {
                b.move_to(c + (u + n) * a);
                b.line_to(c - (u + n) * a);
                b.move_to(c + (u - n) * a);
                b.line_to(c - (u - n) * a);
            });
            f.stroke(&path, Stroke::default().with_color(color).with_width(1.75));
        }
        Head::None => {}
    }
}

fn draw_edge_label(f: &mut Frame, at: Point, label: &str, font: Font, k: Colors) {
    let text = display_label(label);
    let lines = text.lines().count().max(1) as f32;
    let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
    let size = Size::new(longest * 6.6 + 12.0, lines * 16.0 + 6.0);
    let rect = Rectangle::new(Point::new(at.x - size.width / 2.0, at.y - size.height / 2.0), size);
    let path = Path::rounded_rectangle(rect.position(), size, 4.0.into());
    f.fill(&path, k.bg);
    f.stroke(&path, Stroke::default().with_color(k.border).with_width(1.0));
    f.fill_text(Text {
        content: text,
        position: at,
        color: k.secondary,
        size: 12.0.into(),
        font,
        align_x: Alignment::Center,
        align_y: Vertical::Center,
        ..Text::default()
    });
}

/// Draw a node outline filling `rect` (also used for the shape buttons).
pub fn draw_shape(f: &mut Frame, shape: Shape, rect: Rectangle, fill: Color, stroke: Color, width: f32) {
    let s = Stroke::default().with_color(stroke).with_width(width);
    let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
    let c = rect.center();
    let poly = |pts: &[Point]| {
        Path::new(|b| {
            b.move_to(pts[0]);
            for p in &pts[1..] {
                b.line_to(*p);
            }
            b.close();
        })
    };
    let slant = (h * 0.35).min(w * 0.2);
    let outline = match shape {
        Shape::Rect | Shape::Subroutine => Path::rectangle(rect.position(), rect.size()),
        Shape::Round => Path::rounded_rectangle(rect.position(), rect.size(), (h * 0.22).min(10.0).into()),
        Shape::Stadium => Path::rounded_rectangle(rect.position(), rect.size(), (h / 2.0).into()),
        Shape::Circle | Shape::DoubleCircle => Path::circle(c, w.max(h) / 2.0),
        Shape::Diamond => {
            poly(&[Point::new(c.x, y), Point::new(x + w, c.y), Point::new(c.x, y + h), Point::new(x, c.y)])
        }
        Shape::Hexagon => {
            let d = (h / 2.0).min(w / 4.0);
            poly(&[
                Point::new(x + d, y),
                Point::new(x + w - d, y),
                Point::new(x + w, c.y),
                Point::new(x + w - d, y + h),
                Point::new(x + d, y + h),
                Point::new(x, c.y),
            ])
        }
        Shape::Asymmetric => {
            let d = (h / 2.0).min(w / 4.0);
            poly(&[
                Point::new(x, y),
                Point::new(x + w, y),
                Point::new(x + w, y + h),
                Point::new(x, y + h),
                Point::new(x + d, c.y),
            ])
        }
        Shape::Parallelogram => poly(&[
            Point::new(x + slant, y),
            Point::new(x + w, y),
            Point::new(x + w - slant, y + h),
            Point::new(x, y + h),
        ]),
        Shape::ParallelogramAlt => poly(&[
            Point::new(x, y),
            Point::new(x + w - slant, y),
            Point::new(x + w, y + h),
            Point::new(x + slant, y + h),
        ]),
        Shape::Trapezoid => poly(&[
            Point::new(x + slant, y),
            Point::new(x + w - slant, y),
            Point::new(x + w, y + h),
            Point::new(x, y + h),
        ]),
        Shape::TrapezoidAlt => poly(&[
            Point::new(x, y),
            Point::new(x + w, y),
            Point::new(x + w - slant, y + h),
            Point::new(x + slant, y + h),
        ]),
        Shape::Cylinder => {
            let ry = (h * 0.12).min(10.0);
            Path::new(|b| {
                b.move_to(Point::new(x, y + ry));
                b.bezier_curve_to(
                    Point::new(x, y - ry / 3.0),
                    Point::new(x + w, y - ry / 3.0),
                    Point::new(x + w, y + ry),
                );
                b.line_to(Point::new(x + w, y + h - ry));
                b.bezier_curve_to(
                    Point::new(x + w, y + h + ry / 3.0),
                    Point::new(x, y + h + ry / 3.0),
                    Point::new(x, y + h - ry),
                );
                b.close();
            })
        }
    };
    f.fill(&outline, fill);
    f.stroke(&outline, s);
    match shape {
        Shape::Subroutine => {
            let d = (w * 0.08).min(8.0);
            let lines = Path::new(|b| {
                b.move_to(Point::new(x + d, y));
                b.line_to(Point::new(x + d, y + h));
                b.move_to(Point::new(x + w - d, y));
                b.line_to(Point::new(x + w - d, y + h));
            });
            f.stroke(&lines, s);
        }
        Shape::DoubleCircle => {
            f.stroke(&Path::circle(c, w.max(h) / 2.0 - 4.0_f32.min(w / 6.0)), s);
        }
        Shape::Cylinder => {
            let ry = (h * 0.12).min(10.0);
            let rim = Path::new(|b| {
                b.move_to(Point::new(x, y + ry));
                b.bezier_curve_to(
                    Point::new(x, y + ry + ry * 1.33),
                    Point::new(x + w, y + ry + ry * 1.33),
                    Point::new(x + w, y + ry),
                );
            });
            f.stroke(&rim, s);
        }
        _ => {}
    }
}

/// A shape button's glyph.
pub struct Glyph {
    pub shape: Shape,
    pub fill: Color,
    pub stroke: Color,
}

impl<M> canvas::Program<M> for Glyph {
    type State = ();

    fn draw(&self, _: &(), renderer: &iced::Renderer, _: &iced::Theme, bounds: Rectangle, _: Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let round = matches!(self.shape, Shape::Circle | Shape::DoubleCircle);
        let size = if round {
            let d = bounds.height - 2.0;
            Size::new(d, d)
        } else {
            Size::new(bounds.width - 2.0, bounds.height - 4.0)
        };
        let rect =
            Rectangle::new(Point::new((bounds.width - size.width) / 2.0, (bounds.height - size.height) / 2.0), size);
        draw_shape(&mut frame, self.shape, rect, self.fill, self.stroke, 1.25);
        vec![frame.into_geometry()]
    }
}
