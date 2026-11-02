//! A small document tree built from pulldown-cmark events, shared by the
//! Typst and DOCX writers so both agree on structure: title extraction,
//! heading levels and slugs, callouts, footnotes and comment placement.

use std::collections::{BTreeMap, HashSet};

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};

/// A comment to place in the output as a footnote (SPEC 10, "include
/// comments"). The app resolves sidecar anchors to byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportComment {
    /// Byte offset in the markdown source; the note goes after the text
    /// run that contains it.
    pub offset: usize,
    pub author: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Link {
        url: String,
        content: Vec<Inline>,
    },
    Image {
        url: String,
        title: String,
        alt: String,
    },
    /// A reference to `Doc::footnotes[label]`.
    FootnoteRef(String),
    Comment {
        author: String,
        body: String,
    },
    SoftBreak,
    HardBreak,
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
    /// `Some(checked)` for a task list item.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// `level` is already shifted so the top heading level in the body is 1.
    Heading {
        level: u8,
        content: Vec<Inline>,
        slug: String,
    },
    Paragraph(Vec<Inline>),
    /// A paragraph that holds a single image: rendered as a figure.
    Figure {
        url: String,
        title: String,
        alt: String,
    },
    Quote(Vec<Block>),
    /// A blockquote that starts with `[!KIND]` (GitHub alerts) or with a
    /// line that is entirely bold (`> **Before you start**`).
    Callout {
        kind: String,
        title: Vec<Inline>,
        body: Vec<Block>,
    },
    Code {
        lang: String,
        text: String,
    },
    List {
        start: Option<u64>,
        tight: bool,
        items: Vec<ListItem>,
    },
    Table {
        aligns: Vec<Align>,
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Doc {
    /// The first block when it is the only top-level heading of its level
    /// and no heading is above it (a document title).
    pub title: Option<Vec<Inline>>,
    pub blocks: Vec<Block>,
    pub footnotes: BTreeMap<String, Vec<Block>>,
}

impl Doc {
    pub fn parse(markdown: &str, comments: &[ExportComment]) -> Self {
        let parser = Parser::new_ext(markdown, mdedit_core::parse::markdown_options());
        let mut comments: Vec<&ExportComment> = comments.iter().collect();
        comments.sort_by_key(|c| c.offset);
        let mut b = Builder { stack: vec![Frame::new(Container::Root)], footnotes: BTreeMap::new(), comments, next: 0 };
        for (event, range) in parser.into_offset_iter() {
            b.event(event, range.end);
        }
        let blocks = b.finish();
        let mut doc = Doc { title: None, blocks, footnotes: b.footnotes };
        doc.extract_title();
        doc.shift_headings();
        doc
    }

    /// The title is the first heading when every other heading is deeper.
    /// It is usually the first block; a level-1 heading also counts after
    /// a preamble (a note or a date line above the title), which then
    /// follows it.
    fn extract_title(&mut self) {
        let Some((at, level)) = self.blocks.iter().enumerate().find_map(|(i, b)| match b {
            Block::Heading { level, .. } => Some((i, *level)),
            _ => None,
        }) else {
            return;
        };
        if at > 0 && level != 1 {
            return;
        }
        let deeper = self.blocks[at + 1..].iter().all(|b| match b {
            Block::Heading { level: l, .. } => *l > level,
            _ => true,
        });
        if deeper && let Block::Heading { content, .. } = self.blocks.remove(at) {
            self.title = Some(content);
        }
    }

    /// Shift heading levels so the shallowest body heading is level 1 and
    /// give every heading a unique slug.
    fn shift_headings(&mut self) {
        let min = self
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { level, .. } => Some(*level),
                _ => None,
            })
            .min()
            .unwrap_or(1);
        let mut seen = HashSet::new();
        for block in &mut self.blocks {
            if let Block::Heading { level, content, slug } = block {
                *level -= min - 1;
                if slug.is_empty() {
                    *slug = slugify(&plain_text(content));
                }
                let base = slug.clone();
                let mut n = 1;
                while !seen.insert(slug.clone()) {
                    *slug = format!("{base}-{n}");
                    n += 1;
                }
            }
        }
    }

    /// True when every level-1 body heading already starts with its own
    /// number (`1.`, `2)`, `6.1`, `01`), as hand-numbered documents do.
    /// Automatic numbering would then print each number twice.
    pub fn has_manual_numbers(&self) -> bool {
        let mut top = self.blocks.iter().filter_map(|b| match b {
            Block::Heading { level: 1, content, .. } => Some(plain_text(content)),
            _ => None,
        });
        let mut any = false;
        top.all(|text| {
            any = true;
            starts_with_number(&text)
        }) && any
    }

    /// Slugs of all headings, for resolving `#anchor` links.
    pub fn slugs(&self) -> HashSet<&str> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { slug, .. } => Some(slug.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The title as plain text, or `fallback`.
    pub fn title_text(&self, fallback: &str) -> String {
        self.title.as_deref().map(plain_text).unwrap_or_else(|| fallback.to_string())
    }
}

/// GitHub-style heading slug: lowercase, alphanumerics kept, spaces and
/// hyphens become `-`, everything else dropped.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if c == ' ' || c == '-' || c == '_' {
            out.push('-');
        }
    }
    if out.is_empty() { "section".into() } else { out }
}

pub fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    fn walk(out: &mut String, inlines: &[Inline]) {
        for i in inlines {
            match i {
                Inline::Text(t) | Inline::Code(t) => out.push_str(t),
                Inline::Emph(c) | Inline::Strong(c) | Inline::Strike(c) => walk(out, c),
                Inline::Link { content, .. } => walk(out, content),
                Inline::Image { alt, .. } => out.push_str(alt),
                Inline::SoftBreak | Inline::HardBreak => out.push(' '),
                Inline::FootnoteRef(_) | Inline::Comment { .. } => {}
            }
        }
    }
    walk(&mut out, inlines);
    out
}

enum Container {
    Root,
    Paragraph,
    Heading(u8, Option<String>),
    Quote(Option<BlockQuoteKind>),
    Code(String),
    List(Option<u64>),
    Item,
    FootnoteDef(String),
    Table(Vec<Align>),
    TableHead,
    TableRow,
    TableCell,
    Emph,
    Strong,
    Strike,
    Link(String),
    Image(String, String),
    Skip,
}

struct Frame {
    kind: Container,
    blocks: Vec<Block>,
    inlines: Vec<Inline>,
    /// Items, rows or cells collected by list and table frames.
    items: Vec<ListItem>,
    rows: Vec<Vec<Vec<Inline>>>,
    cells: Vec<Vec<Inline>>,
    head: Vec<Vec<Inline>>,
    task: Option<bool>,
    /// List frames: whether an item had inline content outside a paragraph.
    tight: bool,
    text: String,
}

impl Frame {
    fn new(kind: Container) -> Self {
        Frame {
            kind,
            blocks: Vec::new(),
            inlines: Vec::new(),
            items: Vec::new(),
            rows: Vec::new(),
            cells: Vec::new(),
            head: Vec::new(),
            task: None,
            tight: false,
            text: String::new(),
        }
    }

    /// Move loose inline content (tight list items) into a paragraph.
    fn flush_inlines(&mut self) -> bool {
        if self.inlines.is_empty() {
            return false;
        }
        let inlines = std::mem::take(&mut self.inlines);
        self.blocks.push(Block::Paragraph(inlines));
        true
    }
}

struct Builder<'c> {
    stack: Vec<Frame>,
    footnotes: BTreeMap<String, Vec<Block>>,
    comments: Vec<&'c ExportComment>,
    next: usize,
}

impl Builder<'_> {
    fn top(&mut self) -> &mut Frame {
        self.stack.last_mut().expect("root frame")
    }

    fn in_skip(&self) -> bool {
        self.stack.iter().any(|f| matches!(f.kind, Container::Skip))
    }

    fn push_inline(&mut self, inline: Inline) {
        // Inside an image only the text counts, as its alt text.
        if let Some(image) = self.stack.iter_mut().rev().find(|f| matches!(f.kind, Container::Image(..))) {
            if let Inline::Text(t) | Inline::Code(t) = &inline {
                image.text.push_str(t)
            }
            return;
        }
        let top = self.top();
        if !matches!(top.kind, Container::Code(_)) {
            top.inlines.push(inline);
        }
    }

    /// Comments whose offset is before `end` go after the current run.
    fn place_comments(&mut self, end: usize) {
        while self.next < self.comments.len() && self.comments[self.next].offset < end {
            let c = self.comments[self.next];
            self.next += 1;
            self.push_inline(Inline::Comment { author: c.author.clone(), body: c.body.clone() });
        }
    }

    fn event(&mut self, event: Event, end: usize) {
        if self.in_skip() {
            if let Event::Start(_) = event {
                self.stack.push(Frame::new(Container::Skip));
            } else if let Event::End(_) = event {
                self.stack.pop();
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag, end),
            Event::Text(t) => {
                if let Container::Code(_) = self.top().kind {
                    self.top().text.push_str(&t);
                } else {
                    self.push_inline(Inline::Text(t.into_string()));
                    self.place_comments(end);
                }
            }
            Event::Code(t) => {
                self.push_inline(Inline::Code(t.into_string()));
                self.place_comments(end);
            }
            Event::SoftBreak => self.push_inline(Inline::SoftBreak),
            Event::HardBreak => self.push_inline(Inline::HardBreak),
            Event::Rule => self.top().blocks.push(Block::Rule),
            Event::FootnoteReference(label) => self.push_inline(Inline::FootnoteRef(label.into_string())),
            Event::TaskListMarker(checked) => {
                if let Some(item) = self.stack.iter_mut().rev().find(|f| matches!(f.kind, Container::Item)) {
                    item.task = Some(checked);
                }
            }
            Event::InlineHtml(html) | Event::Html(html) => {
                let tag = html.trim().to_ascii_lowercase();
                if matches!(tag.as_str(), "<br>" | "<br/>" | "<br />") {
                    self.push_inline(Inline::HardBreak);
                }
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => self.push_inline(Inline::Code(t.into_string())),
        }
    }

    fn start(&mut self, tag: Tag) {
        let kind = match tag {
            Tag::Paragraph => Container::Paragraph,
            Tag::Heading { level, id, .. } => Container::Heading(heading_level(level), id.map(|s| s.into_string())),
            Tag::BlockQuote(kind) => Container::Quote(kind),
            Tag::CodeBlock(CodeBlockKind::Fenced(info)) => {
                Container::Code(info.split_whitespace().next().unwrap_or("").to_string())
            }
            Tag::CodeBlock(CodeBlockKind::Indented) => Container::Code(String::new()),
            Tag::List(start) => Container::List(start),
            Tag::Item => Container::Item,
            Tag::FootnoteDefinition(label) => Container::FootnoteDef(label.into_string()),
            Tag::Table(aligns) => Container::Table(aligns.into_iter().map(align).collect()),
            Tag::TableHead => Container::TableHead,
            Tag::TableRow => Container::TableRow,
            Tag::TableCell => Container::TableCell,
            Tag::Emphasis => Container::Emph,
            Tag::Strong => Container::Strong,
            Tag::Strikethrough => Container::Strike,
            Tag::Link { dest_url, .. } => Container::Link(dest_url.into_string()),
            Tag::Image { dest_url, title, .. } => Container::Image(dest_url.into_string(), title.into_string()),
            Tag::HtmlBlock
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => Container::Skip,
        };
        if matches!(
            kind,
            Container::Paragraph
                | Container::Heading(..)
                | Container::Code(_)
                | Container::List(_)
                | Container::Quote(_)
                | Container::Table(_)
        ) {
            // Inline content seen so far in a list item means a tight list.
            if self.top().flush_inlines() {
                self.mark_tight();
            }
        }
        self.stack.push(Frame::new(kind));
    }

    fn mark_tight(&mut self) {
        let n = self.stack.len();
        if n >= 2
            && let Container::List(_) = self.stack[n - 2].kind
        {
            self.stack[n - 2].tight = true;
        }
    }

    fn end(&mut self, _tag: TagEnd, end: usize) {
        let mut frame = self.stack.pop().expect("balanced events");
        match std::mem::replace(&mut frame.kind, Container::Skip) {
            Container::Root | Container::Skip => {}
            Container::Paragraph => {
                let block = match frame.inlines.as_slice() {
                    [Inline::Image { url, title, alt }] => {
                        Block::Figure { url: url.clone(), title: title.clone(), alt: alt.clone() }
                    }
                    _ => Block::Paragraph(frame.inlines),
                };
                self.top().blocks.push(block);
            }
            Container::Heading(level, id) => {
                let slug = id.unwrap_or_default();
                self.top().blocks.push(Block::Heading { level, content: frame.inlines, slug });
            }
            Container::Quote(kind) => {
                let block = callout(kind, frame.blocks);
                self.top().blocks.push(block);
            }
            Container::Code(lang) => {
                self.top().blocks.push(Block::Code { lang, text: frame.text });
                self.place_comments(end);
            }
            Container::List(start) => {
                let tight = frame.tight || frame.items.iter().all(|i| i.blocks.len() <= 1);
                self.top().blocks.push(Block::List { start, tight, items: frame.items });
            }
            Container::Item => {
                if frame.flush_inlines() {
                    self.top().tight = true;
                }
                let item = ListItem { task: frame.task, blocks: frame.blocks };
                self.top().items.push(item);
            }
            Container::FootnoteDef(label) => {
                frame.flush_inlines();
                self.footnotes.insert(label, frame.blocks);
            }
            Container::Table(aligns) => {
                let block = Block::Table { aligns, head: frame.head, rows: frame.rows };
                self.top().blocks.push(block);
            }
            Container::TableHead => self.top().head = frame.cells,
            Container::TableRow => {
                let cells = frame.cells;
                self.top().rows.push(cells);
            }
            Container::TableCell => self.top().cells.push(frame.inlines),
            Container::Emph => self.push_inline(Inline::Emph(frame.inlines)),
            Container::Strong => self.push_inline(Inline::Strong(frame.inlines)),
            Container::Strike => self.push_inline(Inline::Strike(frame.inlines)),
            Container::Link(url) => self.push_inline(Inline::Link { url, content: frame.inlines }),
            Container::Image(url, title) => {
                let image = Inline::Image { url, title, alt: frame.text };
                match self.stack.iter_mut().rev().find(|f| matches!(f.kind, Container::Image(..))) {
                    Some(outer) => outer.text.push_str(&plain_text(&[image])),
                    None => self.push_inline(image),
                }
            }
        }
    }

    fn finish(&mut self) -> Vec<Block> {
        let mut root = self.stack.pop().expect("root frame");
        root.flush_inlines();
        root.blocks
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    level as u8
}

fn align(a: Alignment) -> Align {
    match a {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

/// Turn a blockquote into a callout when it starts with `[!KIND]` or with a
/// paragraph whose first line is entirely bold.
fn callout(kind: Option<BlockQuoteKind>, mut blocks: Vec<Block>) -> Block {
    if let Some(kind) = kind {
        let name = format!("{kind:?}").to_lowercase();
        return Block::Callout { title: vec![Inline::Text(name.to_uppercase())], kind: name, body: blocks };
    }
    let Some(Block::Paragraph(first)) = blocks.first_mut() else { return Block::Quote(blocks) };

    // GitHub alert written as text: `[!NOTE]` then a line break.
    let head: String = first
        .iter()
        .take_while(|i| !matches!(i, Inline::SoftBreak | Inline::HardBreak))
        .map(|i| match i {
            Inline::Text(t) => t.as_str(),
            _ => "\u{0}",
        })
        .collect();
    let head = head.trim();
    if let Some(name) = head.strip_prefix("[!").and_then(|h| h.strip_suffix(']'))
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphabetic())
    {
        let n = first.iter().take_while(|i| !matches!(i, Inline::SoftBreak | Inline::HardBreak)).count();
        first.drain(..(n + 1).min(first.len()));
        if first.is_empty() {
            blocks.remove(0);
        }
        return Block::Callout {
            kind: name.to_lowercase(),
            title: vec![Inline::Text(name.to_uppercase())],
            body: blocks,
        };
    }

    // `> **Title**` on its own line.
    if let Some(Inline::Strong(title)) = first.first()
        && matches!(first.get(1), None | Some(Inline::SoftBreak | Inline::HardBreak))
    {
        let title = title.clone();
        first.drain(..2.min(first.len()));
        if first.is_empty() {
            blocks.remove(0);
        }
        return Block::Callout { kind: "note".into(), title, body: blocks };
    }
    Block::Quote(blocks)
}

/// `1 `, `1. `, `2) `, `6.1 `, `01 `: digits, optional `.digits` groups,
/// an optional `.` or `)`, then whitespace.
fn starts_with_number(text: &str) -> bool {
    let rest = text.trim_start();
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let mut rest = &rest[digits..];
    while let Some(after) = rest.strip_prefix('.') {
        let n = after.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            break;
        }
        rest = &after[n..];
    }
    let rest = rest.strip_prefix(['.', ')']).unwrap_or(rest);
    rest.starts_with(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    #[test]
    fn manual_heading_numbers() {
        for yes in ["1. Goals", "2) Releases", "6.1 Layout", "01 Intro", "10. Export"] {
            assert!(super::starts_with_number(yes), "{yes}");
        }
        for no in ["1.5x faster", "Goals", "3D printing", "v1. Notes"] {
            assert!(!super::starts_with_number(no), "{no}");
        }
        let numbered = super::Doc::parse("# Spec\n\n## 1. Goals\n\n### 1.1 Sub\n\n## 2. Releases\n", &[]);
        assert!(numbered.has_manual_numbers());
        let mixed = super::Doc::parse("# Spec\n\n## 1. Goals\n\n## Releases\n", &[]);
        assert!(!mixed.has_manual_numbers());
        let none = super::Doc::parse("## Goals\n", &[]);
        assert!(!none.has_manual_numbers());
    }

    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn title_is_extracted_and_headings_shift() {
        let doc = Doc::parse("# Title\n\nLead.\n\n## One\n\n### Sub\n\n## One\n", &[]);
        assert_eq!(doc.title, Some(vec![text("Title")]));
        let heads: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { level, slug, .. } => Some((*level, slug.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(heads, [(1, "one"), (2, "sub"), (1, "one-1")]);
    }

    #[test]
    fn title_after_a_preamble() {
        let doc = Doc::parse("> A note.\n\n# Title\n\nDate.\n\n## One\n", &[]);
        assert_eq!(doc.title, Some(vec![text("Title")]));
        assert!(matches!(doc.blocks[0], Block::Quote(_)), "{:?}", doc.blocks[0]);
        // Only a level-1 heading is a title after other content.
        let doc = Doc::parse("Intro.\n\n## Part\n\n### Sub\n", &[]);
        assert_eq!(doc.title, None);
    }

    #[test]
    fn two_h1_means_no_title() {
        let doc = Doc::parse("# A\n\n# B\n", &[]);
        assert_eq!(doc.title, None);
        assert_eq!(doc.blocks.len(), 2);
    }

    #[test]
    fn tight_and_loose_lists_and_tasks() {
        let doc = Doc::parse("- [x] done\n- [ ] todo\n\n1. a\n\n2. b\n", &[]);
        let Block::List { tight, items, start } = &doc.blocks[0] else { panic!() };
        assert!(*tight);
        assert_eq!(*start, None);
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].blocks, vec![Block::Paragraph(vec![text("todo")])]);
        let Block::List { start, .. } = &doc.blocks[1] else { panic!() };
        assert_eq!(*start, Some(1));
    }

    #[test]
    fn nested_list_follows_item_text() {
        let doc = Doc::parse("- two\n  - nested\n- three\n", &[]);
        let Block::List { items, .. } = &doc.blocks[0] else { panic!() };
        assert_eq!(items[0].blocks[0], Block::Paragraph(vec![text("two")]));
        assert!(matches!(items[0].blocks[1], Block::List { .. }));
    }

    #[test]
    fn callouts() {
        let doc = Doc::parse("> **Heads up**\n> Body text.\n\n> [!WARNING]\n> Careful.\n\n> plain\n", &[]);
        assert!(matches!(&doc.blocks[0], Block::Callout { title, body, .. }
            if title == &vec![text("Heads up")] && body == &vec![Block::Paragraph(vec![text("Body text.")])]));
        assert!(matches!(&doc.blocks[1], Block::Callout { kind, .. } if kind == "warning"));
        assert!(matches!(&doc.blocks[2], Block::Quote(_)));
    }

    #[test]
    fn comments_follow_their_text_run() {
        let md = "Hello world. Second.";
        let c = ExportComment { offset: 3, author: "Ann".into(), body: "hi".into() };
        let doc = Doc::parse(md, &[c]);
        let Block::Paragraph(inl) = &doc.blocks[0] else { panic!() };
        assert_eq!(inl[0], text("Hello world. Second."));
        assert!(matches!(&inl[1], Inline::Comment { author, .. } if author == "Ann"));
    }

    #[test]
    fn footnotes_and_figures() {
        let doc = Doc::parse("Text[^a].\n\n![Alt *x*](img.png \"Cap\")\n\n[^a]: Note.\n", &[]);
        assert_eq!(doc.footnotes["a"], vec![Block::Paragraph(vec![text("Note.")])]);
        assert_eq!(doc.blocks[1], Block::Figure { url: "img.png".into(), title: "Cap".into(), alt: "Alt x".into() });
    }
}
