//! Markdown tree → DOCX with `docx-rs` (SPEC 10). Every paragraph and run
//! uses a named style created from the template, so a user can restyle the
//! document in Word by editing styles. Units: sizes in half-points,
//! spacing and widths in twentieths of a point (twips), images in EMU.

use std::io::Cursor;

use docx_rs::{
    AbstractNumbering, AlignmentType, BorderType, BreakType, Docx, FieldCharType, Footer, Footnote, Header, Hyperlink, HyperlinkType,
    IndentLevel, InstrText, Level, LevelJc, LevelOverride, LevelText, LineSpacing, LineSpacingType, NumPages, NumberFormat,
    Numbering, NumberingId, PageMargin, PageNum, Paragraph, ParagraphBorder, ParagraphBorderPosition, ParagraphBorders, ParagraphChild,
    Pic, Run, RunFonts, Shading, ShdType, SpecialIndentType, Start, Style, StyleType, Tab, TabValueType,
    Table, TableBorder, TableBorderPosition, TableBorders, TableCell, TableCellMargins, TableRow,
    VertAlignType, WidthType,
};

use crate::ast::{Align, Block, Doc, Inline, ListItem, plain_text};
use crate::highlight::highlight;
use crate::template::{NumberStyle, Style as Look, Template, page_size_mm, parse_hex};
use crate::{ExportError, Settings};

/// The XML parts golden tests compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocxParts {
    pub document: String,
    pub styles: String,
    pub numbering: String,
    pub footnotes: String,
}

pub(crate) type ImageLoader<'a> = dyn FnMut(&str) -> Option<Vec<u8>> + Send + 'a;

/// docx-rs numbers paragraphs from a per-thread counter it does not let
/// us reset, so each document is built on a fresh thread: the same input
/// always gives the same XML.
fn on_fresh_thread<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| scope.spawn(f).join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
}

pub(crate) fn pack(doc: &Doc, s: &Settings, image: &mut ImageLoader) -> Result<Vec<u8>, ExportError> {
    on_fresh_thread(|| {
        let mut buf = Cursor::new(Vec::new());
        build(doc, s, image).build().pack(&mut buf).map_err(|e| ExportError::Docx(e.to_string()))?;
        Ok(buf.into_inner())
    })
}

pub(crate) fn parts(doc: &Doc, s: &Settings, image: &mut ImageLoader) -> DocxParts {
    on_fresh_thread(|| {
        let xml = build(doc, s, image).build();
        let s = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
        DocxParts {
            document: s(xml.document),
            styles: s(xml.styles),
            numbering: s(xml.numberings),
            footnotes: s(xml.footnotes),
        }
    })
}

const BULLETS: usize = 1;
const DECIMAL: usize = 2;
/// Twips per mm.
const TWIPS_MM: f32 = 56.692_9;
const EMU_PER_TWIP: u32 = 635;
const EMU_PER_PX: u32 = 9525;
/// List indent per level, twips.
const LIST_INDENT: i32 = 360;

fn build(doc: &Doc, s: &Settings, image: &mut ImageLoader) -> Docx {
    let t = &s.template;
    let (page_w, page_h, _) = page_size_mm(&t.page_size).unwrap_or((210.0, 297.0, "a4"));
    let [top, right, bottom, left] = t.margins_mm.map(|mm| (mm * TWIPS_MM).round() as i32);
    let page_w = (page_w * TWIPS_MM).round() as i32;
    let text_width = (page_w - left - right).max(1440);
    let title = doc.title_text(&s.title);

    let mut docx = Docx::new()
        .page_size(page_w as u32, (page_h * TWIPS_MM).round() as u32)
        .page_margin(
            PageMargin::new().top(top).right(right).bottom(bottom).left(left).header(top / 2).footer(bottom / 2),
        )
        .default_fonts(fonts(&t.font_body))
        .default_size(half_points(t.base_size_pt, 1.0))
        .created_at("2000-01-01T00:00:00Z")
        .updated_at("2000-01-01T00:00:00Z")
        .add_abstract_numbering(bullet_numbering(t))
        .add_abstract_numbering(decimal_numbering(t))
        .add_numbering(Numbering::new(BULLETS, BULLETS));
    for style in styles(t) {
        docx = docx.add_style(style);
    }
    if let Some(header) = running(&t.header, &title, &s.date, text_width) {
        docx = docx.header(Header::new().add_paragraph(header));
    }
    if let Some(footer) = running(&t.footer, &title, &s.date, text_width) {
        docx = docx.footer(Footer::new().add_paragraph(footer));
    }

    let mut w = Writer {
        out: Vec::new(),
        doc,
        s,
        image,
        text_width,
        next_footnote: 1,
        next_numbering: 3,
        numberings: Vec::new(),
        next_pic: 1,
        next_bookmark: 1,
        counters: [0; 6],
        slugs: doc.slugs().into_iter().map(String::from).collect(),
    };

    let mut blocks = doc.blocks.as_slice();
    if let Some(title_inlines) = &doc.title {
        if !t.eyebrow.is_empty() {
            let eyebrow = fill_placeholders(&t.eyebrow, &title, &s.date).to_uppercase();
            w.out.push(Child::P(Paragraph::new().style("Eyebrow").add_run(Run::new().add_text(eyebrow))));
        }
        let p = w.inlines(Paragraph::new().style("Title"), title_inlines, Fmt::default());
        w.out.push(Child::P(p));
        if t.lead
            && let Some(Block::Paragraph(lead)) = blocks.first()
        {
            let p = w.inlines(Paragraph::new().style("Subtitle"), lead, Fmt::default());
            w.out.push(Child::P(p));
            blocks = &blocks[1..];
        }
        if t.title_page {
            w.out.push(Child::P(Paragraph::new().style("Date").add_run(Run::new().add_text(s.date.clone()))));
            if !t.toc {
                w.out.push(Child::P(page_break()));
            }
        }
    }
    if t.toc {
        w.toc();
        w.out.push(Child::P(if t.title_page { page_break() } else { Paragraph::new() }));
    }
    w.blocks(blocks, &Ctx::default());

    for (id, start) in std::mem::take(&mut w.numberings) {
        docx = docx.add_numbering(
            Numbering::new(id, DECIMAL).add_override(LevelOverride::new(0).start(start as usize)),
        );
    }
    for child in w.out {
        docx = match child {
            Child::P(p) => docx.add_paragraph(p),
            Child::T(table) => docx.add_table(table),
        };
    }
    docx
}

enum Child {
    P(Paragraph),
    T(Table),
}

#[derive(Default, Clone)]
struct Ctx {
    /// Paragraph style for body paragraphs (quote, callout, table cell).
    style: Option<&'static str>,
    list_depth: usize,
}

#[derive(Default, Clone, Copy)]
struct Fmt {
    bold: bool,
    italic: bool,
    strike: bool,
}

struct Writer<'a> {
    out: Vec<Child>,
    doc: &'a Doc,
    s: &'a Settings,
    image: &'a mut ImageLoader<'a>,
    text_width: i32,
    next_footnote: usize,
    next_numbering: usize,
    /// (numbering id, start) for each ordered list.
    numberings: Vec<(usize, u64)>,
    next_pic: usize,
    next_bookmark: usize,
    counters: [usize; 6],
    slugs: Vec<String>,
}

impl Writer<'_> {
    fn blocks(&mut self, blocks: &[Block], ctx: &Ctx) {
        for block in blocks {
            self.block(block, ctx);
        }
    }

    /// Heading number text, `None` when the heading is not numbered.
    fn heading_number(&self, counters: &[usize; 6], level: usize) -> Option<String> {
        let t = &self.s.template;
        match t.number_style {
            _ if !t.heading_numbers => None,
            NumberStyle::Guide if level == 1 => Some(format!("{:02}", counters[0])),
            NumberStyle::Guide => None,
            NumberStyle::Decimal => {
                Some(counters[..level].iter().map(|c| c.to_string()).collect::<Vec<_>>().join("."))
            }
        }
    }

    /// A TOC field whose cached result lists the headings (levels 1-3)
    /// without page numbers: readers that do not update fields show the
    /// list, Word refreshes it with page numbers on open.
    fn toc(&mut self) {
        self.out.push(Child::P(Paragraph::new().style("TOCHeading").add_run(Run::new().add_text("CONTENTS"))));
        let mut counters = [0usize; 6];
        let mut entries = Vec::new();
        for block in &self.doc.blocks {
            if let Block::Heading { level, content, slug } = block {
                let level = (*level).clamp(1, 6) as usize;
                counters[level - 1] += 1;
                counters[level..].iter_mut().for_each(|c| *c = 0);
                if level <= 3 {
                    let number = self.heading_number(&counters, level);
                    entries.push((level, number, plain_text(content), slug.clone()));
                }
            }
        }
        let n = entries.len();
        let begin = Run::new()
            .add_field_char(FieldCharType::Begin, true)
            .add_instr_text(InstrText::Unsupported("TOC \\o \"1-3\" \\h \\z \\u".into()))
            .add_field_char(FieldCharType::Separate, false);
        let end = Run::new().add_field_char(FieldCharType::End, false);
        if n == 0 {
            self.out.push(Child::P(Paragraph::new().add_run(begin).add_run(end)));
            return;
        }
        let mut begin = Some(begin);
        for (i, (level, number, text, slug)) in entries.into_iter().enumerate() {
            let mut p = Paragraph::new().style(&format!("TOC{level}"));
            if let Some(begin) = begin.take() {
                p = p.add_run(begin);
            }
            let label = match number {
                Some(number) => format!("{number}  {text}"),
                None => text,
            };
            p = p.add_hyperlink(Hyperlink::new(slug, HyperlinkType::Anchor).add_run(Run::new().add_text(label)));
            if i + 1 == n {
                p = p.add_run(end.clone());
            }
            self.out.push(Child::P(p));
        }
    }

    fn body_paragraph(&self, ctx: &Ctx) -> Paragraph {
        Paragraph::new().style(ctx.style.unwrap_or("BodyText"))
    }

    fn block(&mut self, block: &Block, ctx: &Ctx) {
        let t = &self.s.template;
        match block {
            Block::Heading { level, content, slug } => {
                let level = (*level).clamp(1, 6) as usize;
                self.counters[level - 1] += 1;
                for c in &mut self.counters[level..] {
                    *c = 0;
                }
                let mut p = Paragraph::new().style(&format!("Heading{level}"));
                let id = self.next_bookmark;
                self.next_bookmark += 1;
                p = p.add_bookmark_start(id, slug.clone());
                if let Some(n) = self.heading_number(&self.counters, level) {
                    if t.number_style == NumberStyle::Guide {
                        let number = Paragraph::new().style("SectionNumber").add_run(Run::new().add_text(n));
                        self.out.push(Child::P(number));
                    } else {
                        p = p.add_run(Run::new().style("HeadingNumber").add_text(format!("{n}  ")));
                    }
                }
                p = self.inlines(p, content, Fmt::default());
                self.out.push(Child::P(p.add_bookmark_end(id)));
            }
            Block::Paragraph(inlines) => {
                let p = self.body_paragraph(ctx);
                let p = self.inlines(p, inlines, Fmt::default());
                self.out.push(Child::P(p));
            }
            Block::Figure { url, title, alt } => {
                let p = Paragraph::new().style("Figure");
                let p = match self.picture(url, self.text_width) {
                    Some(pic) => p.add_run(Run::new().add_image(pic)),
                    None => p.add_run(Run::new().style("Missing").add_text(format!("[{alt}]"))),
                };
                self.out.push(Child::P(p));
                if !title.is_empty() {
                    self.out.push(Child::P(Paragraph::new().style("Caption").add_run(Run::new().add_text(title))));
                }
            }
            Block::Quote(blocks) => {
                let inner = Ctx { style: Some("Quote"), ..ctx.clone() };
                self.blocks(blocks, &inner);
            }
            Block::Callout { kind, title, body } => {
                let warn = matches!(kind.as_str(), "warning" | "caution" | "important");
                let (title_style, body_style) =
                    if warn { ("WarningTitle", "Warning") } else { ("CalloutTitle", "Callout") };
                let p = self.inlines(Paragraph::new().style(title_style), title, Fmt::default());
                self.out.push(Child::P(p));
                self.blocks(body, &Ctx { style: Some(body_style), ..ctx.clone() });
            }
            Block::Code { lang, text } => {
                let lines = highlight(text, lang, &t.code_theme);
                let n = lines.len();
                for (i, line) in lines.into_iter().enumerate() {
                    let mut p = Paragraph::new().style("SourceCode");
                    let mut spacing = LineSpacing::new();
                    if i == 0 {
                        spacing = spacing.before(160);
                    }
                    if i + 1 == n {
                        spacing = spacing.after(200);
                    }
                    if i == 0 || i + 1 == n {
                        p = p.line_spacing(spacing);
                    }
                    for span in line {
                        let mut run = Run::new().add_text(span.text);
                        if let Some(color) = span.color {
                            run = run.color(color);
                        }
                        if span.bold {
                            run = run.bold();
                        }
                        if span.italic {
                            run = run.italic();
                        }
                        p = p.add_run(run);
                    }
                    self.out.push(Child::P(p));
                }
            }
            Block::List { start, tight, items } => self.list(*start, *tight, items, ctx),
            Block::Table { aligns, head, rows } => self.table(aligns, head, rows),
            Block::Rule => self.out.push(Child::P(Paragraph::new().style("Rule"))),
        }
    }

    fn list(&mut self, start: Option<u64>, tight: bool, items: &[ListItem], ctx: &Ctx) {
        let depth = ctx.list_depth;
        let num_id = start.map(|start| {
            let id = self.next_numbering;
            self.next_numbering += 1;
            self.numberings.push((id, start));
            id
        });
        let indent = LIST_INDENT * (depth as i32 + 1);
        for item in items {
            let inner = Ctx { list_depth: depth + 1, ..ctx.clone() };
            let mut first = true;
            for block in &item.blocks {
                match block {
                    Block::Paragraph(inlines) => {
                        let mut p = Paragraph::new().style(if tight { "Compact" } else { "BodyText" });
                        if first {
                            match (item.task, num_id) {
                                (Some(checked), _) => {
                                    let mark = if checked { "\u{2611}" } else { "\u{2610}" };
                                    p = p
                                        .indent(Some(indent), Some(SpecialIndentType::Hanging(LIST_INDENT)), None, None)
                                        .add_run(Run::new().style("TaskMark").add_text(mark).add_tab());
                                }
                                (None, Some(id)) => {
                                    p = p.numbering(NumberingId::new(id), IndentLevel::new(depth.min(8)));
                                }
                                (None, None) => {
                                    p = p.numbering(NumberingId::new(BULLETS), IndentLevel::new(depth.min(8)));
                                }
                            }
                        } else {
                            p = p.indent(Some(indent), None, None, None);
                        }
                        let p = self.inlines(p, inlines, Fmt::default());
                        self.out.push(Child::P(p));
                    }
                    other => self.block(other, &inner),
                }
                first = false;
            }
        }
    }

    fn table(&mut self, aligns: &[Align], head: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>]) {
        let guide = self.s.template.style == Look::Guide;
        let cols = head.len().max(1);
        let col_width = (self.text_width as usize) / cols;
        let border = color(&self.s.template.colors.border);
        let mut table_rows = Vec::new();
        for (r, row) in std::iter::once(head).chain(rows.iter().map(Vec::as_slice)).enumerate() {
            let mut cells = Vec::new();
            for c in 0..cols {
                let style = if r == 0 { "TableHeading" } else { "TableText" };
                let mut p = Paragraph::new().style(style);
                p = match aligns.get(c) {
                    Some(Align::Center) => p.align(AlignmentType::Center),
                    Some(Align::Right) => p.align(AlignmentType::Right),
                    _ => p,
                };
                if let Some(cell) = row.get(c) {
                    p = self.inlines(p, cell, Fmt::default());
                }
                let mut cell = TableCell::new().add_paragraph(p).width(col_width, WidthType::Dxa);
                if r == 0 && !guide {
                    let fill = color(&self.s.template.colors.bg_subtle);
                    cell = cell.shading(Shading::new().shd_type(ShdType::Clear).color("auto").fill(fill));
                }
                cells.push(cell);
            }
            table_rows.push(TableRow::new(cells));
        }
        let edge = |pos, on: bool| match on {
            true => TableBorder::new(pos).border_type(BorderType::Single).size(6).color(border.clone()),
            false => TableBorder::new(pos).border_type(BorderType::None).size(0).color("auto"),
        };
        let borders = TableBorders::new()
            .set(edge(TableBorderPosition::Top, true))
            .set(edge(TableBorderPosition::Bottom, true))
            .set(edge(TableBorderPosition::InsideH, true))
            .set(edge(TableBorderPosition::Left, guide))
            .set(edge(TableBorderPosition::Right, guide))
            .set(edge(TableBorderPosition::InsideV, false));
        let table = Table::new(table_rows)
            .set_grid(vec![col_width; cols])
            .width(col_width * cols, WidthType::Dxa)
            .margins(TableCellMargins::new().margin(120, 160, 120, 160))
            .set_borders(borders);
        self.out.push(Child::T(table));
        // Space after the table.
        self.out.push(Child::P(Paragraph::new().style("Compact")));
    }

    fn picture(&mut self, url: &str, max_width_twips: i32) -> Option<Pic> {
        let bytes = (self.image)(url)?;
        let (png, w, h) = to_png(&bytes)?;
        let id = self.next_pic;
        self.next_pic += 1;
        let (mut w_emu, mut h_emu) = (w * EMU_PER_PX, h * EMU_PER_PX);
        let max = max_width_twips as u32 * EMU_PER_TWIP;
        if w_emu > max {
            h_emu = (h_emu as u64 * max as u64 / w_emu as u64) as u32;
            w_emu = max;
        }
        Some(Pic::new_with_dimensions(png, w, h).id(format!("rIdImage{id}")).size(w_emu, h_emu))
    }

    fn inlines(&mut self, mut p: Paragraph, inlines: &[Inline], fmt: Fmt) -> Paragraph {
        for inline in inlines {
            p = self.inline(p, inline, fmt);
        }
        p
    }

    fn run(text: &str, fmt: Fmt) -> Run {
        let mut run = Run::new().add_text(text);
        if fmt.bold {
            run = run.bold();
        }
        if fmt.italic {
            run = run.italic();
        }
        if fmt.strike {
            run = run.strike();
        }
        run
    }

    fn inline(&mut self, p: Paragraph, inline: &Inline, fmt: Fmt) -> Paragraph {
        match inline {
            Inline::Text(text) => p.add_run(Self::run(text, fmt)),
            Inline::Code(code) => p.add_run(Self::run(code, fmt).style("VerbatimChar")),
            Inline::Emph(c) => self.inlines(p, c, Fmt { italic: true, ..fmt }),
            Inline::Strong(c) => self.inlines(p, c, Fmt { bold: true, ..fmt }),
            Inline::Strike(c) => self.inlines(p, c, Fmt { strike: true, ..fmt }),
            Inline::Link { url, content } => {
                let (target, kind) = match url.strip_prefix('#') {
                    Some(anchor) if self.slugs.iter().any(|s| s == anchor) => (anchor, HyperlinkType::Anchor),
                    Some(_) => return self.inlines(p, content, fmt),
                    None => (url.as_str(), HyperlinkType::External),
                };
                let inner = self.inlines(Paragraph::new(), content, fmt);
                let mut link = Hyperlink::new(target, kind);
                link.children = inner
                    .children
                    .into_iter()
                    .map(|child| match child {
                        ParagraphChild::Run(run) => ParagraphChild::Run(Box::new(run.style("Hyperlink"))),
                        other => other,
                    })
                    .collect();
                p.add_hyperlink(link)
            }
            Inline::Image { url, alt, .. } => {
                // Inline images are drawn at text height.
                let height = (self.s.template.base_size_pt * 1.2 * 20.0) as i32;
                match self.picture(url, self.text_width) {
                    Some(pic) => {
                        let (w, h) = pic.size;
                        let h_new = height as u32 * EMU_PER_TWIP;
                        let w_new = (w as u64 * h_new as u64 / h.max(1) as u64) as u32;
                        p.add_run(Run::new().add_image(pic.size(w_new, h_new)))
                    }
                    None => p.add_run(Run::new().style("Missing").add_text(format!("[{alt}]"))),
                }
            }
            Inline::FootnoteRef(name) => match self.doc.footnotes.get(name) {
                Some(blocks) => {
                    let paragraphs = self.footnote_paragraphs(blocks);
                    self.footnote(p, paragraphs)
                }
                None => p.add_run(Self::run(&format!("[^{name}]"), fmt)),
            },
            Inline::Comment { author, body } => {
                if !self.s.include_comments {
                    return p;
                }
                let note = Paragraph::new()
                    .style("FootnoteText")
                    .add_run(Run::new().bold().add_text(format!("{author}: ")))
                    .add_run(Run::new().add_text(body));
                self.footnote(p, vec![note])
            }
            Inline::SoftBreak => p.add_run(Self::run(" ", fmt)),
            Inline::HardBreak => p.add_run(Run::new().add_break(BreakType::TextWrapping)),
        }
    }

    fn footnote(&mut self, p: Paragraph, content: Vec<Paragraph>) -> Paragraph {
        let id = self.next_footnote;
        self.next_footnote += 1;
        let footnote = Footnote { id, content };
        p.add_run(Run::new().add_footnote_reference(footnote))
    }

    /// Footnote bodies are flattened to paragraphs of text.
    fn footnote_paragraphs(&mut self, blocks: &[Block]) -> Vec<Paragraph> {
        let mut out = Vec::new();
        for block in blocks {
            let text = match block {
                Block::Paragraph(inlines) => {
                    // Nested footnotes and comments are not supported in notes.
                    let flat: Vec<Inline> = inlines
                        .iter()
                        .filter(|i| !matches!(i, Inline::FootnoteRef(_) | Inline::Comment { .. }))
                        .cloned()
                        .collect();
                    out.push(self.inlines(Paragraph::new().style("FootnoteText"), &flat, Fmt::default()));
                    continue;
                }
                Block::Heading { content, .. } => plain_text(content),
                Block::Code { text, .. } => text.clone(),
                _ => continue,
            };
            out.push(Paragraph::new().style("FootnoteText").add_run(Run::new().add_text(text)));
        }
        if out.is_empty() {
            out.push(Paragraph::new().style("FootnoteText"));
        }
        out
    }
}

fn page_break() -> Paragraph {
    Paragraph::new().add_run(Run::new().add_break(BreakType::Page))
}

/// Header or footer: `left | centre | right` with tab stops.
fn running(text: &str, title: &str, date: &str, width: i32) -> Option<Paragraph> {
    if text.trim().is_empty() {
        return None;
    }
    let pieces: Vec<&str> = text.split('|').map(str::trim).collect();
    let parts: [&str; 3] = match pieces.as_slice() {
        [c] => ["", c, ""],
        [l, r] => [l, "", r],
        [l, c, r, ..] => [l, c, r],
        [] => return None,
    };
    let mut p = Paragraph::new()
        .style("HeaderFooter")
        .add_tab(Tab::new().val(TabValueType::Center).pos((width / 2) as usize))
        .add_tab(Tab::new().val(TabValueType::Right).pos(width as usize));
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            p = p.add_run(Run::new().add_tab());
        }
        let mut rest = *part;
        while !rest.is_empty() {
            let (before, field, after) = match (rest.find("{page}"), rest.find("{pages}")) {
                (Some(a), Some(b)) if b <= a => (&rest[..b], Some(false), &rest[b + 7..]),
                (Some(a), _) => (&rest[..a], Some(true), &rest[a + 6..]),
                (None, Some(b)) => (&rest[..b], Some(false), &rest[b + 7..]),
                (None, None) => (rest, None, ""),
            };
            if !before.is_empty() {
                p = p.add_run(Run::new().add_text(fill_placeholders(before, title, date)));
            }
            p = match field {
                Some(true) => p.add_page_num(PageNum::new()),
                Some(false) => p.add_num_pages(NumPages::new()),
                None => p,
            };
            rest = after;
        }
    }
    Some(p)
}

fn fill_placeholders(text: &str, title: &str, date: &str) -> String {
    text.replace("{title}", title).replace("{date}", date)
}

/// Decode any supported image to PNG bytes and pixel size.
fn to_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    if reader.format() == Some(image::ImageFormat::Png) {
        let (w, h) = reader.into_dimensions().ok()?;
        return Some((bytes.to_vec(), w, h));
    }
    let img = reader.decode().ok()?;
    let mut png = Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some((png.into_inner(), img.width(), img.height()))
}

fn color(hex: &str) -> String {
    parse_hex(hex).unwrap_or_else(|| "000000".into()).to_ascii_uppercase()
}

fn half_points(base_pt: f32, scale: f32) -> usize {
    (base_pt * scale * 2.0).round() as usize
}

fn fonts(family: &str) -> RunFonts {
    RunFonts::new().ascii(family).hi_ansi(family).cs(family).east_asia(family)
}

fn border(pos: ParagraphBorderPosition, size: usize, space: usize, color: &str) -> ParagraphBorder {
    ParagraphBorder::new(pos).val(BorderType::Single).size(size).space(space).color(color)
}

fn bullet_numbering(t: &Template) -> AbstractNumbering {
    let muted = color(&t.colors.text_muted);
    let mut n = AbstractNumbering::new(BULLETS);
    for level in 0..9 {
        let indent = LIST_INDENT * (level as i32 + 1);
        n = n.add_level(
            Level::new(level, Start::new(1), NumberFormat::new("bullet"), LevelText::new("\u{2022}"), LevelJc::new("left"))
                .indent(Some(indent), Some(SpecialIndentType::Hanging(LIST_INDENT)), None, None)
                .color(muted.clone()),
        );
    }
    n
}

fn decimal_numbering(t: &Template) -> AbstractNumbering {
    let secondary = color(&t.colors.text_secondary);
    let mut n = AbstractNumbering::new(DECIMAL);
    for level in 0..9 {
        let indent = LIST_INDENT * (level as i32 + 1);
        let mut l = Level::new(
            level,
            Start::new(1),
            NumberFormat::new("decimal"),
            LevelText::new(format!("%{}.", level + 1)),
            LevelJc::new("left"),
        )
        .indent(Some(indent), Some(SpecialIndentType::Hanging(LIST_INDENT)), None, None)
        .color(secondary.clone());
        if t.style == Look::Guide {
            l = l.color(color(&t.colors.accent_text)).bold();
        }
        n = n.add_level(l);
    }
    n
}

/// Paragraph spacing (before, after) and line height for a style.
fn spacing(before: u32, after: u32, line: Option<f32>) -> LineSpacing {
    let s = LineSpacing::new().before(before).after(after);
    match line {
        Some(l) => s.line((l * 240.0).round() as i32).line_rule(LineSpacingType::Auto),
        None => s,
    }
}

/// Every named style, built from the template.
fn styles(t: &Template) -> Vec<Style> {
    let base = t.base_size_pt;
    let c = &t.colors;
    let text = color(&c.text);
    let secondary = color(&c.text_secondary);
    let muted = color(&c.text_muted);
    let accent = color(&t.accent);
    let accent_text = color(&c.accent_text);
    let border_color = color(&c.border);
    let subtle = color(&c.bg_subtle);
    let guide = t.style == Look::Guide;
    let para = |id: &str, name: &str| Style::new(id, StyleType::Paragraph).name(name).q_format(true);
    let chars = |id: &str, name: &str| Style::new(id, StyleType::Character).name(name);
    let shade = |fill: &str| Shading::new().shd_type(ShdType::Clear).color("auto").fill(fill);
    let after = (base * 0.8 * 20.0) as u32;
    let line = Some(t.line_height);

    let mut styles = vec![
        para("Normal", "Normal").fonts(fonts(&t.font_body)).size(half_points(base, 1.0)).color(text.clone()).line_spacing(
            spacing(0, after, line),
        ),
        para("BodyText", "Body Text").based_on("Normal"),
        para("Compact", "Compact").based_on("Normal").line_spacing(spacing(0, (base * 0.3 * 20.0) as u32, line)),
        para("Title", "Title")
            .based_on("Normal")
            .next("BodyText")
            .fonts(fonts(t.heading_font()))
            .size(half_points(base, if t.title_page { 2.6 } else { 2.1 }))
            .bold()
            .line_spacing(spacing(if t.title_page { 2880 } else { 0 }, (base * 0.6 * 20.0) as u32, Some(1.1))),
        para("Subtitle", "Subtitle").based_on("Normal").size(half_points(base, 1.2)).color(secondary.clone()).line_spacing(
            spacing(0, (base * 1.4 * 20.0) as u32, Some(1.35)),
        ),
        para("Eyebrow", "Eyebrow")
            .based_on("Normal")
            .size(half_points(base, 0.75))
            .bold()
            .color(accent_text.clone())
            .line_spacing(spacing(0, 120, None)),
        para("Date", "Date").based_on("Normal").color(muted.clone()),
        para("SectionNumber", "Section Number")
            .based_on("Normal")
            .next("Heading1")
            .fonts(fonts(&t.font_mono))
            .size(half_points(base, 0.85))
            .color(accent.clone())
            .line_spacing(spacing((base * 2.0 * 20.0) as u32, 40, None)),
        para("Quote", "Quote").based_on("Normal").color(secondary.clone()).indent(Some(360), None, None, None),
        para("Callout", "Callout").based_on("Normal").indent(Some(240), None, Some(240), None),
        para("CalloutTitle", "Callout Title")
            .based_on("Callout")
            .size(half_points(base, 0.75))
            .bold()
            .color(accent_text.clone())
            .line_spacing(spacing((base * 0.8 * 20.0) as u32, 80, None)),
        para("Warning", "Warning").based_on("Callout"),
        para("WarningTitle", "Warning Title").based_on("CalloutTitle").color(color(&c.warning_text)),
        para("SourceCode", "Source Code")
            .based_on("Normal")
            .fonts(fonts(&t.font_mono))
            .size(half_points(base, 0.85))
            .color(secondary.clone())
            .line_spacing(spacing(0, 0, Some(1.25))),
        para("Figure", "Figure").based_on("Normal").align(AlignmentType::Center),
        para("Caption", "Caption").based_on("Normal").size(half_points(base, 0.85)).color(muted.clone()).align(
            AlignmentType::Center,
        ),
        para("TableText", "Table Text").based_on("Normal").line_spacing(spacing(0, 0, None)),
        para("TableHeading", "Table Heading")
            .based_on("TableText")
            .size(half_points(base, 0.75))
            .bold()
            .color(secondary.clone()),
        para("Rule", "Rule").based_on("Normal"),
        para("FootnoteText", "footnote text").based_on("Normal").size(half_points(base, 0.85)).line_spacing(spacing(
            0,
            0,
            None,
        )),
        para("HeaderFooter", "Header Footer").based_on("Normal").size(half_points(base, 0.8)).color(muted.clone()),
        para("TOCHeading", "TOC Heading")
            .based_on("Normal")
            .size(half_points(base, 0.85))
            .bold()
            .color(accent_text.clone())
            .line_spacing(spacing(240, 120, None)),
        para("TOC1", "toc 1").based_on("Normal").bold().line_spacing(spacing(60, 20, None)),
        para("TOC2", "toc 2").based_on("Normal").indent(Some(360), None, None, None).line_spacing(spacing(0, 20, None)),
        para("TOC3", "toc 3").based_on("Normal").indent(Some(720), None, None, None).line_spacing(spacing(0, 20, None)),
        chars("VerbatimChar", "Verbatim Char").fonts(fonts(&t.font_mono)).size(half_points(base, 0.9)),
        chars("Hyperlink", "Hyperlink").color(accent_text.clone()).underline("single"),
        chars("FootnoteReference", "footnote reference").color(accent_text.clone()),
        chars("HeadingNumber", "Heading Number").color(if guide { accent.clone() } else { secondary.clone() }),
        chars("TaskMark", "Task Mark").color(accent.clone()),
        chars("Missing", "Missing Image").color(muted.clone()),
    ];

    let sizes = [1.6, 1.3, 1.12, 1.0, 1.0, 1.0];
    for level in 1..=6usize {
        let before = if level == 1 { base * 2.4 } else { base * 1.6 };
        let before = if guide && level == 1 { base * 0.2 } else { before };
        let mut style = para(&format!("Heading{level}"), &format!("heading {level}"))
            .based_on("Normal")
            .next("BodyText")
            .fonts(fonts(t.heading_font()))
            .size(half_points(base, if guide && level == 1 { 1.75 } else { sizes[level - 1] }))
            .color(if level == 6 { secondary.clone() } else { text.clone() })
            .outline_lvl(level - 1)
            .line_spacing(spacing((before * 20.0) as u32, (base * 0.8 * 20.0) as u32, Some(1.15)));
        if level <= 3 {
            style = style.bold();
        }
        style.paragraph_property = style.paragraph_property.keep_next(true);
        styles.push(style);
    }

    for style in &mut styles {
        let pp = std::mem::take(&mut style.paragraph_property);
        // `set_border` would start from all four default borders.
        let only = |positions: &[ParagraphBorderPosition], size, space| {
            positions.iter().fold(ParagraphBorders::with_empty(), |b, pos| {
                b.set(border(pos.clone(), size, space, &border_color))
            })
        };
        style.paragraph_property = match style.style_id.as_str() {
            "SectionNumber" => pp.set_borders(only(&[ParagraphBorderPosition::Top], 6, 18)),
            "Quote" => pp.set_borders(only(&[ParagraphBorderPosition::Left], 18, 10)),
            "Callout" | "Warning" => {
                let fill = if style.style_id == "Warning" { color(&c.warning_bg) } else { subtle.clone() };
                pp.shading(shade(&fill)).set_borders(only(
                    &[
                        ParagraphBorderPosition::Top,
                        ParagraphBorderPosition::Bottom,
                        ParagraphBorderPosition::Left,
                        ParagraphBorderPosition::Right,
                    ],
                    6,
                    8,
                ))
            }
            "SourceCode" => pp.shading(shade(&subtle)),
            "Rule" => pp.set_borders(only(&[ParagraphBorderPosition::Bottom], 6, 1)),
            _ => pp,
        };
        if style.style_id == "VerbatimChar" {
            style.run_property = std::mem::take(&mut style.run_property).shading(shade(&subtle));
        }
        if style.style_id == "FootnoteReference" {
            style.run_property = std::mem::take(&mut style.run_property).vert_align(VertAlignType::SuperScript);
        }
        if matches!(style.style_id.as_str(), "Eyebrow" | "CalloutTitle" | "TableHeading") {
            style.run_property = std::mem::take(&mut style.run_property).caps();
        }
    }
    styles
}
