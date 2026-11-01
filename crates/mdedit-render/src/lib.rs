//! Layout model for rendered markdown (ARCHITECTURE 4.2).
//!
//! Turns one top-level [`Block`] into a [`LayoutNode`] tree of styled spans,
//! list markers, table grids and image boxes. No GPU or toolkit types: the
//! View-mode widget (`mdedit-view`) draws it, and export may walk it. Every
//! node and span keeps its source byte range so rendered positions map back
//! to the buffer (cursor placement, Typora-style reveal, comments).
//!
//! A block is parsed on its own, so document-global definitions (link
//! reference definitions, footnote definitions) come from a
//! [`LinkResolver`] in the [`LayoutContext`]; [`RefTable`] is one built from
//! the whole text. Blocks a [`BlockRenderer`] claims (math, Mermaid in v1.x)
//! become [`LayoutNode::Custom`].
//!
//! [`html::to_html`] writes the model as HTML in pulldown-cmark's dialect;
//! the tests use it to check every CommonMark spec example against
//! pulldown-cmark's own renderer.

mod convert;
pub mod html;
mod refs;

use std::ops::Range;

use mdedit_core::{Block, BlockKind, TextBuffer};

pub use refs::{IndexRefs, LinkDef, LinkResolver, RefTable, normalize_label};

/// Inline style flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpanStyle {
    pub strong: bool,
    pub emphasis: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub superscript: bool,
    pub subscript: bool,
    pub kbd: bool,
}

/// Inline content. `source` is the markup range in the document.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text {
        text: String,
        style: SpanStyle,
        source: Range<usize>,
        /// The source is exactly `text`, so positions map byte for byte.
        /// Otherwise (entities, escapes, line endings in code spans) only
        /// the ends of `source` are meaningful.
        exact: bool,
    },
    Link {
        url: String,
        title: String,
        children: Vec<Inline>,
        source: Range<usize>,
    },
    Image {
        src: String,
        /// Plain text of the image description.
        alt: String,
        title: String,
        source: Range<usize>,
    },
    FootnoteRef {
        label: String,
        source: Range<usize>,
    },
    SoftBreak {
        source: Range<usize>,
    },
    HardBreak {
        source: Range<usize>,
    },
    /// Inline math (v1.x).
    Math {
        tex: String,
        display: bool,
        source: Range<usize>,
    },
    /// Inline HTML shown as source, except `<br> <sup> <sub> <kbd>`, which
    /// become breaks and styles.
    Html {
        html: String,
        source: Range<usize>,
    },
}

impl Inline {
    pub fn source(&self) -> Range<usize> {
        match self {
            Inline::Text { source, .. }
            | Inline::Link { source, .. }
            | Inline::Image { source, .. }
            | Inline::FootnoteRef { source, .. }
            | Inline::SoftBreak { source }
            | Inline::HardBreak { source }
            | Inline::Math { source, .. }
            | Inline::Html { source, .. } => source.clone(),
        }
    }
}

/// Plain text of some inlines (outline titles, image alt, search).
pub fn plain_text(inlines: &[Inline]) -> String {
    fn walk(inlines: &[Inline], out: &mut String) {
        for inline in inlines {
            match inline {
                Inline::Text { text, .. } => out.push_str(text),
                Inline::Link { children, .. } => walk(children, out),
                Inline::Image { alt, .. } => out.push_str(alt),
                Inline::FootnoteRef { label, .. } => out.push_str(label),
                Inline::SoftBreak { .. } | Inline::HardBreak { .. } => out.push(' '),
                Inline::Math { tex, .. } => out.push_str(tex),
                Inline::Html { html, .. } => out.push_str(html),
            }
        }
    }
    let mut out = String::new();
    walk(inlines, &mut out);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// `Some(checked)` for task list items.
    pub task: Option<bool>,
    /// The `[ ]` / `[x]` marker, for toggling it.
    pub task_source: Option<Range<usize>>,
    pub children: Vec<LayoutNode>,
    pub source: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub inlines: Vec<Inline>,
    pub source: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
    pub source: Range<usize>,
}

/// A run of text in a code or HTML block and where it came from. Line
/// prefixes of containers (`> `, list indentation) are not part of the text,
/// so a block is a list of segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Range in the block's `code` / `html` string.
    pub text: Range<usize>,
    pub source: Range<usize>,
    /// The source is exactly the text (maps byte for byte).
    pub exact: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayoutNode {
    Paragraph {
        inlines: Vec<Inline>,
        source: Range<usize>,
    },
    Heading {
        level: u8,
        inlines: Vec<Inline>,
        id: Option<String>,
        classes: Vec<String>,
        attrs: Vec<(String, Option<String>)>,
        source: Range<usize>,
    },
    BlockQuote {
        children: Vec<LayoutNode>,
        source: Range<usize>,
    },
    List {
        /// First number of an ordered list; `None` for bullets.
        start: Option<u64>,
        /// Tight lists put no paragraph gap between or inside items.
        tight: bool,
        items: Vec<ListItem>,
        source: Range<usize>,
    },
    CodeBlock {
        /// First word of the info string.
        lang: Option<String>,
        /// The whole info string (empty for indented code).
        info: String,
        code: String,
        segments: Vec<Segment>,
        source: Range<usize>,
    },
    Table {
        align: Vec<Align>,
        header: TableRow,
        rows: Vec<TableRow>,
        source: Range<usize>,
    },
    ThematicBreak {
        source: Range<usize>,
    },
    FootnoteDefinition {
        label: String,
        children: Vec<LayoutNode>,
        source: Range<usize>,
    },
    /// Raw HTML block, shown as source (SPEC 4).
    Html {
        html: String,
        segments: Vec<Segment>,
        source: Range<usize>,
    },
    /// Produced by a registered [`BlockRenderer`] (math, Mermaid).
    Custom {
        renderer: &'static str,
        payload: String,
        source: Range<usize>,
    },
    /// Several nodes from one block. Only produced when a block's text does
    /// not parse to exactly one block on its own (it should not happen with
    /// the block index's ranges).
    Group {
        children: Vec<LayoutNode>,
        source: Range<usize>,
    },
}

impl LayoutNode {
    pub fn source(&self) -> Range<usize> {
        match self {
            LayoutNode::Paragraph { source, .. }
            | LayoutNode::Heading { source, .. }
            | LayoutNode::BlockQuote { source, .. }
            | LayoutNode::List { source, .. }
            | LayoutNode::CodeBlock { source, .. }
            | LayoutNode::Table { source, .. }
            | LayoutNode::ThematicBreak { source }
            | LayoutNode::FootnoteDefinition { source, .. }
            | LayoutNode::Html { source, .. }
            | LayoutNode::Custom { source, .. }
            | LayoutNode::Group { source, .. } => source.clone(),
        }
    }
}

/// A laid-out top-level block, cached by the view until `generation`
/// changes.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutBlock {
    pub source: Range<usize>,
    pub generation: u64,
    pub node: LayoutNode,
}

/// Extension point for block types beyond CommonMark (ARCHITECTURE 4.4).
pub trait BlockRenderer: Send + Sync {
    fn name(&self) -> &'static str;
    fn handles(&self, block: &Block) -> bool;
    fn layout(&self, block: &Block, source: &str) -> LayoutNode;
}

/// Document-wide inputs to [`layout_block_with`].
#[derive(Default, Clone, Copy)]
pub struct LayoutContext<'a> {
    /// Link reference and footnote definitions from the rest of the
    /// document. Without one, reference links to definitions outside the
    /// block stay plain text and footnote references stay `[^label]`.
    pub links: Option<&'a dyn LinkResolver>,
    pub renderers: &'a [&'a dyn BlockRenderer],
}

/// Lay out one block with no document context (see [`layout_block_with`]).
pub fn layout_block(block: &Block, buf: &dyn TextBuffer) -> LayoutBlock {
    layout_block_with(block, buf, &LayoutContext::default())
}

/// Lay out one block. Math and Mermaid blocks no renderer claims are shown
/// as code blocks (SPEC 4).
pub fn layout_block_with(block: &Block, buf: &dyn TextBuffer, ctx: &LayoutContext<'_>) -> LayoutBlock {
    let node = match ctx.renderers.iter().find(|r| r.handles(block)) {
        Some(renderer) => {
            let src = buf.slice(block.bytes.clone());
            match renderer.layout(block, &src) {
                LayoutNode::Custom { renderer, payload, .. } => {
                    LayoutNode::Custom { renderer, payload, source: block.bytes.clone() }
                }
                other => other,
            }
        }
        None => {
            // A block can start after indentation the index leaves out; parse
            // from the line start so the block means the same thing on its own
            // (four spaces make indented code).
            let line_start = buf.line_to_byte(buf.byte_to_line(block.bytes.start));
            let prefix = buf.slice(line_start..block.bytes.start);
            let base = if prefix.bytes().all(|b| b == b' ' || b == b'\t') { line_start } else { block.bytes.start };
            // The index leaves out the block's final line ending; code blocks
            // need it (their text ends with a newline).
            let tail = buf.slice(block.bytes.end..(block.bytes.end + 2).min(buf.len_bytes()));
            let eol = if tail.starts_with("\r\n") { 2 } else { usize::from(tail.starts_with(['\n', '\r'])) };
            let text = buf.slice(base..block.bytes.end + eol);
            let has_inlines = !matches!(
                block.kind,
                BlockKind::CodeBlock { .. }
                    | BlockKind::HtmlBlock
                    | BlockKind::ThematicBreak
                    | BlockKind::Math
                    | BlockKind::Mermaid
            );
            convert::convert_block(&text, base, block.bytes.clone(), ctx.links, has_inlines)
        }
    };
    LayoutBlock { source: block.bytes.clone(), generation: block.generation, node }
}

/// Lay out a whole markdown string as top-level nodes, resolving
/// definitions across it. For export, tests and small documents; the view
/// lays out block by block instead.
pub fn layout_document(text: &str) -> Vec<LayoutNode> {
    let refs = RefTable::scan(text);
    convert::convert_all(text, 0, Some(&refs))
}
