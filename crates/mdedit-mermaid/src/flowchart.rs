//! Flowchart source model for the visual diagram editor.
//!
//! [`Flowchart::parse`] reads `flowchart` / `graph` source into nodes, edges
//! and subgraphs, remembering where each piece came from. Edits ([`Op`]) are
//! applied to the *text*, touching only the statements involved, so
//! comments, `classDef`/`style` lines, spacing and the user's own way of
//! writing arrows survive. The model is never serialised as a whole: after
//! an edit the new source is parsed again.
//!
//! Statements this parser doesn't understand are kept verbatim and ignored.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Whether `src` is a flowchart (`flowchart` or `graph` header).
pub fn is_flowchart(src: &str) -> bool {
    statements(src).iter().find(|s| !is_comment(&src[s.range.clone()])).is_some_and(|s| {
        let first = src[s.range.clone()].split_whitespace().next().unwrap_or_default();
        first == "flowchart" || first == "graph" || first == "flowchart-elk"
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

impl Direction {
    pub const ALL: [Direction; 4] = [Self::TopDown, Self::LeftRight, Self::BottomUp, Self::RightLeft];

    /// The token written in the header.
    pub fn token(self) -> &'static str {
        match self {
            Self::TopDown => "TD",
            Self::BottomUp => "BT",
            Self::LeftRight => "LR",
            Self::RightLeft => "RL",
        }
    }

    fn from_token(t: &str) -> Option<Self> {
        Some(match t {
            "TB" | "TD" | "v" => Self::TopDown,
            "BT" | "^" => Self::BottomUp,
            "LR" | ">" => Self::LeftRight,
            "RL" | "<" => Self::RightLeft,
            _ => return None,
        })
    }

    pub fn is_vertical(self) -> bool {
        matches!(self, Self::TopDown | Self::BottomUp)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    Rect,
    Round,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    DoubleCircle,
    Asymmetric,
    Diamond,
    Hexagon,
    Parallelogram,
    ParallelogramAlt,
    Trapezoid,
    TrapezoidAlt,
}

impl Shape {
    pub const ALL: [Shape; 14] = [
        Self::Rect,
        Self::Round,
        Self::Stadium,
        Self::Subroutine,
        Self::Cylinder,
        Self::Circle,
        Self::DoubleCircle,
        Self::Asymmetric,
        Self::Diamond,
        Self::Hexagon,
        Self::Parallelogram,
        Self::ParallelogramAlt,
        Self::Trapezoid,
        Self::TrapezoidAlt,
    ];

    pub fn delimiters(self) -> (&'static str, &'static str) {
        match self {
            Self::Rect => ("[", "]"),
            Self::Round => ("(", ")"),
            Self::Stadium => ("([", "])"),
            Self::Subroutine => ("[[", "]]"),
            Self::Cylinder => ("[(", ")]"),
            Self::Circle => ("((", "))"),
            Self::DoubleCircle => ("(((", ")))"),
            Self::Asymmetric => (">", "]"),
            Self::Diamond => ("{", "}"),
            Self::Hexagon => ("{{", "}}"),
            Self::Parallelogram => ("[/", "/]"),
            Self::ParallelogramAlt => ("[\\", "\\]"),
            Self::Trapezoid => ("[/", "\\]"),
            Self::TrapezoidAlt => ("[\\", "/]"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rect => "Rectangle",
            Self::Round => "Rounded",
            Self::Stadium => "Stadium",
            Self::Subroutine => "Subroutine",
            Self::Cylinder => "Database",
            Self::Circle => "Circle",
            Self::DoubleCircle => "Double circle",
            Self::Asymmetric => "Flag",
            Self::Diamond => "Decision",
            Self::Hexagon => "Hexagon",
            Self::Parallelogram => "Input / output",
            Self::ParallelogramAlt => "Output / input",
            Self::Trapezoid => "Trapezoid",
            Self::TrapezoidAlt => "Inverted trapezoid",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Line {
    Solid,
    Dotted,
    Thick,
    Invisible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Head {
    None,
    Arrow,
    Circle,
    Cross,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdgeStyle {
    pub line: Line,
    pub head: Head,
    /// The same head at the start too (`<-->`, `o--o`).
    pub both: bool,
}

impl Default for EdgeStyle {
    fn default() -> Self {
        Self { line: Line::Solid, head: Head::Arrow, both: false }
    }
}

impl EdgeStyle {
    fn from_arrow(arrow: &str) -> Self {
        let line = if arrow.contains('~') {
            Line::Invisible
        } else if arrow.contains('=') {
            Line::Thick
        } else if arrow.contains('.') {
            Line::Dotted
        } else {
            Line::Solid
        };
        let head = match arrow.chars().last() {
            Some('>') => Head::Arrow,
            Some('o') => Head::Circle,
            Some('x') => Head::Cross,
            _ => Head::None,
        };
        let both = head != Head::None && matches!(arrow.chars().next(), Some('<' | 'o' | 'x'));
        Self { line, head, both }
    }

    /// The shortest arrow for this style.
    pub fn arrow(self) -> String {
        if self.line == Line::Invisible {
            return "~~~".into();
        }
        let (start, end) = match self.head {
            Head::None => ("", ""),
            Head::Arrow => ("<", ">"),
            Head::Circle => ("o", "o"),
            Head::Cross => ("x", "x"),
        };
        let body = match (self.line, self.head) {
            (Line::Dotted, _) => "-.-",
            (Line::Thick, Head::None) => "===",
            (Line::Thick, _) => "==",
            (_, Head::None) => "---",
            _ => "--",
        };
        let start = if self.both { start } else { "" };
        format!("{start}{body}{end}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    /// Label as shown (unquoted, `#quot;` decoded); the id when there is no
    /// label in the source.
    pub label: String,
    pub shape: Shape,
    /// Index into [`Flowchart::subgraphs`] of the innermost subgraph the
    /// node was first mentioned in.
    pub subgraph: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub style: EdgeStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Subgraph {
    pub id: String,
    pub title: String,
    pub parent: Option<usize>,
}

/// An edit to the diagram.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    SetDirection(Direction),
    SetLabel {
        id: String,
        label: String,
    },
    SetShape {
        id: String,
        shape: Shape,
    },
    AddNode {
        id: String,
        label: String,
        shape: Shape,
    },
    AddEdge {
        from: String,
        to: String,
        label: Option<String>,
    },
    /// `None` or an empty label removes it.
    SetEdgeLabel {
        edge: usize,
        label: Option<String>,
    },
    SetEdgeStyle {
        edge: usize,
        style: EdgeStyle,
    },
    DeleteEdge(usize),
    DeleteNode(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    #[error("no such node: {0}")]
    NoNode(String),
    #[error("no such edge")]
    NoEdge,
    #[error("a node called {0} already exists")]
    Exists(String),
    #[error("{0} is not a valid node id")]
    BadId(String),
}

// ---------------------------------------------------------------------------
// Parsed statements (spans into the source)

#[derive(Debug, Clone)]
struct Stmt {
    range: Range<usize>,
    kind: StmtKind,
}

#[derive(Debug, Clone)]
enum StmtKind {
    Header {
        dir: Option<Range<usize>>,
        keyword_end: usize,
    },
    Chain(Chain),
    SubgraphStart,
    End,
    /// `style`, `click`: one target node.
    Target(String),
    /// `class a,b name`: the id list span.
    Class(Range<usize>),
    Other,
}

#[derive(Debug, Clone)]
struct Chain {
    groups: Vec<Vec<NodeRef>>,
    links: Vec<Link>,
}

#[derive(Debug, Clone)]
struct NodeRef {
    id: String,
    range: Range<usize>,
    id_end: usize,
    def: Option<Def>,
}

#[derive(Debug, Clone)]
struct Def {
    shape: Shape,
    /// From the opening to the closing delimiter, inclusive.
    range: Range<usize>,
    /// The raw label between the delimiters (quotes included).
    label: Range<usize>,
}

#[derive(Debug, Clone)]
struct Link {
    range: Range<usize>,
    /// The arrow alone (both halves of `-- text -->` when the text is
    /// inline: then `text` is set).
    style: EdgeStyle,
    label: Option<LinkLabel>,
}

#[derive(Debug, Clone)]
struct LinkLabel {
    /// Raw label text span (quotes included).
    text: Range<usize>,
    pipe: bool,
}

/// Where an edge came from: statement, link, and the two refs.
#[derive(Debug, Clone, Copy)]
struct EdgeSrc {
    stmt: usize,
    link: usize,
}

/// A parsed flowchart.
#[derive(Debug, Clone)]
pub struct Flowchart {
    src: String,
    pub direction: Direction,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub subgraphs: Vec<Subgraph>,
    stmts: Vec<Stmt>,
    edge_src: Vec<EdgeSrc>,
    /// Effective (last) definition of each node: statement, group, ref.
    defs: HashMap<String, (usize, usize, usize)>,
    /// First mention of each node.
    first: HashMap<String, (usize, usize, usize)>,
    header: usize,
}

impl Flowchart {
    /// Parse flowchart source. Fails only when it isn't a flowchart.
    pub fn parse(src: &str) -> Result<Self, String> {
        let raw = statements(src);
        let mut stmts = Vec::new();
        let mut header = None;
        let mut direction = Direction::TopDown;
        let mut nodes: Vec<Node> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut edges = Vec::new();
        let mut edge_src = Vec::new();
        let mut subgraphs: Vec<Subgraph> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut defs = HashMap::new();
        let mut first = HashMap::new();

        for s in raw {
            let text = &src[s.range.clone()];
            if is_comment(text) {
                continue;
            }
            let i = stmts.len();
            let word = text.split_whitespace().next().unwrap_or_default();
            if header.is_none() {
                if !matches!(word, "flowchart" | "graph" | "flowchart-elk") {
                    return Err("not a flowchart".into());
                }
                let keyword_end = s.range.start + word.len();
                let rest = &src[keyword_end..s.range.end];
                let tok = rest.split_whitespace().next();
                let dir = tok.and_then(|t| {
                    let at = keyword_end + rest.find(t)?;
                    Direction::from_token(t).map(|d| (d, at..at + t.len()))
                });
                if let Some((d, _)) = &dir {
                    direction = *d;
                }
                header = Some(i);
                stmts.push(Stmt { range: s.range, kind: StmtKind::Header { dir: dir.map(|(_, r)| r), keyword_end } });
                continue;
            }
            let kind = match word {
                "subgraph" => {
                    let (id, title) = subgraph_title(text["subgraph".len()..].trim());
                    stack.push(subgraphs.len());
                    let parent = stack.iter().rev().nth(1).copied();
                    subgraphs.push(Subgraph { id, title, parent });
                    StmtKind::SubgraphStart
                }
                "end" if text.trim() == "end" => {
                    stack.pop();
                    StmtKind::End
                }
                "style" | "click" => {
                    let target = text.split_whitespace().nth(1).unwrap_or_default().to_string();
                    StmtKind::Target(target)
                }
                "class" => {
                    let after = &text[5..];
                    let lead = after.len() - after.trim_start().len();
                    let ids = after.split_whitespace().next().unwrap_or_default();
                    let at = s.range.start + 5 + lead;
                    StmtKind::Class(at..at + ids.len())
                }
                "direction" | "classDef" | "linkStyle" | "accTitle" | "accDescr" | "accTitle:" | "accDescr:" => {
                    StmtKind::Other
                }
                _ => match parse_chain(src, s.range.clone()) {
                    Some(chain) => {
                        for (g, group) in chain.groups.iter().enumerate() {
                            for (r, nref) in group.iter().enumerate() {
                                let at = (i, g, r);
                                let n = *index.entry(nref.id.clone()).or_insert_with(|| {
                                    first.insert(nref.id.clone(), at);
                                    nodes.push(Node {
                                        id: nref.id.clone(),
                                        label: nref.id.clone(),
                                        shape: Shape::Rect,
                                        subgraph: stack.last().copied(),
                                    });
                                    nodes.len() - 1
                                });
                                if let Some(def) = &nref.def {
                                    nodes[n].label = decode_label(&src[def.label.clone()]);
                                    nodes[n].shape = def.shape;
                                    defs.insert(nref.id.clone(), at);
                                }
                            }
                        }
                        for (l, link) in chain.links.iter().enumerate() {
                            for a in &chain.groups[l] {
                                for b in &chain.groups[l + 1] {
                                    edges.push(Edge {
                                        from: a.id.clone(),
                                        to: b.id.clone(),
                                        label: link.label.as_ref().map(|t| decode_label(&src[t.text.clone()])),
                                        style: link.style,
                                    });
                                    edge_src.push(EdgeSrc { stmt: i, link: l });
                                }
                            }
                        }
                        StmtKind::Chain(chain)
                    }
                    None => StmtKind::Other,
                },
            };
            stmts.push(Stmt { range: s.range, kind });
        }
        let Some(header) = header else { return Err("not a flowchart".into()) };
        Ok(Self { src: src.to_string(), direction, nodes, edges, subgraphs, stmts, edge_src, defs, first, header })
    }

    pub fn source(&self) -> &str {
        &self.src
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// A node id not used yet: `N1`, `N2`, ...
    pub fn fresh_id(&self) -> String {
        (1..).map(|i| format!("N{i}")).find(|id| self.node(id).is_none()).unwrap_or_default()
    }

    /// The source after `op`.
    pub fn apply(&self, op: &Op) -> Result<String, EditError> {
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        match op {
            Op::SetDirection(d) => {
                let StmtKind::Header { dir, keyword_end } = &self.stmts[self.header].kind else { unreachable!() };
                match dir {
                    Some(r) => {
                        let old = &self.src[r.clone()];
                        // Keep TB if that is how the user writes it.
                        let tok = if *d == Direction::TopDown && old == "TB" { "TB" } else { d.token() };
                        edits.push((r.clone(), tok.into()));
                    }
                    None => edits.push((*keyword_end..*keyword_end, format!(" {}", d.token()))),
                }
            }
            Op::SetLabel { id, label } => {
                self.node(id).ok_or_else(|| EditError::NoNode(id.clone()))?;
                match self.def(id) {
                    Some(def) => {
                        let old = &self.src[def.label.clone()];
                        edits.push((def.label.clone(), encode_label(label, old.trim_start().starts_with('"'))));
                    }
                    None => {
                        let r = self.first_ref(id);
                        let (open, close) = Shape::Rect.delimiters();
                        edits.push((r.id_end..r.id_end, format!("{open}{}{close}", encode_label(label, false))));
                    }
                }
            }
            Op::SetShape { id, shape } => {
                let node = self.node(id).ok_or_else(|| EditError::NoNode(id.clone()))?;
                let (open, close) = shape.delimiters();
                match self.def(id) {
                    Some(def) => {
                        let label = &self.src[def.label.clone()];
                        edits.push((def.range.clone(), format!("{open}{label}{close}")));
                    }
                    None => {
                        let r = self.first_ref(id);
                        let label = encode_label(&node.label, false);
                        edits.push((r.id_end..r.id_end, format!("{open}{label}{close}")));
                    }
                }
            }
            Op::AddNode { id, label, shape } => {
                if !valid_id(id) {
                    return Err(EditError::BadId(id.clone()));
                }
                if self.node(id).is_some() {
                    return Err(EditError::Exists(id.clone()));
                }
                let (open, close) = shape.delimiters();
                let label = encode_label(label, false);
                edits.push(self.append(&format!("{id}{open}{label}{close}")));
            }
            Op::AddEdge { from, to, label } => {
                for id in [from, to] {
                    self.node(id).ok_or_else(|| EditError::NoNode(id.clone()))?;
                }
                let label = label.as_deref().filter(|l| !l.is_empty());
                let arrow = match label {
                    Some(l) => format!("-->|{}|", encode_edge_label(l)),
                    None => "-->".into(),
                };
                edits.push(self.append(&format!("{from} {arrow} {to}")));
            }
            Op::SetEdgeLabel { edge, label } => {
                let src = *self.edge_src.get(*edge).ok_or(EditError::NoEdge)?;
                let label = label.as_deref().filter(|l| !l.trim().is_empty());
                if self.link_shared(src) {
                    let mut e = self.edges[*edge].clone();
                    e.label = label.map(str::to_string);
                    edits.push(self.explode(src.stmt, *edge, Some(e)));
                } else {
                    let link = self.link(src);
                    edits.push(self.relabel(link, label));
                }
            }
            Op::SetEdgeStyle { edge, style } => {
                let src = *self.edge_src.get(*edge).ok_or(EditError::NoEdge)?;
                if self.link_shared(src) {
                    let e = Edge { style: *style, ..self.edges[*edge].clone() };
                    edits.push(self.explode(src.stmt, *edge, Some(e)));
                } else {
                    let link = self.link(src);
                    let label = link.label.as_ref().map(|l| &self.src[l.text.clone()]);
                    let text = match label {
                        Some(l) => format!("{}|{}|", style.arrow(), l.trim()),
                        None => style.arrow(),
                    };
                    edits.push((link.range.clone(), text));
                }
            }
            Op::DeleteEdge(edge) => {
                let src = *self.edge_src.get(*edge).ok_or(EditError::NoEdge)?;
                if self.link_shared(src) {
                    edits.push(self.explode(src.stmt, *edge, None));
                } else {
                    edits.extend(self.re_emit(&[src.stmt], &HashSet::new(), Some(src)));
                }
            }
            Op::DeleteNode(id) => {
                self.node(id).ok_or_else(|| EditError::NoNode(id.clone()))?;
                let gone: HashSet<&str> = [id.as_str()].into();
                let touched: Vec<usize> = (0..self.stmts.len())
                    .filter(|&i| {
                        matches!(&self.stmts[i].kind, StmtKind::Chain(c) if c.groups.iter().flatten().any(|r| r.id == *id))
                    })
                    .collect();
                edits.extend(self.re_emit(&touched, &gone, None));
                for (i, s) in self.stmts.iter().enumerate() {
                    match &s.kind {
                        StmtKind::Target(t) if t == id => edits.push(self.delete_stmt(i)),
                        StmtKind::Class(r) => {
                            let ids: Vec<&str> = self.src[r.clone()].split(',').collect();
                            if ids.contains(&id.as_str()) {
                                let kept: Vec<&str> = ids.into_iter().filter(|x| x != id).collect();
                                if kept.is_empty() {
                                    edits.push(self.delete_stmt(i));
                                } else {
                                    edits.push((r.clone(), kept.join(",")));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(apply_edits(&self.src, edits))
    }

    fn def(&self, id: &str) -> Option<&Def> {
        let &(s, g, r) = self.defs.get(id)?;
        self.chain(s).groups[g][r].def.as_ref()
    }

    fn first_ref(&self, id: &str) -> &NodeRef {
        let (s, g, r) = self.first[id];
        &self.chain(s).groups[g][r]
    }

    fn chain(&self, stmt: usize) -> &Chain {
        match &self.stmts[stmt].kind {
            StmtKind::Chain(c) => c,
            _ => unreachable!("not a chain statement"),
        }
    }

    fn link(&self, e: EdgeSrc) -> &Link {
        &self.chain(e.stmt).links[e.link]
    }

    /// Whether the link makes more than one edge (`A & B --> C`).
    fn link_shared(&self, e: EdgeSrc) -> bool {
        let c = self.chain(e.stmt);
        c.groups[e.link].len() * c.groups[e.link + 1].len() > 1
    }

    /// The link with a new label (or none), keeping its arrow.
    fn relabel(&self, link: &Link, label: Option<&str>) -> (Range<usize>, String) {
        match (&link.label, label) {
            (Some(l), Some(text)) => {
                let quoted = self.src[l.text.clone()].trim_start().starts_with('"');
                let text = if l.pipe { encode_edge_label(text) } else { encode_label(text, quoted) };
                (l.text.clone(), text)
            }
            (Some(l), None) if l.pipe => {
                // Remove `|text|` and the space before it.
                let arrow_end = self.src[link.range.start..l.text.start].rfind('|').map_or(l.text.start, |p| {
                    let before = &self.src[link.range.start..link.range.start + p];
                    link.range.start + before.trim_end().len()
                });
                (arrow_end..link.range.end, String::new())
            }
            (Some(_), None) => (link.range.clone(), link.style.arrow()),
            (None, Some(text)) => (link.range.end..link.range.end, format!("|{}|", encode_edge_label(text))),
            (None, None) => (link.range.end..link.range.end, String::new()),
        }
    }

    /// Append a statement on its own line at the end of the diagram.
    fn append(&self, stmt: &str) -> (Range<usize>, String) {
        let end = self.src.trim_end().len();
        let indent = self.indent();
        (end..end, format!("\n{indent}{stmt}"))
    }

    /// Indent of the first statement after the header.
    fn indent(&self) -> String {
        let s = self.stmts.get(self.header + 1).unwrap_or(&self.stmts[self.header]);
        let line_start = self.src[..s.range.start].rfind('\n').map_or(0, |p| p + 1);
        let lead = &self.src[line_start..s.range.start];
        if lead.chars().all(char::is_whitespace) && !lead.is_empty() { lead.to_string() } else { "    ".into() }
    }

    /// Remove a whole statement, and its line when nothing else is on it.
    fn delete_stmt(&self, i: usize) -> (Range<usize>, String) {
        let r = self.stmts[i].range.clone();
        let line_start = self.src[..r.start].rfind('\n').map_or(0, |p| p + 1);
        let line_end = self.src[r.end..].find('\n').map_or(self.src.len(), |p| r.end + p);
        let before = &self.src[line_start..r.start];
        let after = &self.src[r.end..line_end];
        let after_trim = after.trim_start().strip_prefix(';').unwrap_or(after.trim_start());
        if before.trim().is_empty() && after_trim.trim().is_empty() {
            // The whole line, with its newline (or the one before at EOF).
            if line_end < self.src.len() {
                (line_start..line_end + 1, String::new())
            } else if line_start > 0 {
                (line_start - 1..line_end, String::new())
            } else {
                (line_start..line_end, String::new())
            }
        } else {
            match after.find(';').filter(|p| after[..*p].trim().is_empty()) {
                Some(p) => {
                    let rest = &after[p + 1..];
                    let ws = rest.len() - rest.trim_start().len();
                    (r.start..r.end + p + 1 + ws, String::new())
                }
                // Last on its line: take the `;` before it instead.
                None => {
                    let start = self.src[line_start..r.start].trim_end().strip_suffix(';').map_or(r.start, |b| {
                        let b = b.trim_end();
                        line_start + b.len()
                    });
                    (start..r.end, String::new())
                }
            }
        }
    }

    /// Separator between statements replacing one statement.
    fn stmt_sep(&self, i: usize) -> String {
        let r = &self.stmts[i].range;
        let line_start = self.src[..r.start].rfind('\n').map_or(0, |p| p + 1);
        let line_end = self.src[r.end..].find('\n').map_or(self.src.len(), |p| r.end + p);
        let alone = self.src[line_start..r.start].trim().is_empty()
            && self.src[r.end..line_end].trim().trim_start_matches(';').trim().is_empty();
        if alone { format!("\n{}", &self.src[line_start..r.start]) } else { "; ".into() }
    }

    /// Nodes mentioned anywhere but statement `except`.
    fn mentioned_elsewhere(&self, id: &str, except: usize) -> bool {
        self.stmts.iter().enumerate().any(|(i, s)| {
            i != except && matches!(&s.kind, StmtKind::Chain(c) if c.groups.iter().flatten().any(|r| r.id == id))
        })
    }

    /// Re-emit chain statements without the nodes in `gone` and (for the
    /// statement of `cut`) without that link, reusing the original text of
    /// everything kept. A group left on its own is kept only when it defines
    /// a node or the node would otherwise vanish.
    fn re_emit(&self, touched: &[usize], gone: &HashSet<&str>, cut: Option<EdgeSrc>) -> Vec<(Range<usize>, String)> {
        // Per statement: kept refs of each group, and runs of linked groups.
        type Plan<'a> = (usize, Vec<Vec<&'a NodeRef>>, Vec<Vec<usize>>);
        let plans: Vec<Plan> = touched
            .iter()
            .map(|&i| {
                let c = self.chain(i);
                let cut = cut.filter(|e| e.stmt == i).map(|e| e.link);
                let groups: Vec<Vec<&NodeRef>> =
                    c.groups.iter().map(|g| g.iter().filter(|r| !gone.contains(r.id.as_str())).collect()).collect();
                // Runs of non-empty groups joined by surviving links.
                let mut runs: Vec<Vec<usize>> = Vec::new();
                let mut run: Vec<usize> = Vec::new();
                for (g, group) in groups.iter().enumerate() {
                    if group.is_empty() {
                        runs.extend((!run.is_empty()).then(|| std::mem::take(&mut run)));
                        continue;
                    }
                    if !run.is_empty() && Some(g - 1) == cut {
                        runs.push(std::mem::take(&mut run));
                    }
                    run.push(g);
                }
                runs.extend((!run.is_empty()).then_some(run));
                (i, groups, runs)
            })
            .collect();
        // Nodes that stay mentioned: untouched statements and kept links.
        let mut emitted: HashSet<&str> = HashSet::new();
        for (i, s) in self.stmts.iter().enumerate() {
            if let StmtKind::Chain(c) = &s.kind
                && !touched.contains(&i)
            {
                emitted.extend(c.groups.iter().flatten().map(|r| r.id.as_str()));
            }
        }
        for (_, groups, runs) in &plans {
            for r in runs.iter().filter(|r| r.len() > 1) {
                emitted.extend(r.iter().flat_map(|g| groups[*g].iter().map(|n| n.id.as_str())));
            }
        }
        let mut out = Vec::new();
        for (i, groups, runs) in &plans {
            let c = self.chain(*i);
            let mut segments: Vec<String> = Vec::new();
            for r in runs {
                if r.len() == 1 {
                    let mut kept = Vec::new();
                    for n in &groups[r[0]] {
                        if n.def.is_some() || !emitted.contains(n.id.as_str()) {
                            emitted.insert(&n.id);
                            kept.push(&self.src[n.range.clone()]);
                        }
                    }
                    if !kept.is_empty() {
                        segments.push(kept.join(" & "));
                    }
                    continue;
                }
                let mut text = String::new();
                for (k, g) in r.iter().enumerate() {
                    if k > 0 {
                        text.push(' ');
                        text.push_str(&self.src[c.links[g - 1].range.clone()]);
                        text.push(' ');
                    }
                    text.push_str(&self.group_text(c, *g, &groups[*g]));
                }
                segments.push(text);
            }
            out.push(if segments.is_empty() {
                self.delete_stmt(*i)
            } else {
                (self.stmts[*i].range.clone(), segments.join(&self.stmt_sep(*i)))
            });
        }
        out
    }

    fn group_text(&self, c: &Chain, g: usize, kept: &[&NodeRef]) -> String {
        if kept.len() == c.groups[g].len() {
            let first = &c.groups[g][0];
            let last = &c.groups[g][c.groups[g].len() - 1];
            return self.src[first.range.start..last.range.end].to_string();
        }
        kept.iter().map(|r| &self.src[r.range.clone()]).collect::<Vec<_>>().join(" & ")
    }

    /// Replace a chain with one line per edge, dropping or replacing edge
    /// `target` (for edits to one edge of a shared link).
    fn explode(&self, i: usize, target: usize, replace: Option<Edge>) -> (Range<usize>, String) {
        let c = self.chain(i);
        let mut lines = Vec::new();
        let mut written: HashSet<&str> = HashSet::new();
        // The first time a node is written it carries its definition.
        let text_of = |r: &NodeRef, written: &HashSet<&str>| -> String {
            if r.def.is_some() && !written.contains(r.id.as_str()) {
                self.src[r.range.clone()].to_string()
            } else {
                r.id.clone()
            }
        };
        let mut at = self.edge_src.iter().position(|s| s.stmt == i).unwrap_or(0);
        for (l, link) in c.links.iter().enumerate() {
            for a in &c.groups[l] {
                for b in &c.groups[l + 1] {
                    let this = at;
                    at += 1;
                    let arrow = if this == target {
                        let Some(edge) = &replace else { continue };
                        match edge.label.as_deref().filter(|l| !l.is_empty()) {
                            Some(l) => format!("{}|{}|", edge.style.arrow(), encode_edge_label(l)),
                            None => edge.style.arrow(),
                        }
                    } else {
                        match &link.label {
                            Some(l) if !l.pipe => {
                                let text = decode_label(&self.src[l.text.clone()]);
                                format!("{}|{}|", link.style.arrow(), encode_edge_label(&text))
                            }
                            _ => self.src[link.range.clone()].to_string(),
                        }
                    };
                    let left = text_of(a, &written);
                    written.insert(&a.id);
                    let right = text_of(b, &written);
                    written.insert(&b.id);
                    lines.push(format!("{left} {arrow} {right}"));
                }
            }
        }
        // Definitions and nodes that would otherwise vanish.
        for r in c.groups.iter().flatten() {
            if !written.contains(r.id.as_str()) && (r.def.is_some() || !self.mentioned_elsewhere(&r.id, i)) {
                lines.push(self.src[r.range.clone()].to_string());
                written.insert(&r.id);
            }
        }
        (self.stmts[i].range.clone(), lines.join(&self.stmt_sep(i)))
    }
}

fn apply_edits(src: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    // Neighbouring deletions may overlap (`A; B` losing both): merge them.
    edits.sort_by_key(|(r, _)| (r.start, r.end));
    let mut merged: Vec<(Range<usize>, String)> = Vec::new();
    for (r, text) in edits {
        if let Some((prev, prev_text)) = merged.last_mut()
            && prev_text.is_empty()
            && text.is_empty()
            && r.start < prev.end
        {
            prev.end = prev.end.max(r.end);
            continue;
        }
        merged.push((r, text));
    }
    // A deletion that ends its line takes the `;` before it.
    for (r, text) in &mut merged {
        let line_end = src[r.end..].find('\n').map_or(src.len(), |p| r.end + p);
        if text.is_empty() && src[r.end..line_end].trim().is_empty() {
            let before = src[..r.start].trim_end_matches([' ', '\t']);
            if let Some(b) = before.strip_suffix(';') {
                r.start = b.trim_end_matches([' ', '\t']).len();
            }
        }
    }
    let mut edits = merged;
    edits.reverse();
    let mut out = src.to_string();
    let mut last = usize::MAX;
    for (r, text) in edits {
        // Overlapping edits (a statement re-emitted twice) keep the first.
        if r.end > last {
            continue;
        }
        out.replace_range(r.clone(), &text);
        last = r.start;
    }
    out
}

// ---------------------------------------------------------------------------
// Labels

fn decode_label(raw: &str) -> String {
    let t = raw.trim();
    let t = t.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(t);
    let t = t.strip_prefix('`').and_then(|s| s.strip_suffix('`')).unwrap_or(t);
    t.replace("#quot;", "\"")
}

/// A node label for the source: quoted when it holds characters that would
/// end it early (or when the old one was quoted).
fn encode_label(label: &str, quoted: bool) -> String {
    let special = label.chars().any(|c| "[](){}|\"<>;#&%".contains(c))
        || label.starts_with([' ', '/', '\\'])
        || label.ends_with(' ');
    if quoted || special { format!("\"{}\"", label.replace('"', "#quot;")) } else { label.to_string() }
}

fn encode_edge_label(label: &str) -> String {
    if label.contains('|') || label.contains('"') {
        format!("\"{}\"", label.replace('"', "#quot;"))
    } else {
        label.to_string()
    }
}

pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(is_id_char)
        && id.chars().all(|c| is_id_char(c) || c == '-' || c == '.')
        && !id.ends_with(['-', '.'])
        && !id.contains("--")
        && !matches!(id, "end" | "subgraph" | "graph" | "flowchart" | "style" | "class" | "click")
}

fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn subgraph_title(rest: &str) -> (String, String) {
    let id_len = rest.find(|c: char| !(is_id_char(c) || c == '-' || c == '.')).unwrap_or(rest.len());
    let id = &rest[..id_len];
    let after = rest[id_len..].trim();
    if !id.is_empty() && after.starts_with('[') && after.ends_with(']') {
        return (id.to_string(), decode_label(&after[1..after.len() - 1]));
    }
    let title = decode_label(rest);
    (if id_len == rest.len() { id.to_string() } else { title.clone() }, title)
}

// ---------------------------------------------------------------------------
// Statements

struct RawStmt {
    range: Range<usize>,
}

fn is_comment(text: &str) -> bool {
    text.trim_start().starts_with("%%")
}

/// Split the source into statements (lines, and `;` outside quotes and
/// brackets), trimmed, skipping a `---` front matter block.
fn statements(src: &str) -> Vec<RawStmt> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut in_front = false;
    let mut first_line = true;
    for line in src.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        if first_line && body.trim() == "---" {
            in_front = true;
            first_line = false;
            continue;
        }
        first_line = first_line && body.trim().is_empty();
        if in_front {
            if body.trim() == "---" {
                in_front = false;
            }
            continue;
        }
        if is_comment(body) {
            push_trimmed(&mut out, src, start, start + body.len());
            continue;
        }
        let mut depth = 0i32;
        let mut quote = false;
        let mut s = start;
        for (k, ch) in body.char_indices() {
            match ch {
                '"' => quote = !quote,
                '[' | '(' | '{' if !quote => depth += 1,
                ']' | ')' | '}' if !quote => depth -= 1,
                ';' if !quote && depth <= 0 => {
                    push_trimmed(&mut out, src, s, start + k);
                    s = start + k + 1;
                }
                _ => {}
            }
        }
        push_trimmed(&mut out, src, s, start + body.len());
    }
    out
}

fn push_trimmed(out: &mut Vec<RawStmt>, src: &str, start: usize, end: usize) {
    let text = &src[start..end];
    let lead = text.len() - text.trim_start().len();
    let t = text.trim();
    if !t.is_empty() {
        out.push(RawStmt { range: start + lead..start + lead + t.len() });
    }
}

// ---------------------------------------------------------------------------
// Chains: `A[x] & B --> C -->|y| D`

struct Cursor<'a> {
    src: &'a str,
    pos: usize,
    end: usize,
}

impl Cursor<'_> {
    fn rest(&self) -> &str {
        &self.src[self.pos..self.end]
    }

    fn skip_ws(&mut self) {
        let r = self.rest();
        self.pos += r.len() - r.trim_start().len();
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.rest().starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }
}

fn parse_chain(src: &str, range: Range<usize>) -> Option<Chain> {
    let mut c = Cursor { src, pos: range.start, end: range.end };
    let mut groups = vec![parse_group(&mut c)?];
    let mut links = Vec::new();
    loop {
        c.skip_ws();
        if c.pos >= c.end {
            break;
        }
        links.push(parse_link(&mut c)?);
        c.skip_ws();
        groups.push(parse_group(&mut c)?);
    }
    Some(Chain { groups, links })
}

fn parse_group(c: &mut Cursor<'_>) -> Option<Vec<NodeRef>> {
    let mut refs = vec![parse_ref(c)?];
    loop {
        let save = c.pos;
        c.skip_ws();
        if c.eat("&") {
            c.skip_ws();
            refs.push(parse_ref(c)?);
        } else {
            c.pos = save;
            return Some(refs);
        }
    }
}

fn parse_ref(c: &mut Cursor<'_>) -> Option<NodeRef> {
    let start = c.pos;
    let rest = c.rest();
    let mut len = 0;
    let mut chars = rest.char_indices().peekable();
    while let Some((k, ch)) = chars.next() {
        let next = chars.peek().map(|(_, n)| *n);
        let ok = is_id_char(ch) || (k > 0 && matches!(ch, '-' | '.') && next.is_some_and(is_id_char));
        if !ok {
            break;
        }
        len = k + ch.len_utf8();
    }
    if len == 0 {
        return None;
    }
    let id = rest[..len].to_string();
    if matches!(id.as_str(), "end" | "subgraph") {
        return None;
    }
    c.pos += len;
    let id_end = c.pos;
    let def = parse_def(c);
    if def.is_none() && c.rest().starts_with("@{") {
        // New-style `A@{ shape: ... }`: kept, not understood.
        let close = c.rest().find('}')?;
        c.pos += close + 1;
    }
    if c.rest().starts_with(":::") {
        c.pos += 3;
        let r = c.rest();
        c.pos += r.find(|ch: char| !(is_id_char(ch) || ch == '-')).unwrap_or(r.len());
    }
    Some(NodeRef { id, range: start..c.pos, id_end, def })
}

fn parse_def(c: &mut Cursor<'_>) -> Option<Def> {
    const OPENERS: [(&str, &[(&str, Shape)]); 12] = [
        ("(((", &[(")))", Shape::DoubleCircle)]),
        ("((", &[("))", Shape::Circle)]),
        ("([", &[("])", Shape::Stadium)]),
        ("(", &[(")", Shape::Round)]),
        ("[[", &[("]]", Shape::Subroutine)]),
        ("[(", &[(")]", Shape::Cylinder)]),
        ("[/", &[("/]", Shape::Parallelogram), ("\\]", Shape::Trapezoid)]),
        ("[\\", &[("\\]", Shape::ParallelogramAlt), ("/]", Shape::TrapezoidAlt)]),
        ("[", &[("]", Shape::Rect)]),
        ("{{", &[("}}", Shape::Hexagon)]),
        ("{", &[("}", Shape::Diamond)]),
        (">", &[("]", Shape::Asymmetric)]),
    ];
    let start = c.pos;
    for (open, closers) in OPENERS {
        if !c.rest().starts_with(open) {
            continue;
        }
        let body = start + open.len();
        let text = &c.src[body..c.end];
        // Quoted label: up to the closing quote, then the closer.
        let label_end = if text.trim_start().starts_with('"') {
            let q = text.find('"')?;
            let close = text[q + 1..].find('"')? + q + 2;
            let tail = &text[close..];
            close + (tail.len() - tail.trim_start().len())
        } else {
            closers.iter().filter_map(|(cl, _)| text.find(cl)).min()?
        };
        let Some((closer, shape)) = closers.iter().find(|(cl, _)| text[label_end..].starts_with(cl)) else {
            continue;
        };
        let end = body + label_end + closer.len();
        c.pos = end;
        return Some(Def { shape: *shape, range: start..end, label: body..body + label_end });
    }
    None
}

fn parse_link(c: &mut Cursor<'_>) -> Option<Link> {
    let start = c.pos;
    let rest = c.rest();
    let bytes = rest.as_bytes();
    // Optional start head.
    let mut k = 0;
    if matches!(bytes.first(), Some(b'<'))
        || (matches!(bytes.first(), Some(b'o' | b'x')) && matches!(bytes.get(1), Some(b'-' | b'=')))
    {
        k = 1;
    }
    let body = &rest[k..];
    // Inline text: `-- text -->`, `== text ==>`, `-. text .->`.
    for (open, closers) in [("--", ["-->", "---", "--o", "--x"]), ("==", ["==>", "===", "==o", "==x"])] {
        if body.starts_with(open) && body[open.len()..].starts_with([' ', '"']) {
            let after = &body[open.len()..];
            if let Some((p, cl)) = closers
                .iter()
                .filter_map(|cl| after.find(&format!(" {cl}")).map(|p| (p + 1, *cl)))
                .min_by_key(|(p, _)| *p)
            {
                let text_start = start + k + open.len();
                let text = text_start..text_start + p;
                let mut end = text.end + cl.len();
                // Longer closers: `---->`.
                let tail = &c.src[end..c.end];
                let extra = tail.len() - tail.trim_start_matches(['-', '=']).len();
                end += extra;
                if extra > 0 && matches!(c.src[end..c.end].chars().next(), Some('>' | 'o' | 'x')) {
                    end += 1;
                }
                let arrow = format!("{}{}", &rest[..k], c.src[text.end..end].trim());
                c.pos = end;
                let text = trim_range(c.src, text);
                return Some(Link {
                    range: start..end,
                    style: EdgeStyle::from_arrow(&arrow),
                    label: Some(LinkLabel { text, pipe: false }),
                });
            }
        }
    }
    if body.starts_with("-.") && body[2..].starts_with(' ') {
        let after = &body[2..];
        if let Some(p) = after.find(" .-") {
            let text_start = start + k + 2;
            let text = text_start..text_start + p + 1;
            let mut end = text.end + 2;
            let tail = &c.src[end..c.end];
            end += tail.len() - tail.trim_start_matches('-').len();
            if matches!(c.src[end..c.end].chars().next(), Some('>' | 'o' | 'x')) {
                end += 1;
            }
            let mut style = EdgeStyle::from_arrow(&format!("-.{}", &c.src[end - 1..end]));
            style.both = k == 1 && style.head != Head::None;
            c.pos = end;
            return Some(Link {
                range: start..end,
                style,
                label: Some(LinkLabel { text: trim_range(c.src, text), pipe: false }),
            });
        }
    }
    // Plain arrow.
    let b = body.as_bytes();
    let mut n = 0;
    match b.first()? {
        b'~' => {
            while b.get(n) == Some(&b'~') {
                n += 1;
            }
            if n < 3 {
                return None;
            }
        }
        b'=' => {
            while b.get(n) == Some(&b'=') {
                n += 1;
            }
        }
        b'-' => {
            n = 1;
            while matches!(b.get(n), Some(b'-' | b'.')) {
                n += 1;
            }
        }
        _ => return None,
    }
    let head = match b.get(n) {
        Some(b'>') => 1,
        // `--o B`, not `--oB` (a node called oB).
        Some(b'o' | b'x') if !b.get(n + 1).is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'_') => 1,
        _ => 0,
    };
    let valid = match b[0] {
        b'~' => head == 0,
        b'=' => n + head >= 3,
        // `-->`, `---`, `-.->`, `-.-`; not `--` or `-.`.
        _ => n + head >= 3 && b[n - 1] == b'-',
    };
    if !valid {
        return None;
    }
    let arrow_end = start + k + n + head;
    let arrow = &c.src[start..arrow_end];
    c.pos = arrow_end;
    let style = EdgeStyle::from_arrow(arrow);
    // `-->|text|`
    let save = c.pos;
    c.skip_ws();
    if c.rest().starts_with('|') {
        let r = c.rest();
        if let Some(close) = r[1..].find('|') {
            let text = c.pos + 1..c.pos + 1 + close;
            c.pos += close + 2;
            return Some(Link { range: start..c.pos, style, label: Some(LinkLabel { text, pipe: true }) });
        }
    }
    c.pos = save;
    Some(Link { range: start..arrow_end, style, label: None })
}

fn trim_range(src: &str, r: Range<usize>) -> Range<usize> {
    let t = &src[r.clone()];
    let lead = t.len() - t.trim_start().len();
    r.start + lead..r.start + lead + t.trim().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fc(src: &str) -> Flowchart {
        Flowchart::parse(src).unwrap()
    }

    fn edges(f: &Flowchart) -> Vec<(String, String, Option<String>)> {
        f.edges.iter().map(|e| (e.from.clone(), e.to.clone(), e.label.clone())).collect()
    }

    fn e(a: &str, b: &str, l: Option<&str>) -> (String, String, Option<String>) {
        (a.into(), b.into(), l.map(Into::into))
    }

    #[test]
    fn parses_nodes_edges_and_shapes() {
        let f = fc("flowchart LR\n  A[Start] --> B{Is it?}\n  B -->|Yes| C((OK))\n  B -- No --> D[(Store)]\n");
        assert_eq!(f.direction, Direction::LeftRight);
        let shapes: Vec<_> = f.nodes.iter().map(|n| (n.id.as_str(), n.label.as_str(), n.shape)).collect();
        assert_eq!(
            shapes,
            [
                ("A", "Start", Shape::Rect),
                ("B", "Is it?", Shape::Diamond),
                ("C", "OK", Shape::Circle),
                ("D", "Store", Shape::Cylinder)
            ]
        );
        assert_eq!(edges(&f), [e("A", "B", None), e("B", "C", Some("Yes")), e("B", "D", Some("No"))]);
    }

    #[test]
    fn every_shape_round_trips() {
        for shape in Shape::ALL {
            let (o, c) = shape.delimiters();
            let f = fc(&format!("graph TD\nX{o}Hello{c} --> Y"));
            assert_eq!(f.nodes[0].shape, shape, "{o}{c}");
            assert_eq!(f.nodes[0].label, "Hello");
        }
    }

    #[test]
    fn arrows_chains_groups_and_semicolons() {
        let f = fc("graph TD;A-->B;B-.->C & D==>E\nE --- F\nF ~~~ G\nG <--> H\nH --o I\nI x--x J\nA-.text.->J");
        let got: Vec<_> = f.edges.iter().map(|e| (e.from.as_str(), e.to.as_str(), e.style)).collect();
        let s = |line, head, both| EdgeStyle { line, head, both };
        assert_eq!(got[0], ("A", "B", s(Line::Solid, Head::Arrow, false)));
        assert_eq!(got[1], ("B", "C", s(Line::Dotted, Head::Arrow, false)));
        assert_eq!(got[2], ("B", "D", s(Line::Dotted, Head::Arrow, false)));
        assert_eq!(got[3], ("C", "E", s(Line::Thick, Head::Arrow, false)));
        assert_eq!(got[4], ("D", "E", s(Line::Thick, Head::Arrow, false)));
        assert_eq!(got[5], ("E", "F", s(Line::Solid, Head::None, false)));
        assert_eq!(got[6], ("F", "G", s(Line::Invisible, Head::None, false)));
        assert_eq!(got[7], ("G", "H", s(Line::Solid, Head::Arrow, true)));
        assert_eq!(got[8], ("H", "I", s(Line::Solid, Head::Circle, false)));
        assert_eq!(got[9], ("I", "J", s(Line::Solid, Head::Cross, true)));
        assert_eq!(f.nodes.len(), 10);
    }

    #[test]
    fn subgraphs_comments_and_styles_are_kept() {
        let src = "%% top\nflowchart TB\n  subgraph one [First]\n    A --> B\n  end\n  classDef hot fill:#f00\n  class A hot\n  style B fill:#0f0\n  B --> C\n";
        let f = fc(src);
        assert_eq!(f.subgraphs, [Subgraph { id: "one".into(), title: "First".into(), parent: None }]);
        assert_eq!(f.node("A").unwrap().subgraph, Some(0));
        assert_eq!(f.node("C").unwrap().subgraph, None);
        assert_eq!(f.direction, Direction::TopDown);
    }

    #[test]
    fn not_a_flowchart() {
        assert!(Flowchart::parse("sequenceDiagram\n A->>B: hi").is_err());
        assert!(!is_flowchart("pie\n \"a\": 1"));
        assert!(is_flowchart("%% c\ngraph LR\nA-->B"));
        assert!(is_flowchart("---\ntitle: x\n---\nflowchart TD\nA-->B"));
    }

    #[test]
    fn direction_edits_only_the_token() {
        let f = fc("graph TD\n  A --> B");
        assert_eq!(f.apply(&Op::SetDirection(Direction::LeftRight)).unwrap(), "graph LR\n  A --> B");
        let f = fc("flowchart\n  A --> B");
        assert_eq!(f.apply(&Op::SetDirection(Direction::RightLeft)).unwrap(), "flowchart RL\n  A --> B");
        let f = fc("graph LR\nA-->B");
        assert_eq!(f.apply(&Op::SetDirection(Direction::TopDown)).unwrap(), "graph TD\nA-->B");
    }

    #[test]
    fn label_and_shape_edits_touch_the_definition() {
        let src = "flowchart LR\n  A[Start] --> B\n  %% keep me\n  B --> C";
        let f = fc(src);
        let out = f.apply(&Op::SetLabel { id: "A".into(), label: "Begin here".into() }).unwrap();
        assert_eq!(out, "flowchart LR\n  A[Begin here] --> B\n  %% keep me\n  B --> C");
        let out = f.apply(&Op::SetLabel { id: "B".into(), label: "Check (x)".into() }).unwrap();
        assert_eq!(out, "flowchart LR\n  A[Start] --> B[\"Check (x)\"]\n  %% keep me\n  B --> C");
        let out = f.apply(&Op::SetShape { id: "A".into(), shape: Shape::Diamond }).unwrap();
        assert_eq!(out, "flowchart LR\n  A{Start} --> B\n  %% keep me\n  B --> C");
        let out = f.apply(&Op::SetShape { id: "C".into(), shape: Shape::Circle }).unwrap();
        assert_eq!(out, "flowchart LR\n  A[Start] --> B\n  %% keep me\n  B --> C((C))");
        assert_eq!(fc(&out).node("C").unwrap().shape, Shape::Circle);
    }

    #[test]
    fn quoted_labels_stay_quoted() {
        let f = fc("graph TD\nA[\"Say (hi)\"]");
        assert_eq!(f.nodes[0].label, "Say (hi)");
        let out = f.apply(&Op::SetLabel { id: "A".into(), label: "plain".into() }).unwrap();
        assert_eq!(out, "graph TD\nA[\"plain\"]");
        let out = f.apply(&Op::SetLabel { id: "A".into(), label: "a \"q\"".into() }).unwrap();
        assert_eq!(fc(&out).nodes[0].label, "a \"q\"");
    }

    #[test]
    fn add_node_and_edge_append_lines_with_the_same_indent() {
        let f = fc("flowchart TD\n    A --> B\n");
        let out = f.apply(&Op::AddNode { id: "N1".into(), label: "New".into(), shape: Shape::Round }).unwrap();
        assert_eq!(out, "flowchart TD\n    A --> B\n    N1(New)\n");
        let f = fc(&out);
        let out = f.apply(&Op::AddEdge { from: "B".into(), to: "N1".into(), label: Some("go".into()) }).unwrap();
        assert_eq!(out, "flowchart TD\n    A --> B\n    N1(New)\n    B -->|go| N1\n");
        assert_eq!(f.fresh_id(), "N2");
        assert_eq!(
            f.apply(&Op::AddNode { id: "A".into(), label: "x".into(), shape: Shape::Rect }),
            Err(EditError::Exists("A".into()))
        );
        assert!(f.apply(&Op::AddNode { id: "a b".into(), label: "x".into(), shape: Shape::Rect }).is_err());
    }

    #[test]
    fn edge_labels() {
        let f = fc("graph LR\nA -->|old| B\nB -- inline --> C\nC --> D");
        let out = f.apply(&Op::SetEdgeLabel { edge: 0, label: Some("new".into()) }).unwrap();
        assert_eq!(out, "graph LR\nA -->|new| B\nB -- inline --> C\nC --> D");
        let out = f.apply(&Op::SetEdgeLabel { edge: 0, label: None }).unwrap();
        assert_eq!(out, "graph LR\nA --> B\nB -- inline --> C\nC --> D");
        let out = f.apply(&Op::SetEdgeLabel { edge: 1, label: Some("changed".into()) }).unwrap();
        assert_eq!(out, "graph LR\nA -->|old| B\nB -- changed --> C\nC --> D");
        let out = f.apply(&Op::SetEdgeLabel { edge: 1, label: None }).unwrap();
        assert_eq!(out, "graph LR\nA -->|old| B\nB --> C\nC --> D");
        let out = f.apply(&Op::SetEdgeLabel { edge: 2, label: Some("added".into()) }).unwrap();
        assert_eq!(out, "graph LR\nA -->|old| B\nB -- inline --> C\nC -->|added| D");
        for (i, expect) in [(0, Some("old")), (1, Some("inline")), (2, None)] {
            assert_eq!(f.edges[i].label.as_deref(), expect);
        }
    }

    #[test]
    fn edge_styles() {
        let f = fc("graph LR\nA -->|x| B\nB --> C");
        let dotted = EdgeStyle { line: Line::Dotted, head: Head::Arrow, both: false };
        let out = f.apply(&Op::SetEdgeStyle { edge: 0, style: dotted }).unwrap();
        assert_eq!(out, "graph LR\nA -.->|x| B\nB --> C");
        let thick_open = EdgeStyle { line: Line::Thick, head: Head::None, both: false };
        let out = f.apply(&Op::SetEdgeStyle { edge: 1, style: thick_open }).unwrap();
        assert_eq!(out, "graph LR\nA -->|x| B\nB === C");
        for style in [
            dotted,
            thick_open,
            EdgeStyle { line: Line::Solid, head: Head::Circle, both: true },
            EdgeStyle { line: Line::Dotted, head: Head::None, both: false },
            EdgeStyle { line: Line::Invisible, head: Head::None, both: false },
            EdgeStyle { line: Line::Thick, head: Head::Cross, both: false },
        ] {
            let out = f.apply(&Op::SetEdgeStyle { edge: 1, style }).unwrap();
            assert_eq!(fc(&out).edges[1].style, style, "{out}");
        }
    }

    #[test]
    fn delete_edge_keeps_nodes_and_definitions() {
        let f = fc("graph TD\n  A[Start] --> B[End]\n  B --> C");
        assert_eq!(f.apply(&Op::DeleteEdge(0)).unwrap(), "graph TD\n  A[Start]\n  B[End]\n  B --> C");
        // A bare node mentioned elsewhere goes away with its edge.
        let f = fc("graph TD\n  A --> B\n  B --> C\n  A --> C");
        assert_eq!(f.apply(&Op::DeleteEdge(0)).unwrap(), "graph TD\n  B --> C\n  A --> C");
        // A chain is split.
        let f = fc("graph TD\n  A --> B --> C --> D");
        let out = f.apply(&Op::DeleteEdge(1)).unwrap();
        assert_eq!(out, "graph TD\n  A --> B\n  C --> D");
        assert_eq!(edges(&fc(&out)), [e("A", "B", None), e("C", "D", None)]);
    }

    #[test]
    fn delete_edge_of_a_shared_link() {
        let f = fc("graph TD\n  A & B -->|go| C");
        let out = f.apply(&Op::DeleteEdge(1)).unwrap();
        assert_eq!(edges(&fc(&out)), [e("A", "C", Some("go"))]);
        assert!(fc(&out).node("B").is_some(), "{out}");
        let out = f.apply(&Op::SetEdgeLabel { edge: 0, label: Some("x".into()) }).unwrap();
        assert_eq!(edges(&fc(&out)), [e("A", "C", Some("x")), e("B", "C", Some("go"))], "{out}");
    }

    #[test]
    fn delete_node_removes_edges_styles_and_class_entries() {
        let src = "graph TD\n  A[Start] --> B --> C\n  B --> D\n  class A,B hot\n  style B fill:#f00\n  click B callback\n  D --> E";
        let f = fc(src);
        let out = f.apply(&Op::DeleteNode("B".into())).unwrap();
        // D still has an edge, so its bare mention goes.
        assert_eq!(out, "graph TD\n  A[Start]\n  C\n  class A hot\n  D --> E");
        let g = fc(&out);
        assert!(g.node("B").is_none());
        assert!(g.edges.iter().all(|e| e.from != "B" && e.to != "B"));
        // Nodes left without a mention elsewhere stay.
        assert!(g.node("C").is_some());
    }

    #[test]
    fn delete_node_on_shared_lines() {
        let f = fc("graph TD; A --> B; B --> C; C --> A");
        let out = f.apply(&Op::DeleteNode("C".into())).unwrap();
        assert_eq!(out, "graph TD; A --> B");
        let f = fc("graph TD\n  A & B --> C\n  C --> A");
        let out = f.apply(&Op::DeleteNode("B".into())).unwrap();
        assert_eq!(out, "graph TD\n  A --> C\n  C --> A");
    }

    #[test]
    fn unknown_statements_survive_edits() {
        let src = "flowchart LR\n  A --> B\n  linkStyle 0 stroke:#f00\n  weird ::: stuff\n";
        let f = fc(src);
        let out = f.apply(&Op::SetLabel { id: "A".into(), label: "x".into() }).unwrap();
        assert_eq!(out, "flowchart LR\n  A[x] --> B\n  linkStyle 0 stroke:#f00\n  weird ::: stuff\n");
    }

    #[test]
    fn edited_sources_render() {
        let style = crate::Style::from_theme(&mdedit_theme::Theme::ember_light());
        let f = fc("flowchart LR\n  A[Start] --> B{Is it?}\n  B -->|Yes| C[OK]\n  B -- No --> D[Rethink]");
        let ops = [
            Op::SetShape { id: "C".into(), shape: Shape::Hexagon },
            Op::AddNode { id: "N1".into(), label: "New (one)".into(), shape: Shape::Stadium },
            Op::DeleteNode("B".into()),
            Op::SetDirection(Direction::BottomUp),
            Op::SetEdgeStyle { edge: 2, style: EdgeStyle { line: Line::Thick, head: Head::Circle, both: true } },
        ];
        for op in ops {
            let out = f.apply(&op).unwrap();
            crate::render(&out, &style).unwrap_or_else(|e| panic!("{op:?}: {e}\n{out}"));
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(300))]
        /// Random sequences of edits keep the source parseable and the model
        /// consistent with what was asked.
        #[test]
        fn random_edits_stay_consistent(ops in proptest::collection::vec((0u8..7, 0usize..16, 0usize..16), 1..12)) {
            let mut src = String::from("flowchart TD\n  A[Start] --> B{Choice}\n  B -->|yes| C & D\n  %% note\n  C -- no --> A\n");
            for (kind, x, y) in ops {
                let f = Flowchart::parse(&src).unwrap();
                let n = f.nodes.len().max(1);
                let id = |k: usize| f.nodes.get(k % n).map(|n| n.id.clone()).unwrap_or_default();
                let op = match kind {
                    0 => Op::AddNode { id: f.fresh_id(), label: format!("n {x}"), shape: Shape::ALL[y % 14] },
                    1 => Op::AddEdge { from: id(x), to: id(y), label: (y % 2 == 0).then(|| "l".into()) },
                    2 => Op::DeleteNode(id(x)),
                    3 => Op::DeleteEdge(x % f.edges.len().max(1)),
                    4 => Op::SetLabel { id: id(x), label: format!("L{y} (x)") },
                    5 => Op::SetShape { id: id(x), shape: Shape::ALL[y % 14] },
                    _ => Op::SetEdgeLabel { edge: x % f.edges.len().max(1), label: (y % 3 != 0).then(|| format!("e{y}")) },
                };
                let Ok(out) = f.apply(&op) else { continue };
                let g = Flowchart::parse(&out).unwrap();
                proptest::prop_assert!(out.contains("%% note"));
                match &op {
                    Op::AddNode { id, label, shape } => {
                        let node = g.node(id).unwrap();
                        proptest::prop_assert_eq!(&node.label, label);
                        proptest::prop_assert_eq!(node.shape, *shape);
                        proptest::prop_assert_eq!(g.edges.len(), f.edges.len());
                    }
                    Op::AddEdge { .. } => proptest::prop_assert_eq!(g.edges.len(), f.edges.len() + 1),
                    Op::DeleteNode(id) => {
                        proptest::prop_assert!(g.node(id).is_none(), "{}", out);
                        let left = f.edges.iter().filter(|e| e.from != *id && e.to != *id).count();
                        proptest::prop_assert_eq!(g.edges.len(), left, "{}", out);
                        proptest::prop_assert_eq!(g.nodes.len(), f.nodes.len() - 1, "{}", out);
                    }
                    Op::DeleteEdge(_) => {
                        proptest::prop_assert_eq!(g.edges.len(), f.edges.len() - 1, "{}", out);
                        proptest::prop_assert_eq!(g.nodes.len(), f.nodes.len(), "{}", out);
                    }
                    Op::SetLabel { id, label } => {
                        proptest::prop_assert_eq!(&g.node(id).unwrap().label, label, "{}", out);
                        proptest::prop_assert_eq!(g.edges.len(), f.edges.len());
                    }
                    Op::SetShape { id, shape } => {
                        proptest::prop_assert_eq!(g.node(id).unwrap().shape, *shape, "{}", out);
                        proptest::prop_assert_eq!(&g.node(id).unwrap().label, &f.node(id).unwrap().label, "{}", out);
                    }
                    Op::SetEdgeLabel { edge, label } => {
                        let want = label.clone().filter(|l| !l.is_empty());
                        let got = g.edges.iter().find(|e| e.from == f.edges[*edge].from && e.to == f.edges[*edge].to && e.label == want);
                        proptest::prop_assert!(got.is_some(), "{}", out);
                        proptest::prop_assert_eq!(g.edges.len(), f.edges.len());
                    }
                    _ => {}
                }
                src = out;
            }
        }
    }
}
