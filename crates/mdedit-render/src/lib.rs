//! Layout model for rendered markdown (ARCHITECTURE 4.2).
//!
//! Turns one top-level [`Block`] into a [`LayoutNode`] tree of styled spans,
//! list markers, table grids and image boxes. No GPU or toolkit types: the
//! View-mode widget (`mdedit-view`) draws it, and export may walk it. Every
//! span keeps its source byte range so rendered positions map back to the
//! buffer (cursor placement, Typora-style reveal, comments).
//!
//! Render workstream: implement [`layout_block`] for every CommonMark/GFM
//! block, the `BlockRenderer` registry for math/Mermaid (v1.x), and tests
//! against the CommonMark spec examples.

use std::ops::Range;

use mdedit_core::{Block, TextBuffer};

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

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text {
        text: String,
        style: SpanStyle,
        source: Range<usize>,
    },
    Link {
        url: String,
        title: String,
        children: Vec<Inline>,
        source: Range<usize>,
    },
    Image {
        src: String,
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
    /// Inline HTML shown as source, except `<br> <sup> <sub> <kbd>`.
    Html {
        html: String,
        source: Range<usize>,
    },
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
    pub children: Vec<LayoutNode>,
    pub source: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub inlines: Vec<Inline>,
    pub source: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayoutNode {
    Paragraph {
        inlines: Vec<Inline>,
    },
    Heading {
        level: u8,
        inlines: Vec<Inline>,
        id: Option<String>,
    },
    BlockQuote {
        children: Vec<LayoutNode>,
    },
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
    Table {
        align: Vec<Align>,
        header: Vec<TableCell>,
        rows: Vec<Vec<TableCell>>,
    },
    ThematicBreak,
    FootnoteDefinition {
        label: String,
        children: Vec<LayoutNode>,
    },
    /// Raw HTML block, shown as source (SPEC 4).
    Html {
        html: String,
    },
    /// Produced by a registered [`BlockRenderer`] (math, Mermaid).
    Custom {
        renderer: &'static str,
        payload: String,
    },
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

/// Lay out one block. Placeholder: shows every block as a code block of its
/// source.
pub fn layout_block(block: &Block, buf: &dyn TextBuffer) -> LayoutBlock {
    let code = buf.slice(block.bytes.clone()).into_owned();
    LayoutBlock {
        source: block.bytes.clone(),
        generation: block.generation,
        node: LayoutNode::CodeBlock { lang: None, code },
    }
}
