//! Positions a [`LayoutNode`] (or a block's styled source, when revealed)
//! into drawable items for one top-level block (DESIGN_SYSTEM 3.2, 4.1,
//! 7.1). Items are relative to the block's top-left corner at the left
//! edge of the document column. Source ranges are absolute offsets as of
//! when the block was laid out ([`BlockLayout::laid_start`]); the widget
//! shifts them when the block has moved since.

use std::ops::Range;
use std::path::Path;

use iced::advanced::graphics::text::Paragraph;
use iced::advanced::image::Handle;
use iced::advanced::svg::Handle as SvgHandle;
use iced::advanced::text::{self, LineHeight, Paragraph as _, Shaping, Span, Text, Wrapping};
use iced::border::Radius;
use iced::{Border, Color, Font, Pixels, Point, Rectangle, Size, alignment};
use mdedit_core::highlight::{StyledRange, SyntaxToken};
use mdedit_render::{Align, Callout, Inline, LayoutNode, ListItem, SpanStyle};
use mdedit_theme::space::*;
use mdedit_theme::typography::{self, TextStyle};

use crate::diagrams::{self, DiagramState};
use crate::geometry;
use crate::images::{self, ImageState};
use crate::style::{Look, bold, italic};

/// A text run's position in its paragraph and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MapSpan {
    pub text: Range<usize>,
    pub source: Range<usize>,
    /// Positions map byte for byte (same length).
    pub exact: bool,
}

pub(crate) struct TextItem {
    pub pos: Point,
    pub para: Paragraph,
    pub text: String,
    /// Line starts of the paragraph's text (`text` plus joiners).
    starts: Vec<usize>,
    /// See [`Shaped::joins`].
    joins: Vec<usize>,
    pub map: Vec<MapSpan>,
    pub links: Vec<(Range<usize>, String)>,
    pub color: Color,
    pub line_height: f32,
    /// Backgrounds and decorations (inline code, kbd, strikethrough),
    /// relative to `pos`, drawn under (`under`) or over the text.
    pub under: Vec<(Rectangle, Color, Border)>,
    pub over: Vec<(Rectangle, Color)>,
}

impl TextItem {
    /// Source offset for a text offset.
    pub fn to_source(&self, offset: usize) -> Option<usize> {
        let i = self.map.partition_point(|m| m.text.end < offset);
        let m = self.map.get(i).or(self.map.last())?;
        Some(if offset <= m.text.start {
            m.source.start
        } else if offset >= m.text.end {
            m.source.end
        } else if m.exact {
            m.source.start + (offset - m.text.start)
        } else if offset - m.text.start < m.text.end - offset {
            m.source.start
        } else {
            m.source.end
        })
    }

    /// Text offset for a source offset, if this item shows it.
    pub fn text_at_source(&self, byte: usize) -> Option<usize> {
        let m = self.map.iter().find(|m| m.source.start <= byte && byte <= m.source.end)?;
        Some(if m.exact {
            m.text.start + (byte - m.source.start)
        } else if byte == m.source.start {
            m.text.start
        } else {
            m.text.end
        })
    }

    pub fn source_range(&self) -> Option<Range<usize>> {
        Some(self.map.first()?.source.start..self.map.last()?.source.end)
    }

    /// Text ranges showing any of `source`.
    pub fn text_ranges(&self, source: Range<usize>) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for m in &self.map {
            let outside = m.source.end <= source.start || m.source.start >= source.end;
            if outside && !(m.source.is_empty() && source.contains(&m.source.start)) {
                continue;
            }
            let r = if m.exact {
                let s = source.start.max(m.source.start) - m.source.start;
                let e = source.end.min(m.source.end) - m.source.start;
                m.text.start + s..m.text.start + e
            } else {
                m.text.clone()
            };
            match out.last_mut() {
                Some(last) if last.end == r.start => last.end = r.end,
                _ => out.push(r),
            }
        }
        out
    }

    /// Paragraph offset for a text offset.
    fn text_to_para(&self, offset: usize) -> usize {
        offset + JOINER.len() * self.joins.partition_point(|&j| j < offset)
    }

    /// Text offset for a paragraph offset.
    fn para_to_text(&self, offset: usize) -> usize {
        let mut out = offset;
        for (k, j) in self.joins.iter().enumerate() {
            let at = j + k * JOINER.len();
            if offset <= at {
                break;
            }
            out = (offset - (k + 1) * JOINER.len()).max(*j);
        }
        out
    }

    pub fn caret(&self, offset: usize) -> Rectangle {
        geometry::caret(&self.para, &self.starts, self.text_to_para(offset), self.line_height)
    }

    pub fn hit(&self, p: Point) -> usize {
        let len = self.text.len() + self.joins.len() * JOINER.len();
        self.para_to_text(geometry::hit(&self.para, &self.starts, len, p))
    }

    pub fn range_rects(&self, start: usize, end: usize) -> Vec<Rectangle> {
        geometry::range_rects(&self.para, &self.starts, self.text_to_para(start), self.text_to_para(end))
    }

    /// Text offsets of the start and end of the visual row at `offset`.
    pub fn row_bounds(&self, offset: usize) -> (usize, usize) {
        let (s, e) = geometry::row_bounds(&self.para, &self.starts, self.text_to_para(offset));
        (self.para_to_text(s), self.para_to_text(e))
    }

    pub fn bounds(&self) -> Rectangle {
        let b = self.para.min_bounds();
        Rectangle::new(self.pos, Size::new(b.width, b.height.max(self.line_height)))
    }

    pub fn link_at(&self, offset: usize) -> Option<&str> {
        self.links.iter().find(|(r, _)| r.start <= offset && offset < r.end).map(|(_, u)| u.as_str())
    }
}

pub(crate) enum Item {
    Text(TextItem),
    Quad {
        rect: Rectangle,
        color: Color,
        border: Border,
    },
    Image {
        rect: Rectangle,
        handle: Handle,
    },
    /// A rendered Mermaid diagram.
    Svg {
        rect: Rectangle,
        handle: SvgHandle,
    },
    Check {
        rect: Rectangle,
        checked: bool,
        source: Range<usize>,
    },
    /// Copy button of a code block (shown on hover).
    Copy {
        rect: Rectangle,
        code: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cell {
    pub rect: Rectangle,
    /// Cell content without padding spaces and pipes.
    pub content: Range<usize>,
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableOp {
    RowAbove,
    RowBelow,
    ColLeft,
    ColRight,
    DeleteRow,
    DeleteCol,
    Align(Align),
}

pub(crate) struct TableInfo {
    pub cells: Vec<Cell>,
    pub cols: usize,
    /// Rows including the header.
    pub rows: usize,
    pub align: Vec<Align>,
    pub source: Range<usize>,
    /// Floating toolbar above the table, shown while the cursor is in it.
    pub toolbar: Vec<(Rectangle, TableOp, TextItem)>,
}

pub(crate) struct BlockLayout {
    pub height: f32,
    pub items: Vec<Item>,
    pub laid_start: usize,
    pub table: Option<TableInfo>,
    /// Media generation ([`crate::widget::media_generation`]) this layout
    /// saw, if it holds images or diagrams.
    pub image_gen: Option<u64>,
    pub revealed: bool,
}

impl BlockLayout {
    pub fn texts(&self) -> impl Iterator<Item = &TextItem> {
        self.items.iter().filter_map(|i| match i {
            Item::Text(t) => Some(t),
            _ => None,
        })
    }
}

/// Inputs shared by every block layout of one widget build.
pub(crate) struct Ctx<'a> {
    pub look: &'a Look,
    pub zoom: f32,
    /// Column width and the wider limit for tables, code and images.
    pub width: f32,
    pub wide: f32,
    pub doc_path: Option<&'a Path>,
    pub buf: &'a dyn mdedit_core::TextBuffer,
    /// Cursor, when it is inside this block and the view is editable (for
    /// the table's active cell and toolbar).
    pub cursor: Option<usize>,
    pub editable: bool,
    pub image_gen: u64,
}

struct Builder<'a, 'b> {
    ctx: &'b Ctx<'a>,
    items: Vec<Item>,
    table: Option<TableInfo>,
    images: bool,
}

fn scaled(s: TextStyle, zoom: f32) -> TextStyle {
    TextStyle { size: s.size * zoom, line_height: s.line_height * zoom, ..s }
}

/// Space above a top-level block.
pub(crate) fn top_margin(node_kind: &mdedit_core::BlockKind, first: bool, after_heading: Option<u8>) -> f32 {
    use mdedit_core::BlockKind;
    if first {
        return SPACE_6;
    }
    let own = match node_kind {
        BlockKind::Heading { level: 1 | 2 } => SPACE_7,
        BlockKind::Heading { .. } => SPACE_5,
        BlockKind::ThematicBreak => SPACE_6 - SPACE_4,
        _ => SPACE_4,
    };
    match after_heading {
        Some(1 | 2) => own.min(SPACE_3),
        Some(_) => own.min(SPACE_2),
        None => own,
    }
}

/// Lay out a rendered block.
pub(crate) fn layout_node(ctx: &Ctx<'_>, node: &LayoutNode, top: f32, laid_start: usize) -> BlockLayout {
    let mut b = Builder { ctx, items: Vec::new(), table: None, images: false };
    let h = b.node(node, 0.0, top * ctx.zoom, ctx.width, ctx.look.colors.text);
    BlockLayout {
        height: top * ctx.zoom + h,
        items: b.items,
        laid_start,
        table: b.table,
        image_gen: b.images.then_some(ctx.image_gen),
        revealed: false,
    }
}

/// Lay out a block as its styled source (Typora reveal). `lines` are the
/// source lines (without endings) starting at `start`, with their syntax
/// ranges; `eols` the byte length of each line ending.
/// Quote markers and indentation at the start of a line.
fn container_prefix(line: &str) -> usize {
    let mut i = 0;
    let b = line.as_bytes();
    loop {
        let indent = b[i..].iter().take_while(|&&c| c == b' ' || c == b'\t').count();
        if b.get(i + indent) == Some(&b'>') {
            i += indent + 1;
            if b.get(i) == Some(&b' ') {
                i += 1;
            }
        } else {
            return i + indent;
        }
    }
}

/// If the line break between `prev` and `next` is a soft break in running
/// text (not a hard break, nor the start of another block or list item),
/// the length of `next`'s quote markers and indentation.
fn soft_break(prev: &str, next: &str) -> Option<usize> {
    let pp = container_prefix(prev);
    let np = container_prefix(next);
    let quotes = |s: &str, n: usize| s[..n].bytes().filter(|&c| c == b'>').count();
    let p = &prev[pp..];
    let t = &next[np..];
    let hard = p.ends_with("  ") || p.ends_with('\\');
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    let starts_block = t.is_empty()
        || t.starts_with(['>', '#', '|', '`', '~', '<'])
        || (t.starts_with(['-', '*', '+']) && matches!(t.as_bytes().get(1), Some(b' ') | None))
        || (digits > 0 && matches!(t.as_bytes().get(digits), Some(b'.' | b')')))
        || t.starts_with("[^");
    let joins = !hard
        && !p.trim().is_empty()
        && !starts_block
        && !p.starts_with(['#', '|'])
        && quotes(prev, pp) == quotes(next, np);
    joins.then_some(np)
}

pub(crate) fn layout_source(
    ctx: &Ctx<'_>,
    lines: &[(String, usize, Vec<StyledRange>)],
    code: bool,
    top: f32,
) -> BlockLayout {
    let z = ctx.zoom;
    let c = ctx.look.colors;
    let base = scaled(if code { typography::CODE } else { typography::BODY }, z);
    let base_font = if code { ctx.look.mono } else { ctx.look.prose };
    let mut spans: Vec<Span<'static, (), Font>> = Vec::new();
    let mut text = String::new();
    let mut map = Vec::new();
    let mut code_ranges = Vec::new();
    for (n, (line, source_start, ranges)) in lines.iter().enumerate() {
        // Bytes of this line hidden behind a joined line break.
        let mut skip = 0;
        if n > 0 {
            let prev_end = lines[n - 1].1 + lines[n - 1].0.len();
            let start = text.len();
            // A soft line break inside running text shows as a space (with
            // the next line's quote markers and indentation folded into it),
            // so the revealed block wraps like the rendered one.
            let brk = match soft_break(&lines[n - 1].0, line) {
                Some(prefix) if !code => {
                    skip = prefix;
                    " "
                }
                _ => "\n",
            };
            text.push_str(brk);
            map.push(MapSpan {
                text: start..start + 1,
                source: prev_end..*source_start + skip,
                exact: *source_start + skip == prev_end + 1,
            });
            spans.push(Span::new(brk).size(base.size).line_height(LineHeight::Absolute(Pixels(base.line_height))));
        }
        let line_start = text.len();
        text.push_str(&line[skip..]);
        map.push(MapSpan {
            text: line_start..text.len(),
            source: *source_start + skip..source_start + line.len(),
            exact: true,
        });
        let line_start = line_start.wrapping_sub(skip);
        // Split the line where any token starts or ends; the innermost
        // (last) token wins for colour, enclosing ones still apply size and
        // weight.
        let mut cuts: Vec<usize> = vec![skip, line.len()];
        for r in ranges {
            cuts.push(r.range.start.clamp(skip, line.len()));
            cuts.push(r.range.end.clamp(skip, line.len()));
        }
        cuts.sort_unstable();
        cuts.dedup();
        let heading = ranges.iter().find_map(|r| match r.token {
            SyntaxToken::Heading(l) => Some(l),
            _ => None,
        });
        for w in cuts.windows(2) {
            let (s, e) = (w[0], w[1]);
            if s >= e || !line.is_char_boundary(s) || !line.is_char_boundary(e) {
                continue;
            }
            let mut style = heading.map_or(base, |l| scaled(typography::heading(l), z));
            let mut font = if heading.is_some() {
                Font { weight: crate::style::weight(style.weight), ..base_font }
            } else {
                base_font
            };
            let mut color = if heading.is_some() { c.heading } else { c.text };
            let mut strike = false;
            for r in ranges.iter().filter(|r| r.range.start <= s && e <= r.range.end) {
                match r.token {
                    SyntaxToken::Heading(_) => {}
                    SyntaxToken::Marker => color = c.marker,
                    SyntaxToken::Emphasis => {
                        font = italic(font);
                        color = c.emphasis;
                    }
                    SyntaxToken::Strong => font = bold(font),
                    SyntaxToken::Strikethrough => strike = true,
                    SyntaxToken::Link => color = c.link,
                    SyntaxToken::Url => color = c.url,
                    SyntaxToken::Code => {
                        font = Font { weight: font.weight, ..ctx.look.mono };
                        color = c.code;
                        if !code {
                            style = TextStyle { size: style.size * 0.875, ..style };
                            code_ranges.push(line_start.wrapping_add(s)..line_start.wrapping_add(e));
                        }
                    }
                    SyntaxToken::Html => color = c.html,
                    SyntaxToken::Math => color = c.math,
                }
            }
            let mut span = Span::new(line[s..e].to_string())
                .font(font)
                .size(style.size)
                .line_height(LineHeight::Absolute(Pixels(style.line_height.max(base.line_height))))
                .color(color);
            span.strikethrough = strike;
            spans.push(span);
        }
    }
    let width = ctx.width;
    let para = shape(&spans, width, base, base_font, true);
    let mut item = text_item(Point::new(0.0, top * z), para, text, map, Vec::new(), c.text, base.line_height);
    for r in code_ranges {
        for rect in item.range_rects(r.start, r.end) {
            item.under.push((pad(rect, 2.0, 0.0), c.code_bg, Border { radius: RADIUS_SM.into(), ..Border::default() }));
        }
    }
    let h = item.bounds().height;
    let mut items = Vec::new();
    if code {
        items.push(Item::Quad {
            rect: Rectangle::new(
                Point::new(-SPACE_2, top * z - SPACE_1),
                Size::new(width + 2.0 * SPACE_2, h + 2.0 * SPACE_1),
            ),
            color: c.bg_subtle,
            border: Border { radius: RADIUS_MD.into(), ..Border::default() },
        });
    }
    items.push(Item::Text(item));
    BlockLayout {
        height: top * z + h,
        items,
        laid_start: lines.first().map_or(0, |l| l.1),
        table: None,
        image_gen: None,
        revealed: true,
    }
}

/// Keep a revealed image paragraph's pictures visible under its source, so
/// editing the markdown doesn't make the image disappear. Captions and
/// loading or error labels are left out: their text would map to the same
/// source as the revealed markdown.
pub(crate) fn append_images(ctx: &Ctx<'_>, layout: &mut BlockLayout, node: &LayoutNode) {
    let LayoutNode::Paragraph { inlines, .. } = node else { return };
    let mut b = Builder { ctx, items: Vec::new(), table: None, images: false };
    let y = layout.height + SPACE_2;
    let Some(h) = b.image_paragraph(inlines, 0.0, y, ctx.width) else { return };
    layout.items.extend(b.items.into_iter().filter(|i| !matches!(i, Item::Text(_))));
    layout.height = y + h;
    layout.image_gen = Some(ctx.image_gen);
}

/// Keep a revealed Mermaid block's diagram visible under its source while
/// it is edited (the last good one while the source doesn't parse), with
/// the error under it.
pub(crate) fn append_diagram(ctx: &Ctx<'_>, layout: &mut BlockLayout, node: &LayoutNode) {
    let LayoutNode::CodeBlock { lang: Some(lang), code, source, .. } = node else { return };
    if !mdedit_mermaid::is_mermaid(lang) {
        return;
    }
    let mut b = Builder { ctx, items: Vec::new(), table: None, images: true };
    let mut y = layout.height + SPACE_4;
    let state = b.diagram_state(code, source.start);
    let (shown, error) = match state {
        DiagramState::Ready(d) => (Some(d), None),
        DiagramState::Loading { stale } => (stale, None),
        DiagramState::Failed { error, stale } => (stale, Some(error)),
    };
    if let Some(error) = error {
        y += b.diagram_error(&error, source.end, 0.0, y, ctx.width);
    }
    if let Some(d) = shown {
        y += SPACE_2 + b.place_diagram(&d, 0.0, y + SPACE_2, ctx.width);
    }
    // The error label is not part of the source: keep it out of hit tests.
    layout.items.extend(b.items.into_iter().map(|i| match i {
        Item::Text(mut t) => {
            t.map.clear();
            Item::Text(t)
        }
        other => other,
    }));
    layout.height = y;
    layout.image_gen = Some(ctx.image_gen);
}

fn pad(r: Rectangle, x: f32, y: f32) -> Rectangle {
    Rectangle::new(Point::new(r.x - x, r.y - y), Size::new(r.width + 2.0 * x, r.height + 2.0 * y))
}

/// A shaped paragraph. `joins` are the text offsets where a word joiner
/// (U+2060, 3 bytes) was added to the paragraph's text: cosmic-text shapes
/// an all-ASCII word with the font of its first character, so a font change
/// inside a word (`**bold**.`) needs a non-ASCII character there to take
/// effect.
pub(crate) struct Shaped {
    pub para: Paragraph,
    pub joins: Vec<usize>,
}

const JOINER: &str = "\u{2060}";

pub(crate) fn shape(spans: &[Span<'static, (), Font>], width: f32, base: TextStyle, font: Font, wrap: bool) -> Shaped {
    let mut joined: Vec<Span<'static, (), Font>> = Vec::with_capacity(spans.len());
    let mut joins = Vec::new();
    let mut offset = 0;
    let mut prev: Option<(Font, char)> = None;
    for span in spans {
        let f = span.font.unwrap_or(font);
        if let (Some((pf, last)), Some(first)) = (prev, span.text.chars().next())
            && pf != f
            && !last.is_whitespace()
            && !first.is_whitespace()
            && let Some(p) = joined.last_mut()
        {
            p.text = format!("{}{JOINER}", p.text).into();
            joins.push(offset);
        }
        joined.push(span.clone());
        offset += span.text.len();
        if let Some(last) = span.text.chars().next_back() {
            prev = Some((f, last));
        }
    }
    let para = Paragraph::with_spans(Text {
        content: &joined[..],
        bounds: Size::new(if wrap { width.max(1.0) } else { f32::INFINITY }, f32::INFINITY),
        size: Pixels(base.size),
        line_height: LineHeight::Absolute(Pixels(base.line_height)),
        font,
        align_x: text::Alignment::Left,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: if wrap { Wrapping::WordOrGlyph } else { Wrapping::None },
    });
    Shaped { para, joins }
}

fn text_item(
    pos: Point,
    shaped: Shaped,
    text: String,
    map: Vec<MapSpan>,
    links: Vec<(Range<usize>, String)>,
    color: Color,
    line_height: f32,
) -> TextItem {
    let Shaped { para, joins } = shaped;
    let mut shown = text.clone();
    for (k, j) in joins.iter().enumerate() {
        shown.insert_str(j + k * JOINER.len(), JOINER);
    }
    let starts = geometry::line_starts(&shown);
    TextItem { pos, para, text, starts, joins, map, links, color, line_height, under: Vec::new(), over: Vec::new() }
}

/// Inline content flattened to spans plus everything needed to map and
/// decorate it.
struct Runs {
    spans: Vec<Span<'static, (), Font>>,
    text: String,
    map: Vec<MapSpan>,
    links: Vec<(Range<usize>, String)>,
    code: Vec<Range<usize>>,
    kbd: Vec<Range<usize>>,
    strike: Vec<Range<usize>>,
}

struct RunStyle {
    base: TextStyle,
    font: Font,
    color: Color,
    uppercase: bool,
}

impl Builder<'_, '_> {
    fn look(&self) -> &Look {
        self.ctx.look
    }

    fn runs(&self, inlines: &[Inline], rs: &RunStyle) -> Runs {
        let mut r = Runs {
            spans: Vec::new(),
            text: String::new(),
            map: Vec::new(),
            links: Vec::new(),
            code: Vec::new(),
            kbd: Vec::new(),
            strike: Vec::new(),
        };
        self.push_inlines(&mut r, inlines, rs, None);
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn push_text(
        &self,
        r: &mut Runs,
        text: &str,
        style: SpanStyle,
        source: Range<usize>,
        exact: bool,
        rs: &RunStyle,
        link: Option<&str>,
    ) {
        let c = self.look().colors;
        let text = if rs.uppercase { text.to_uppercase() } else { text.to_string() };
        let mut size = rs.base.size;
        let mut font = rs.font;
        if style.code || style.kbd {
            font = Font { weight: font.weight, ..self.look().mono };
            size *= 0.875;
        }
        if style.strong {
            font = bold(font);
        }
        if style.emphasis {
            font = italic(font);
        }
        if style.superscript || style.subscript {
            size *= 0.75;
        }
        let color = if link.is_some() { c.accent_text } else { rs.color };
        let start = r.text.len();
        r.text.push_str(&text);
        let range = start..r.text.len();
        let exact = exact && text.len() == source.len();
        r.map.push(MapSpan { text: range.clone(), source, exact });
        if style.code {
            r.code.push(range.clone());
        }
        if style.kbd {
            r.kbd.push(range.clone());
        }
        if style.strikethrough {
            r.strike.push(range.clone());
        }
        r.spans.push(
            Span::new(text)
                .font(font)
                .size(size)
                .line_height(LineHeight::Absolute(Pixels(rs.base.line_height)))
                .color(color),
        );
    }

    fn push_inlines(&self, r: &mut Runs, inlines: &[Inline], rs: &RunStyle, link: Option<&str>) {
        let c = self.look().colors;
        for inline in inlines {
            match inline {
                Inline::Text { text, style, source, exact } => {
                    self.push_text(r, text, *style, source.clone(), *exact, rs, link)
                }
                Inline::Link { url, children, .. } => {
                    let start = r.text.len();
                    self.push_inlines(r, children, rs, Some(url));
                    r.links.push((start..r.text.len(), url.clone()));
                }
                Inline::Image { alt, src, source, .. } => {
                    let label = if alt.is_empty() { src.clone() } else { alt.clone() };
                    let style = SpanStyle { emphasis: true, ..SpanStyle::default() };
                    let muted = RunStyle { color: c.muted, uppercase: false, ..*rs };
                    self.push_text(r, &format!("[{label}]"), style, source.clone(), false, &muted, link);
                }
                Inline::FootnoteRef { label, number, source } => {
                    let style = SpanStyle { superscript: true, ..SpanStyle::default() };
                    let accent = RunStyle { color: c.accent_text, uppercase: false, ..*rs };
                    let text = number.map_or_else(|| label.clone(), |n| n.to_string());
                    self.push_text(r, &text, style, source.clone(), false, &accent, link);
                }
                Inline::SoftBreak { source } => {
                    self.push_text(r, " ", SpanStyle::default(), source.clone(), false, rs, link)
                }
                Inline::HardBreak { source } => {
                    self.push_text(r, "\n", SpanStyle::default(), source.clone(), false, rs, link)
                }
                Inline::Math { tex, source, .. } => {
                    let style = SpanStyle { code: true, ..SpanStyle::default() };
                    let math = RunStyle { color: c.math, uppercase: false, ..*rs };
                    self.push_text(r, tex, style, source.clone(), false, &math, link);
                }
                Inline::Html { html, source } => {
                    let style = SpanStyle { code: true, ..SpanStyle::default() };
                    let muted = RunStyle { color: c.html, uppercase: false, ..*rs };
                    self.push_text(r, html, style, source.clone(), true, &muted, link);
                }
            }
        }
    }

    /// Shape runs into a text item at (x, y); returns its height.
    fn paragraph(&mut self, runs: Runs, rs: &RunStyle, x: f32, y: f32, width: f32, wrap: bool) -> (f32, f32) {
        let c = self.look().colors;
        let para = shape(&runs.spans, width, rs.base, rs.font, wrap);
        let mut item =
            text_item(Point::new(x, y), para, runs.text, runs.map, runs.links, rs.color, rs.base.line_height);
        for r in &runs.code {
            for rect in item.range_rects(r.start, r.end) {
                let rect = Rectangle::new(
                    Point::new(rect.x - 3.0, rect.y + rect.height * 0.12),
                    Size::new(rect.width + 6.0, rect.height * 0.76),
                );
                item.under.push((rect, c.bg_subtle, Border { radius: RADIUS_SM.into(), ..Border::default() }));
            }
        }
        for r in &runs.kbd {
            for rect in item.range_rects(r.start, r.end) {
                let rect = Rectangle::new(
                    Point::new(rect.x - 3.0, rect.y + rect.height * 0.12),
                    Size::new(rect.width + 6.0, rect.height * 0.76),
                );
                let border = Border { color: c.border_strong, width: 1.0, radius: RADIUS_SM.into() };
                item.under.push((rect, c.surface, border));
            }
        }
        for r in &runs.strike {
            for rect in item.range_rects(r.start, r.end) {
                let y = rect.y + rect.height * 0.55;
                item.over.push((Rectangle::new(Point::new(rect.x, y), Size::new(rect.width, 1.0)), rs.color));
            }
        }
        let b = item.bounds();
        self.items.push(Item::Text(item));
        (b.width, b.height)
    }

    fn body(&self, color: Color) -> RunStyle {
        let base = scaled(typography::BODY, self.ctx.zoom);
        RunStyle { base, font: self.look().font(&base), color, uppercase: false }
    }

    /// Lay out `node` at (x, y) in `width`; returns its height.
    fn node(&mut self, node: &LayoutNode, x: f32, y: f32, width: f32, color: Color) -> f32 {
        let z = self.ctx.zoom;
        let c = self.look().colors;
        match node {
            LayoutNode::Paragraph { inlines, .. } => {
                if let Some(h) = self.image_paragraph(inlines, x, y, width) {
                    return h;
                }
                let rs = self.body(color);
                let runs = self.runs(inlines, &rs);
                self.paragraph(runs, &rs, x, y, width, true).1
            }
            LayoutNode::Heading { level, inlines, .. } => {
                let base = scaled(typography::heading(*level), z);
                let color = if *level == 6 { c.secondary } else { color };
                let rs = RunStyle { base, font: self.look().font(&base), color, uppercase: false };
                let runs = self.runs(inlines, &rs);
                self.paragraph(runs, &rs, x, y, width, true).1
            }
            LayoutNode::BlockQuote { callout: Some(kind), children, .. } => {
                self.callout(*kind, children, node.source().start, x, y, width)
            }
            LayoutNode::BlockQuote { children, .. } => {
                let inset = 3.0 + SPACE_4;
                let h = self.children(children, x + inset, y, width - inset, c.secondary, SPACE_4);
                self.items.push(Item::Quad {
                    rect: Rectangle::new(Point::new(x, y), Size::new(3.0, h)),
                    color: c.border_strong,
                    border: Border::default(),
                });
                h
            }
            LayoutNode::List { start, tight, items, .. } => self.list(*start, *tight, items, x, y, width, color),
            LayoutNode::CodeBlock { lang: Some(lang), code, segments, source, .. }
                if mdedit_mermaid::is_mermaid(lang) =>
            {
                self.diagram(code, segments, source.start, x, y, width)
            }
            LayoutNode::CodeBlock { lang, code, segments, .. } => {
                let width = (self.ctx.wide - x).max(width);
                self.code_block(lang.as_deref(), code, segments, x, y, width, c.text)
            }
            LayoutNode::Html { html, segments, .. } => self.code_block(None, html, segments, x, y, width, c.html),
            LayoutNode::Table { align, header, rows, source } => {
                let width = (self.ctx.wide - x).max(width);
                self.table(align, header, rows, source.clone(), x, y, width)
            }
            LayoutNode::ThematicBreak { .. } => {
                let pad = SPACE_6 - SPACE_4;
                self.items.push(Item::Quad {
                    rect: Rectangle::new(Point::new(x, y + pad), Size::new(width, 1.0)),
                    color: c.border,
                    border: Border::default(),
                });
                2.0 * pad + 1.0
            }
            LayoutNode::FootnoteDefinition { label, number, children, .. } => {
                let label = &number.map_or_else(|| label.clone(), |n| n.to_string());
                let base = scaled(typography::UI, z);
                let rs = RunStyle { base, font: self.look().font(&base), color: c.accent_text, uppercase: false };
                let mut runs = self.runs(&[], &rs);
                let style = SpanStyle { superscript: true, ..SpanStyle::default() };
                self.push_text(&mut runs, label, style, node.source().start..node.source().start, false, &rs, None);
                let (label_w, _) = self.paragraph(runs, &rs, x, y, f32::INFINITY, false);
                let indent = SPACE_5.max(label_w + SPACE_2);
                let mut h = 0.0;
                for (i, child) in children.iter().enumerate() {
                    if i > 0 {
                        h += SPACE_2;
                    }
                    h += self.small_node(child, x + indent, y + h, width - indent);
                }
                h
            }
            LayoutNode::Custom { renderer, payload, .. } => {
                let seg = vec![mdedit_render::Segment {
                    text: 0..payload.len(),
                    source: node.source(),
                    exact: payload.len() == node.source().len(),
                }];
                self.code_block(Some(renderer), payload, &seg, x, y, width, c.text)
            }
            LayoutNode::Group { children, .. } => self.children(children, x, y, width, color, SPACE_4),
        }
    }

    /// A GitHub alert as a DESIGN_SYSTEM callout: `radius-lg` box, `eyebrow`
    /// title, info / warning / danger tones.
    fn callout(&mut self, kind: Callout, children: &[LayoutNode], at: usize, x: f32, y: f32, width: f32) -> f32 {
        let c = self.look().colors;
        let z = self.ctx.zoom;
        let (fill, tone) = match kind {
            Callout::Note => (c.bg_subtle, c.info),
            Callout::Tip => (c.bg_subtle, c.success),
            Callout::Important => (c.bg_subtle, c.accent_text),
            Callout::Warning => (c.warning_bg, c.warning_text),
            Callout::Caution => (Color { a: if self.look().dark { 0.18 } else { 0.10 }, ..c.danger }, c.danger),
        };
        let (pad_x, pad_y) = (SPACE_5, SPACE_4);
        // The box goes under the text, so push it first and fix its height.
        let quad = self.items.len();
        self.items.push(Item::Quad {
            rect: Rectangle::new(Point::new(x, y), Size::ZERO),
            color: fill,
            border: Border { radius: RADIUS_LG.into(), ..Border::default() },
        });
        let eyebrow = scaled(typography::EYEBROW, z);
        let rs = RunStyle { base: eyebrow, font: self.look().font(&eyebrow), color: tone, uppercase: true };
        let mut runs = self.runs(&[], &rs);
        self.push_text(&mut runs, kind.title(), SpanStyle::default(), at..at, false, &rs, None);
        let (_, title_h) = self.paragraph(runs, &rs, x + pad_x, y + pad_y, width - 2.0 * pad_x, false);
        let body_y = y + pad_y + title_h + SPACE_2;
        let body_h = self.children(children, x + pad_x, body_y, width - 2.0 * pad_x, c.text, SPACE_4);
        let h = body_y - y + body_h + pad_y;
        if let Item::Quad { rect, .. } = &mut self.items[quad] {
            *rect = Rectangle::new(Point::new(x, y), Size::new(width, h));
        }
        h
    }

    /// Footnote bodies use the `ui` size.
    fn small_node(&mut self, node: &LayoutNode, x: f32, y: f32, width: f32) -> f32 {
        let c = self.look().colors;
        if let LayoutNode::Paragraph { inlines, .. } = node {
            let base = scaled(typography::UI, self.ctx.zoom);
            let rs = RunStyle { base, font: self.look().font(&base), color: c.secondary, uppercase: false };
            let runs = self.runs(inlines, &rs);
            return self.paragraph(runs, &rs, x, y, width, true).1;
        }
        self.node(node, x, y, width, c.secondary)
    }

    fn children(&mut self, children: &[LayoutNode], x: f32, y: f32, width: f32, color: Color, gap: f32) -> f32 {
        let mut h = 0.0;
        for (i, child) in children.iter().enumerate() {
            if i > 0 {
                h += gap;
            }
            h += self.node(child, x, y + h, width, color);
        }
        h
    }

    #[allow(clippy::too_many_arguments)]
    fn list(
        &mut self,
        start: Option<u64>,
        tight: bool,
        items: &[ListItem],
        x: f32,
        y: f32,
        width: f32,
        color: Color,
    ) -> f32 {
        let c = self.look().colors;
        let z = self.ctx.zoom;
        let indent = 28.0 * z;
        let gap = if tight { SPACE_1 } else { SPACE_4 };
        let body = scaled(typography::BODY, z);
        let mut h = 0.0;
        for (n, item) in items.iter().enumerate() {
            if n > 0 {
                h += gap;
            }
            let top = y + h;
            if let Some(checked) = item.task {
                let size = 16.0 * z;
                let rect = Rectangle::new(
                    Point::new(x + indent - size - SPACE_2, top + (body.line_height - size) / 2.0),
                    Size::new(size, size),
                );
                let source = item.task_source.clone().unwrap_or(item.source.start..item.source.start);
                self.items.push(Item::Check { rect, checked, source });
            } else {
                let (label, label_color, style) = match start {
                    Some(s) => (format!("{}.", s + n as u64), c.secondary, body),
                    None => ("•".to_string(), c.muted, body),
                };
                let rs = RunStyle { base: style, font: self.look().font(&style), color: label_color, uppercase: false };
                let mut runs = self.runs(&[], &rs);
                let at = item.source.start;
                self.push_text(&mut runs, &label, SpanStyle::default(), at..at, false, &rs, None);
                let para = shape(&runs.spans, f32::INFINITY, rs.base, rs.font, false);
                let w = para.para.min_bounds().width;
                let pos = Point::new(x + indent - w - SPACE_2, top);
                self.items.push(Item::Text(text_item(
                    pos,
                    para,
                    runs.text,
                    runs.map,
                    Vec::new(),
                    label_color,
                    style.line_height,
                )));
            }
            let mut ih = 0.0;
            for (i, child) in item.children.iter().enumerate() {
                if i > 0 {
                    ih += if tight { SPACE_1 } else { SPACE_4 };
                }
                let child_color = if item.task == Some(true) { c.muted } else { color };
                ih += self.node(child, x + indent, top + ih, width - indent, child_color);
            }
            h += ih.max(body.line_height);
        }
        h
    }

    #[allow(clippy::too_many_arguments)]
    fn code_block(
        &mut self,
        lang: Option<&str>,
        code: &str,
        segments: &[mdedit_render::Segment],
        x: f32,
        y: f32,
        width: f32,
        color: Color,
    ) -> f32 {
        let c = self.look().colors;
        let z = self.ctx.zoom;
        let pad = SPACE_4;
        let base = scaled(typography::CODE, z);
        let font = self.look().mono;
        // The final line ending would show as an empty last line.
        let shown = code.strip_suffix('\n').unwrap_or(code);
        let shown = shown.strip_suffix('\r').unwrap_or(shown);
        let mut map: Vec<MapSpan> = segments
            .iter()
            .filter(|s| s.text.start < shown.len() || s.text.is_empty())
            .map(|s| {
                let end = s.text.end.min(shown.len());
                let cut = s.text.end - end;
                let exact = s.exact && cut == 0 || s.exact && s.source.len() >= cut;
                let source_end = if s.exact { s.source.end - cut } else { s.source.end };
                MapSpan { text: s.text.start..end, source: s.source.start..source_end, exact }
            })
            .collect();
        if map.is_empty() {
            // Empty code block: put the caret at its content start.
            let at = segments.first().map_or(0, |s| s.source.start);
            map.push(MapSpan { text: 0..0, source: at..at, exact: true });
        }
        let spans = vec![Span::new(shown.to_string()).font(font).color(color)];
        let inner = width - 2.0 * pad;
        let para = shape(&spans, inner, base, font, true);
        let item =
            text_item(Point::new(x + pad, y + pad), para, shown.to_string(), map, Vec::new(), color, base.line_height);
        let h = item.bounds().height + 2.0 * pad;
        self.items.push(Item::Quad {
            rect: Rectangle::new(Point::new(x, y), Size::new(width, h)),
            color: c.bg_subtle,
            border: Border { radius: RADIUS_LG.into(), ..Border::default() },
        });
        self.items.push(Item::Text(item));
        if let Some(lang) = lang.filter(|l| !l.is_empty()) {
            let eyebrow = scaled(typography::EYEBROW, z);
            let rs = RunStyle { base: eyebrow, font: self.look().font(&eyebrow), color: c.muted, uppercase: true };
            let mut runs = self.runs(&[], &rs);
            let at = segments.first().map_or(0, |s| s.source.start);
            self.push_text(&mut runs, lang, SpanStyle::default(), at..at, false, &rs, None);
            let para = shape(&runs.spans, f32::INFINITY, rs.base, rs.font, false);
            let w = para.para.min_bounds().width;
            let pos = Point::new(x + width - pad - w, y + SPACE_2);
            self.items.push(Item::Text(text_item(
                pos,
                para,
                runs.text,
                runs.map,
                Vec::new(),
                c.muted,
                eyebrow.line_height,
            )));
            let size = 20.0 * z;
            self.items.push(Item::Copy {
                rect: Rectangle::new(
                    Point::new(x + width - pad - w - SPACE_2 - size, y + SPACE_1),
                    Size::new(size, size),
                ),
                code: code.to_string(),
            });
        } else {
            let size = 20.0 * z;
            self.items.push(Item::Copy {
                rect: Rectangle::new(Point::new(x + width - SPACE_2 - size, y + SPACE_1), Size::new(size, size)),
                code: code.to_string(),
            });
        }
        h
    }

    fn diagram_state(&self, code: &str, at: usize) -> DiagramState {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut slot = DefaultHasher::new();
        (self.ctx.doc_path, at).hash(&mut slot);
        diagrams::cache().get(code, &self.look().mermaid, slot.finish())
    }

    /// A Mermaid block as its diagram; as code with the error under it when
    /// the source doesn't render.
    #[allow(clippy::too_many_arguments)]
    fn diagram(
        &mut self,
        code: &str,
        segments: &[mdedit_render::Segment],
        at: usize,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let c = self.look().colors;
        let z = self.ctx.zoom;
        self.images = true;
        match self.diagram_state(code, at) {
            DiagramState::Ready(d) | DiagramState::Loading { stale: Some(d) } => self.place_diagram(&d, x, y, width),
            DiagramState::Loading { stale: None } => {
                let h = 120.0 * z;
                self.items.push(Item::Quad {
                    rect: Rectangle::new(Point::new(x, y), Size::new(width, h)),
                    color: c.bg_subtle,
                    border: Border { color: c.border, width: 1.0, radius: RADIUS_LG.into() },
                });
                let base = scaled(typography::UI, z);
                let rs = RunStyle { base, font: self.look().font(&base), color: c.muted, uppercase: false };
                let mut runs = self.runs(&[], &rs);
                self.push_text(&mut runs, "Rendering diagram", SpanStyle::default(), at..at, false, &rs, None);
                let ty = y + (h - base.line_height) / 2.0;
                self.paragraph(runs, &rs, x + SPACE_4, ty, width - 2.0 * SPACE_4, false);
                h
            }
            DiagramState::Failed { error, .. } => {
                let wide = (self.ctx.wide - x).max(width);
                let h = self.code_block(Some("mermaid"), code, segments, x, y, wide, c.text);
                let end = segments.last().map_or(at, |s| s.source.end);
                h + SPACE_2 + self.diagram_error(&error, end, x, y + h + SPACE_2, width)
            }
        }
    }

    /// Draw a diagram at its natural size × zoom, shrunk to fit and centred
    /// in the column (wide ones may use the wider measure).
    fn place_diagram(&mut self, d: &diagrams::Ready, x: f32, y: f32, width: f32) -> f32 {
        let z = self.ctx.zoom;
        let avail = (self.ctx.wide - x).max(width);
        let scale = z.min(avail / d.width.max(1.0));
        let (dw, dh) = (d.width * scale, d.height * scale);
        let left = if dw <= width { x + (width - dw) / 2.0 } else { x };
        self.items
            .push(Item::Svg { rect: Rectangle::new(Point::new(left, y), Size::new(dw, dh)), handle: d.handle.clone() });
        dh
    }

    fn diagram_error(&mut self, error: &str, at: usize, x: f32, y: f32, width: f32) -> f32 {
        let c = self.look().colors;
        let base = scaled(typography::UI_SMALL, self.ctx.zoom);
        let rs = RunStyle { base, font: self.look().font(&base), color: c.danger, uppercase: false };
        let mut runs = self.runs(&[], &rs);
        let label = format!("Diagram error: {error}");
        self.push_text(&mut runs, &label, SpanStyle::default(), at..at, false, &rs, None);
        self.paragraph(runs, &rs, x, y, width, true).1
    }

    /// A paragraph of nothing but images (and the breaks between them) is
    /// laid out as image boxes.
    fn image_paragraph(&mut self, inlines: &[Inline], x: f32, y: f32, width: f32) -> Option<f32> {
        let only_images = inlines.iter().all(|i| match i {
            Inline::Image { .. } | Inline::SoftBreak { .. } | Inline::HardBreak { .. } => true,
            Inline::Text { text, .. } => text.trim().is_empty(),
            _ => false,
        });
        if !only_images || !inlines.iter().any(|i| matches!(i, Inline::Image { .. })) {
            return None;
        }
        let c = self.look().colors;
        let z = self.ctx.zoom;
        let mut h = 0.0;
        for inline in inlines {
            let Inline::Image { src, alt, title, source } = inline else { continue };
            if h > 0.0 {
                h += SPACE_4;
            }
            self.images = true;
            let state = match images::resolve(src, self.ctx.doc_path) {
                Some(path) => images::cache().get(&path, width.ceil() as u32),
                None => ImageState::Failed("remote image".into()),
            };
            match state {
                ImageState::Ready { handle, width: w, height: ih } => {
                    let dw = (w as f32).min(width);
                    let dh = ih as f32 * dw / w.max(1) as f32;
                    self.items
                        .push(Item::Image { rect: Rectangle::new(Point::new(x, y + h), Size::new(dw, dh)), handle });
                    h += dh;
                }
                ImageState::Loading | ImageState::Failed(_) => {
                    let loading = matches!(state, ImageState::Loading);
                    let bh = if loading { 160.0 * z } else { 56.0 * z };
                    self.items.push(Item::Quad {
                        rect: Rectangle::new(Point::new(x, y + h), Size::new(width, bh)),
                        color: c.bg_subtle,
                        border: Border { color: c.border, width: 1.0, radius: RADIUS_LG.into() },
                    });
                    let label = match &state {
                        ImageState::Failed(e) => format!("{} ({e})", if alt.is_empty() { src } else { alt }),
                        _ => format!("Loading {}", if alt.is_empty() { src } else { alt }),
                    };
                    let base = scaled(typography::UI, z);
                    let rs = RunStyle { base, font: self.look().font(&base), color: c.muted, uppercase: false };
                    let mut runs = self.runs(&[], &rs);
                    self.push_text(&mut runs, &label, SpanStyle::default(), source.clone(), false, &rs, None);
                    let ty = y + h + (bh - base.line_height) / 2.0;
                    self.paragraph(runs, &rs, x + SPACE_4, ty, width - 2.0 * SPACE_4, false);
                    h += bh;
                }
            }
            if !title.is_empty() {
                let base = scaled(typography::UI_SMALL, z);
                let rs = RunStyle { base, font: self.look().font(&base), color: c.muted, uppercase: false };
                let mut runs = self.runs(&[], &rs);
                self.push_text(&mut runs, title, SpanStyle::default(), source.end..source.end, false, &rs, None);
                h += SPACE_2;
                h += self.paragraph(runs, &rs, x, y + h, width, true).1;
            }
        }
        Some(h)
    }

    #[allow(clippy::too_many_arguments)]
    fn table(
        &mut self,
        align: &[Align],
        header: &mdedit_render::TableRow,
        rows: &[mdedit_render::TableRow],
        source: Range<usize>,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let c = self.look().colors;
        let z = self.ctx.zoom;
        let (pad_x, pad_y) = (SPACE_4, SPACE_3);
        let cols = std::iter::once(header).chain(rows).map(|r| r.cells.len()).max().unwrap_or(0).max(1);
        let header_style = {
            let base = scaled(typography::EYEBROW, z);
            RunStyle { base, font: self.look().font(&base), color: c.secondary, uppercase: true }
        };
        let body_style = self.body(c.text);
        let editing = self.ctx.editable;
        let cursor = self.ctx.cursor.filter(|&p| source.start <= p && p <= source.end);
        // Cell content ranges (trimmed of padding and pipes).
        let all_rows: Vec<&mdedit_render::TableRow> = std::iter::once(header).chain(rows).collect();
        let contents: Vec<Vec<Range<usize>>> = all_rows
            .iter()
            .map(|r| r.cells.iter().map(|cell| trim_cell(self.ctx.buf, cell.source.clone())).collect())
            .collect();
        let active = cursor.and_then(|p| {
            contents
                .iter()
                .enumerate()
                .find_map(|(ri, row)| row.iter().position(|r| r.start <= p && p <= r.end).map(|ci| (ri, ci)))
        });
        // Runs for every cell; the active cell shows its source.
        let mut cell_runs: Vec<Vec<(Runs, bool)>> = Vec::new();
        for (ri, row) in all_rows.iter().enumerate() {
            let rs = if ri == 0 { &header_style } else { &body_style };
            let mut v = Vec::new();
            for (ci, cell) in row.cells.iter().enumerate() {
                if active == Some((ri, ci)) {
                    let src = contents[ri][ci].clone();
                    let text = self.ctx.buf.slice(src.clone()).into_owned();
                    let mut runs = self.runs(&[], &body_style);
                    self.push_text(&mut runs, &text, SpanStyle::default(), src, true, &body_style, None);
                    v.push((runs, true));
                } else {
                    v.push((self.runs(&cell.inlines, rs), false));
                }
            }
            cell_runs.push(v);
        }
        // Column widths: natural widths, shrunk to fit or stretched to the
        // column measure.
        let mut natural = vec![24.0f32; cols];
        for (ri, row) in cell_runs.iter().enumerate() {
            let rs = if ri == 0 { &header_style } else { &body_style };
            for (ci, (runs, _)) in row.iter().enumerate() {
                let p = shape(&runs.spans, f32::INFINITY, rs.base, rs.font, false).para;
                natural[ci] = natural[ci].max(p.min_bounds().width + 2.0);
            }
        }
        let avail = width - cols as f32 * 2.0 * pad_x;
        let total: f32 = natural.iter().sum();
        let target = if total < self.ctx.width - cols as f32 * 2.0 * pad_x {
            self.ctx.width - cols as f32 * 2.0 * pad_x
        } else {
            total.min(avail)
        };
        let widths: Vec<f32> = natural.iter().map(|w| (w / total * target).max(24.0)).collect();
        let table_w: f32 = widths.iter().sum::<f32>() + cols as f32 * 2.0 * pad_x;

        let mark = self.items.len();
        let mut cells = Vec::new();
        let mut h = 0.0;
        let mut row_tops = Vec::new();
        for (ri, row) in cell_runs.into_iter().enumerate() {
            let rs = if ri == 0 { &header_style } else { &body_style };
            row_tops.push(h);
            let mut cx = x;
            let mut row_h: f32 = 0.0;
            for (ci, (runs, source_mode)) in row.into_iter().enumerate() {
                let w = widths.get(ci).copied().unwrap_or(24.0);
                let style = if source_mode { &body_style } else { rs };
                // Alignment: shift a single-row cell inside its column.
                let shift = {
                    let p = shape(&runs.spans, f32::INFINITY, style.base, style.font, false).para;
                    let nat = p.min_bounds().width.min(w);
                    match align.get(ci) {
                        Some(Align::Center) if !source_mode => (w - nat) / 2.0,
                        Some(Align::Right) if !source_mode => w - nat,
                        _ => 0.0,
                    }
                };
                let (_, th) = self.paragraph(runs, style, cx + pad_x + shift.max(0.0), y + h + pad_y, w, true);
                row_h = row_h.max(th.max(style.base.line_height) + 2.0 * pad_y);
                let content = contents[ri].get(ci).cloned().unwrap_or(source.end..source.end);
                cells.push(Cell {
                    rect: Rectangle::new(Point::new(cx, y + h), Size::new(w + 2.0 * pad_x, 0.0)),
                    content,
                    row: ri,
                    col: ci,
                });
                cx += w + 2.0 * pad_x;
            }
            for cell in cells.iter_mut().filter(|c| c.row == ri) {
                cell.rect.height = row_h;
            }
            h += row_h;
        }
        // Frame, header band and row rules go under the text.
        let mut under = vec![
            Item::Quad {
                rect: Rectangle::new(Point::new(x, y), Size::new(table_w, h)),
                color: c.bg,
                border: Border { color: c.border, width: 1.0, radius: RADIUS_LG.into() },
            },
            Item::Quad {
                rect: Rectangle::new(
                    Point::new(x + 1.0, y + 1.0),
                    Size::new(table_w - 2.0, row_tops.get(1).copied().unwrap_or(h) - 1.0),
                ),
                color: c.bg_subtle,
                border: Border {
                    radius: Radius { top_left: RADIUS_LG - 1.0, top_right: RADIUS_LG - 1.0, ..Radius::default() },
                    ..Border::default()
                },
            },
        ];
        for top in row_tops.iter().skip(1) {
            under.push(Item::Quad {
                rect: Rectangle::new(Point::new(x, y + top), Size::new(table_w, 1.0)),
                color: c.border,
                border: Border::default(),
            });
        }
        if let Some((ri, ci)) = active
            && let Some(cell) = cells.iter().find(|c| c.row == ri && c.col == ci)
        {
            under.push(Item::Quad {
                rect: pad(cell.rect, -1.0, -1.0),
                color: Color::TRANSPARENT,
                border: Border { color: c.accent, width: 2.0, radius: RADIUS_SM.into() },
            });
        }
        let mine = self.items.split_off(mark);
        self.items.extend(under);
        self.items.extend(mine);

        let toolbar = if editing && cursor.is_some() { self.toolbar(x, y) } else { Vec::new() };
        self.table = Some(TableInfo {
            cells,
            cols,
            rows: all_rows.len(),
            align: (0..cols).map(|i| align.get(i).copied().unwrap_or(Align::None)).collect(),
            source,
            toolbar,
        });
        h
    }

    fn toolbar(&self, x: f32, y: f32) -> Vec<(Rectangle, TableOp, TextItem)> {
        let c = self.look().colors;
        let base = scaled(typography::UI_SMALL, self.ctx.zoom);
        let font = self.look().font(&base);
        let ops = [
            ("+ Row above", TableOp::RowAbove),
            ("+ Row below", TableOp::RowBelow),
            ("+ Col left", TableOp::ColLeft),
            ("+ Col right", TableOp::ColRight),
            ("Delete row", TableOp::DeleteRow),
            ("Delete col", TableOp::DeleteCol),
            ("Left", TableOp::Align(Align::Left)),
            ("Center", TableOp::Align(Align::Center)),
            ("Right", TableOp::Align(Align::Right)),
        ];
        let height = 24.0 * self.ctx.zoom;
        let mut cx = x;
        let mut out = Vec::new();
        for (label, op) in ops {
            let spans = vec![Span::new(label.to_string()).font(font).color(c.secondary)];
            let para = shape(&spans, f32::INFINITY, base, font, false);
            let w = para.para.min_bounds().width + 2.0 * SPACE_2;
            let rect = Rectangle::new(Point::new(cx, y - height - SPACE_2), Size::new(w, height));
            let pos = Point::new(cx + SPACE_2, rect.y + (height - base.line_height) / 2.0);
            let item = text_item(pos, para, label.to_string(), Vec::new(), Vec::new(), c.secondary, base.line_height);
            out.push((rect, op, item));
            cx += w + 2.0;
        }
        out
    }
}

/// A cell's content range: its source without the padding spaces and the
/// surrounding pipes.
pub(crate) fn trim_cell(buf: &dyn mdedit_core::TextBuffer, range: Range<usize>) -> Range<usize> {
    let s = buf.slice(range.clone());
    let lead = s.len() - s.trim_start_matches([' ', '\t', '|']).len();
    let mut end = s.len();
    let bytes = s.as_bytes();
    while end > lead {
        let b = bytes[end - 1];
        let escaped_pipe = b == b'|' && end >= 2 && bytes[end - 2] == b'\\';
        if (b == b' ' || b == b'\t' || b == b'|') && !escaped_pipe {
            end -= 1;
        } else {
            break;
        }
    }
    range.start + lead..range.start + end
}

#[cfg(test)]
mod reveal_tests {
    use super::soft_break;

    #[test]
    fn soft_breaks_join_running_text_only() {
        assert_eq!(soft_break("some text", "more text"), Some(0));
        assert_eq!(soft_break("> quoted", "> more"), Some(2));
        assert_eq!(soft_break("- item", "  continued"), Some(2));
        assert_eq!(soft_break("- item", "- next"), None);
        assert_eq!(soft_break("hard  ", "break"), None);
        assert_eq!(soft_break("hard\\", "break"), None);
        assert_eq!(soft_break("> quote", "lazy"), None);
        assert_eq!(soft_break("# Heading", "text"), None);
        assert_eq!(soft_break("text", "1. item"), None);
    }
}
