//! pulldown-cmark events to [`LayoutNode`]s.

use std::collections::HashSet;
use std::ops::Range;

use mdedit_core::parse::markdown_options;
use pulldown_cmark::{Alignment, BrokenLink, CodeBlockKind, CowStr, Event, LinkType, Parser, Tag, TagEnd};

use crate::refs::{LinkResolver, normalize_label};
use crate::{Align, Inline, LayoutNode, ListItem, Segment, SpanStyle, TableCell, TableRow};

/// Lay out one block's text. `base` is the document offset of `text`,
/// `source` the block's range; definitions come from `links`.
pub(crate) fn convert_block(
    text: &str,
    base: usize,
    source: Range<usize>,
    links: Option<&dyn LinkResolver>,
    has_inlines: bool,
) -> LayoutNode {
    // Footnote references only become references when their definition
    // exists, and it usually lives in another block: append stand-in
    // definitions after the block (and drop what they produce).
    let mut parse_text = text.to_string();
    if has_inlines && let Some(links) = links {
        let mut seen = HashSet::new();
        for label in footnote_refs(text) {
            if links.has_footnote(label) && seen.insert(normalize_label(label)) {
                parse_text.push_str("\n\n[^");
                parse_text.push_str(label);
                parse_text.push_str("]: .\n");
            }
        }
    }
    let mut nodes = convert_all(&parse_text, base, links);
    nodes.retain(|n| n.source().start < base + text.len());
    if nodes.len() == 1 { nodes.pop().unwrap() } else { LayoutNode::Group { children: nodes, source } }
}

/// Labels of everything that looks like `[^label]`.
fn footnote_refs(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices("[^").filter_map(|(i, _)| {
        let rest = &text[i + 2..];
        let end = rest.find(']')?;
        let label = &rest[..end];
        (!label.is_empty() && !label.contains(|c: char| c.is_whitespace() || c == '[')).then_some(label)
    })
}

/// Lay out all top-level blocks of `text`.
pub(crate) fn convert_all(text: &str, base: usize, links: Option<&dyn LinkResolver>) -> Vec<LayoutNode> {
    let callback = |link: BrokenLink<'_>| {
        let def = links?.link_ref(&link.reference)?;
        Some((CowStr::from(def.dest), CowStr::from(def.title)))
    };
    let parser = Parser::new_with_broken_link_callback(text, markdown_options(), Some(callback));
    let mut b = Builder { text, base, stack: Vec::new(), out: Vec::new(), style: Styles::default() };
    for (event, range) in parser.into_offset_iter() {
        b.event(event, base + range.start..base + range.end);
    }
    while !b.stack.is_empty() {
        b.close();
    }
    b.out
}

#[derive(Default)]
struct Styles {
    strong: u32,
    emphasis: u32,
    strikethrough: u32,
    superscript: u32,
    subscript: u32,
    kbd: u32,
}

impl Styles {
    fn current(&self) -> SpanStyle {
        SpanStyle {
            strong: self.strong > 0,
            emphasis: self.emphasis > 0,
            strikethrough: self.strikethrough > 0,
            code: false,
            superscript: self.superscript > 0,
            subscript: self.subscript > 0,
            kbd: self.kbd > 0,
        }
    }

    fn counter(&mut self, tag: HtmlStyle) -> &mut u32 {
        match tag {
            HtmlStyle::Sup => &mut self.superscript,
            HtmlStyle::Sub => &mut self.subscript,
            HtmlStyle::Kbd => &mut self.kbd,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtmlStyle {
    Sup,
    Sub,
    Kbd,
}

enum InlineHtml {
    Br,
    Open(HtmlStyle),
    Close(HtmlStyle),
    Other,
}

fn classify_html(html: &str) -> InlineHtml {
    let tag = html.trim().to_ascii_lowercase();
    let style = |name: &str| match name {
        "sup" => Some(HtmlStyle::Sup),
        "sub" => Some(HtmlStyle::Sub),
        "kbd" => Some(HtmlStyle::Kbd),
        _ => None,
    };
    match tag.as_str() {
        "<br>" | "<br/>" | "<br />" => InlineHtml::Br,
        _ => {
            if let Some(name) = tag.strip_prefix("</").and_then(|t| t.strip_suffix('>')) {
                style(name).map_or(InlineHtml::Other, InlineHtml::Close)
            } else if let Some(name) = tag.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
                style(name).map_or(InlineHtml::Other, InlineHtml::Open)
            } else {
                InlineHtml::Other
            }
        }
    }
}

enum Kind {
    Paragraph,
    Heading { level: u8, id: Option<String>, classes: Vec<String>, attrs: Vec<(String, Option<String>)> },
    BlockQuote,
    List { start: Option<u64>, loose: bool },
    Item,
    CodeBlock { info: String },
    HtmlBlock,
    Table { align: Vec<Align> },
    TableHead,
    TableRow,
    TableCell,
    FootnoteDefinition { label: String },
    Link { url: String, title: String },
    Image { src: String, title: String, depth: u32 },
    /// Tags of extensions mdedit does not enable; their content is kept.
    Other,
}

struct Frame {
    kind: Kind,
    source: Range<usize>,
    children: Vec<LayoutNode>,
    inlines: Vec<Inline>,
    items: Vec<ListItem>,
    header: Option<TableRow>,
    rows: Vec<TableRow>,
    cells: Vec<TableCell>,
    text: String,
    segments: Vec<Segment>,
    task: Option<(bool, Range<usize>)>,
}

impl Frame {
    fn new(kind: Kind, source: Range<usize>) -> Self {
        Self {
            kind,
            source,
            children: Vec::new(),
            inlines: Vec::new(),
            items: Vec::new(),
            header: None,
            rows: Vec::new(),
            cells: Vec::new(),
            text: String::new(),
            segments: Vec::new(),
            task: None,
        }
    }

    /// Inline content not inside a paragraph (tight list items) becomes one.
    fn flush_inlines(&mut self) {
        if self.inlines.is_empty() {
            return;
        }
        let inlines = std::mem::take(&mut self.inlines);
        let source = inlines[0].source().start..inlines[inlines.len() - 1].source().end;
        self.children.push(LayoutNode::Paragraph { inlines, source });
    }

    fn push_segment(&mut self, text: &str, source: Range<usize>) {
        let start = self.text.len();
        self.text.push_str(text);
        self.segments.push(Segment { text: start..self.text.len(), source });
    }
}

struct Builder<'t> {
    text: &'t str,
    base: usize,
    stack: Vec<Frame>,
    out: Vec<LayoutNode>,
    style: Styles,
}

impl Builder<'_> {
    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        // Everything inside an image is its description.
        if let Some(Frame { kind: Kind::Image { depth, .. }, text, .. }) = self.stack.last_mut() {
            match event {
                Event::Start(_) => *depth += 1,
                Event::End(_) if *depth == 0 => self.close(),
                Event::End(_) => *depth -= 1,
                Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) | Event::InlineMath(t) => text.push_str(&t),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if let Some(top @ Frame { kind: Kind::CodeBlock { .. }, .. }) = self.stack.last_mut() {
                    top.push_segment(&text, range);
                } else {
                    let style = self.style.current();
                    self.push_inline(Inline::Text { text: text.into_string(), style, source: range });
                }
            }
            Event::Code(text) => {
                let source = self.code_span_source(&text, range);
                let style = SpanStyle { code: true, ..self.style.current() };
                self.push_inline(Inline::Text { text: text.into_string(), style, source });
            }
            Event::InlineMath(tex) => {
                self.push_inline(Inline::Math { tex: tex.into_string(), display: false, source: range })
            }
            Event::DisplayMath(tex) => {
                self.push_inline(Inline::Math { tex: tex.into_string(), display: true, source: range })
            }
            Event::Html(html) => {
                if let Some(top @ Frame { kind: Kind::HtmlBlock, .. }) = self.stack.last_mut() {
                    top.push_segment(&html, range);
                } else {
                    self.inline_html(html, range);
                }
            }
            Event::InlineHtml(html) => self.inline_html(html, range),
            Event::FootnoteReference(label) => {
                self.push_inline(Inline::FootnoteRef { label: label.into_string(), source: range })
            }
            Event::SoftBreak => self.push_inline(Inline::SoftBreak { source: range }),
            Event::HardBreak => self.push_inline(Inline::HardBreak { source: range }),
            Event::Rule => self.push_node(LayoutNode::ThematicBreak { source: trim_newlines(&range, self) }),
            Event::TaskListMarker(checked) => {
                if let Some(item) = self.stack.iter_mut().rev().find(|f| matches!(f.kind, Kind::Item)) {
                    item.task = Some((checked, range));
                }
            }
        }
    }

    fn inline_html(&mut self, html: CowStr<'_>, range: Range<usize>) {
        match classify_html(&html) {
            InlineHtml::Br => self.push_inline(Inline::HardBreak { source: range }),
            InlineHtml::Open(tag) => *self.style.counter(tag) += 1,
            InlineHtml::Close(tag) if *self.style.counter(tag) > 0 => *self.style.counter(tag) -= 1,
            InlineHtml::Close(_) | InlineHtml::Other => {
                self.push_inline(Inline::Html { html: html.into_string(), source: range })
            }
        }
    }

    /// The content range of a code span when the text maps onto it byte
    /// for byte, else the whole span.
    fn code_span_source(&self, text: &str, range: Range<usize>) -> Range<usize> {
        let src = &self.text[range.start - self.base..range.end - self.base];
        let ticks = src.bytes().take_while(|&b| b == b'`').count();
        if src.len() < 2 * ticks {
            return range;
        }
        let inner = range.start + ticks..range.end - ticks;
        if inner.len() == text.len() {
            inner
        } else if inner.len() == text.len() + 2 {
            inner.start + 1..inner.end - 1
        } else {
            range
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        let kind = match tag {
            Tag::Paragraph => {
                if let [.., Frame { kind: Kind::List { loose, .. }, .. }, Frame { kind: Kind::Item, .. }] =
                    self.stack.as_mut_slice()
                {
                    *loose = true;
                }
                Kind::Paragraph
            }
            Tag::Heading { level, id, classes, attrs } => Kind::Heading {
                level: level as u8,
                id: id.map(CowStr::into_string),
                classes: classes.into_iter().map(CowStr::into_string).collect(),
                attrs: attrs.into_iter().map(|(k, v)| (k.into_string(), v.map(CowStr::into_string))).collect(),
            },
            Tag::BlockQuote(_) => Kind::BlockQuote,
            Tag::CodeBlock(CodeBlockKind::Indented) => Kind::CodeBlock { info: String::new() },
            Tag::CodeBlock(CodeBlockKind::Fenced(info)) => Kind::CodeBlock { info: info.into_string() },
            Tag::HtmlBlock => Kind::HtmlBlock,
            Tag::List(start) => Kind::List { start, loose: false },
            Tag::Item => Kind::Item,
            Tag::FootnoteDefinition(label) => Kind::FootnoteDefinition { label: label.into_string() },
            Tag::Table(align) => Kind::Table { align: align.into_iter().map(align_of).collect() },
            Tag::TableHead => Kind::TableHead,
            Tag::TableRow => Kind::TableRow,
            Tag::TableCell => Kind::TableCell,
            Tag::Emphasis => return self.style.emphasis += 1,
            Tag::Strong => return self.style.strong += 1,
            Tag::Strikethrough => return self.style.strikethrough += 1,
            Tag::Superscript => return self.style.superscript += 1,
            Tag::Subscript => return self.style.subscript += 1,
            Tag::Link { link_type, dest_url, title, .. } => {
                let url = match link_type {
                    LinkType::Email => format!("mailto:{dest_url}"),
                    _ => dest_url.into_string(),
                };
                Kind::Link { url, title: title.into_string() }
            }
            Tag::Image { dest_url, title, .. } => {
                Kind::Image { src: dest_url.into_string(), title: title.into_string(), depth: 0 }
            }
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => Kind::Other,
        };
        self.stack.push(Frame::new(kind, range));
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Emphasis => self.style.emphasis = self.style.emphasis.saturating_sub(1),
            TagEnd::Strong => self.style.strong = self.style.strong.saturating_sub(1),
            TagEnd::Strikethrough => self.style.strikethrough = self.style.strikethrough.saturating_sub(1),
            TagEnd::Superscript => self.style.superscript = self.style.superscript.saturating_sub(1),
            TagEnd::Subscript => self.style.subscript = self.style.subscript.saturating_sub(1),
            _ => self.close(),
        }
    }

    /// Pop the top frame and attach what it built to its parent.
    fn close(&mut self) {
        let Some(mut f) = self.stack.pop() else { return };
        let source = trim_newlines(&f.source, self);
        let node = match std::mem::replace(&mut f.kind, Kind::Other) {
            Kind::Paragraph => {
                // Unclosed <sup> & co. end with their paragraph.
                self.reset_html_styles();
                LayoutNode::Paragraph { inlines: f.inlines, source }
            }
            Kind::Heading { level, id, classes, attrs } => {
                self.reset_html_styles();
                LayoutNode::Heading { level, inlines: f.inlines, id, classes, attrs, source }
            }
            Kind::BlockQuote => {
                f.flush_inlines();
                LayoutNode::BlockQuote { children: f.children, source }
            }
            Kind::List { start, loose } => LayoutNode::List { start, tight: !loose, items: f.items, source },
            Kind::Item => {
                self.reset_html_styles();
                f.flush_inlines();
                let (task, task_source) = f.task.map_or((None, None), |(c, r)| (Some(c), Some(r)));
                let item = ListItem { task, task_source, children: f.children, source };
                if let Some(Frame { kind: Kind::List { .. }, items, .. }) = self.stack.last_mut() {
                    items.push(item);
                }
                return;
            }
            Kind::CodeBlock { info } => {
                let lang = info.split_whitespace().next().filter(|l| !l.is_empty()).map(str::to_string);
                LayoutNode::CodeBlock { lang, info, code: f.text, segments: f.segments, source }
            }
            Kind::HtmlBlock => LayoutNode::Html { html: f.text, segments: f.segments, source },
            Kind::Table { align } => {
                let header = f.header.unwrap_or(TableRow { cells: Vec::new(), source: source.start..source.start });
                LayoutNode::Table { align, header, rows: f.rows, source }
            }
            kind @ (Kind::TableHead | Kind::TableRow) => {
                let is_head = matches!(kind, Kind::TableHead);
                let row = TableRow { cells: f.cells, source };
                if let Some(table @ Frame { kind: Kind::Table { .. }, .. }) = self.stack.last_mut() {
                    if is_head {
                        table.header = Some(row);
                    } else {
                        table.rows.push(row);
                    }
                }
                return;
            }
            Kind::TableCell => {
                self.reset_html_styles();
                if let Some(row) = self.stack.last_mut() {
                    row.cells.push(TableCell { inlines: f.inlines, source });
                }
                return;
            }
            Kind::FootnoteDefinition { label } => {
                f.flush_inlines();
                LayoutNode::FootnoteDefinition { label, children: f.children, source }
            }
            Kind::Link { url, title } => {
                return self.push_inline(Inline::Link { url, title, children: f.inlines, source: f.source });
            }
            Kind::Image { src, title, .. } => {
                return self.push_inline(Inline::Image { src, alt: f.text, title, source: f.source });
            }
            Kind::Other => {
                f.flush_inlines();
                LayoutNode::Group { children: f.children, source }
            }
        };
        self.push_node(node);
    }

    fn reset_html_styles(&mut self) {
        if !self.stack.iter().any(|f| matches!(f.kind, Kind::Paragraph | Kind::Heading { .. } | Kind::TableCell)) {
            self.style.superscript = 0;
            self.style.subscript = 0;
            self.style.kbd = 0;
        }
    }

    fn push_node(&mut self, node: LayoutNode) {
        match self.stack.last_mut() {
            Some(parent) => {
                parent.flush_inlines();
                parent.children.push(node);
            }
            None => self.out.push(node),
        }
    }

    fn push_inline(&mut self, inline: Inline) {
        let Some(parent) = self.stack.last_mut() else {
            // Inline content outside any block cannot come from pulldown.
            let source = inline.source();
            self.out.push(LayoutNode::Paragraph { inlines: vec![inline], source });
            return;
        };
        if let (
            Some(Inline::Text { text: prev, style: prev_style, source: prev_src }),
            Inline::Text { text, style, source },
        ) = (parent.inlines.last_mut(), &inline)
            && prev_style == style
            && prev_src.end == source.start
            && prev.len() == prev_src.len()
            && text.len() == source.len()
        {
            prev.push_str(text);
            prev_src.end = source.end;
            return;
        }
        parent.inlines.push(inline);
    }
}

fn align_of(a: Alignment) -> Align {
    match a {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

/// Block ranges from pulldown include the trailing line ending; the block
/// index excludes it, so do the same.
fn trim_newlines(range: &Range<usize>, b: &Builder<'_>) -> Range<usize> {
    let src = &b.text[range.start - b.base..range.end - b.base];
    range.start..range.start + src.trim_end_matches(['\n', '\r']).len()
}
