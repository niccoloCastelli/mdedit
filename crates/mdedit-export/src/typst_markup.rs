//! Markdown tree → Typst markup (SPEC 10, PDF).
//!
//! Two files come out: `conf.typ` (the template options as a Typst
//! dictionary) and `main.typ` (imports the template, then the body). The
//! body uses plain Typst markup for headings and paragraphs and calls
//! `md-*` functions for everything a template may want to restyle; the
//! built-in `templates/mdedit.typ` defines them all and documents the
//! contract for custom `template.typ` files.

use std::collections::HashSet;
use std::fmt::Write;

use crate::Settings;
use crate::ast::{Align, Block, Doc, Inline, ListItem};
use crate::highlight::highlight;
use crate::template::{NumberStyle, Style, page_size_mm, parse_hex};

/// Virtual paths the generated files live at in the Typst world.
pub const CONF_PATH: &str = "/__mdedit/conf.typ";
pub const TEMPLATE_PATH: &str = "/__mdedit/template.typ";
pub const MAIN_PATH: &str = "/__mdedit/main.typ";

pub struct TypstFiles {
    pub conf: String,
    pub main: String,
}

/// `image` maps a markdown image URL to a path in the Typst world, or
/// `None` when the image is not available (its alt text is shown instead).
pub fn convert(doc: &Doc, s: &Settings, image: &mut dyn FnMut(&str) -> Option<String>) -> TypstFiles {
    let conf = conf(doc, s);
    let mut w =
        Writer { out: String::new(), slugs: doc.slugs(), doc, s, image, footnotes_seen: HashSet::new(), code_end: 0 };
    w.out.push_str(&format!("#import \"{TEMPLATE_PATH}\": *\n#show: md-document\n\n"));
    if let Some(title) = &doc.title {
        let mut blocks = doc.blocks.as_slice();
        w.out.push_str("#md-title(");
        if s.template.lead
            && let Some(Block::Paragraph(lead)) = blocks.first()
        {
            w.out.push_str("lead: [");
            w.inlines(lead);
            w.out.push_str("], ");
            blocks = &blocks[1..];
        }
        w.out.push_str(")[");
        w.inlines(title);
        w.out.push_str("]\n\n");
        if s.template.toc {
            w.out.push_str("#md-toc()\n\n");
        }
        w.blocks(blocks);
    } else {
        if s.template.toc {
            w.out.push_str("#md-toc()\n\n");
        }
        w.blocks(&doc.blocks);
    }
    let mut main = w.out;
    while main.ends_with("\n\n") {
        main.pop();
    }
    TypstFiles { conf, main }
}

fn conf(doc: &Doc, s: &Settings) -> String {
    let t = &s.template;
    let title = doc.title_text(&s.title);
    let paper = page_size_mm(&t.page_size).map(|p| p.2).unwrap_or("a4");
    let [top, right, bottom, left] = t.margins_mm;
    let color = |c: &str| format!("rgb(\"#{}\")", parse_hex(c).unwrap_or_else(|| "000000".into()));
    let c = &t.colors;
    let style = match t.style {
        Style::Guide => "guide",
        Style::Report => "report",
        Style::Plain => "plain",
    };
    let number_style = match t.number_style {
        NumberStyle::Decimal => "decimal",
        NumberStyle::Guide => "guide",
    };
    let mut out = String::from("// Template options, generated from template.toml.\n#let conf = (\n");
    let mut field = |name: &str, value: String| {
        let _ = writeln!(out, "  {name}: {value},");
    };
    field("style", string(style));
    field("paper", string(paper));
    field("margin", format!("(top: {top}mm, right: {right}mm, bottom: {bottom}mm, left: {left}mm)"));
    field("font-body", string(&t.font_body));
    field("font-heading", string(t.heading_font()));
    field("font-mono", string(&t.font_mono));
    field("size", format!("{}pt", t.base_size_pt));
    field("line-height", format!("{}", t.line_height));
    field("accent", color(&t.accent));
    field("text", color(&c.text));
    field("text-secondary", color(&c.text_secondary));
    field("text-muted", color(&c.text_muted));
    field("accent-text", color(&c.accent_text));
    field("accent-soft", color(&c.accent_soft));
    field("border", color(&c.border));
    field("bg-subtle", color(&c.bg_subtle));
    field("warning-bg", color(&c.warning_bg));
    field("warning-text", color(&c.warning_text));
    field("heading-numbers", t.heading_numbers.to_string());
    field("number-style", string(number_style));
    field("title-page", t.title_page.to_string());
    field("toc", t.toc.to_string());
    field("title", string(&title));
    field("date", string(&s.date));
    field("eyebrow", placeholders(&t.eyebrow, &title, &s.date));
    field("header", parts(&t.header, &title, &s.date));
    field("footer", parts(&t.footer, &title, &s.date));
    out.push_str(")\n");
    out
}

/// Header/footer text as a `(left, centre, right)` array of content.
fn parts(text: &str, title: &str, date: &str) -> String {
    let pieces: Vec<&str> = text.split('|').map(str::trim).collect();
    let (l, c, r) = match pieces.as_slice() {
        [] | [""] => ("", "", ""),
        [c] => ("", *c, ""),
        [l, r] => (*l, "", *r),
        [l, c, r, ..] => (*l, *c, *r),
    };
    format!("({}, {}, {})", placeholders(l, title, date), placeholders(c, title, date), placeholders(r, title, date))
}

/// Text with `{title}`, `{date}`, `{page}`, `{pages}` as Typst content.
fn placeholders(text: &str, title: &str, date: &str) -> String {
    if text.is_empty() {
        return "none".into();
    }
    let mut out = String::from("[");
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        escape_into(&mut out, &rest[..i], false);
        rest = &rest[i..];
        let replaced = [
            ("{title}", None, title),
            ("{date}", None, date),
            ("{pages}", Some("#context counter(page).final().first()"), ""),
            ("{page}", Some("#context counter(page).display()"), ""),
        ]
        .into_iter()
        .find(|(key, ..)| rest.starts_with(key));
        match replaced {
            Some((key, code, value)) => {
                match code {
                    Some(code) => out.push_str(code),
                    None => escape_into(&mut out, value, false),
                }
                rest = &rest[key.len()..];
            }
            None => {
                out.push_str("\\{");
                rest = &rest[1..];
            }
        }
    }
    escape_into(&mut out, rest, false);
    out.push(']');
    out
}

/// A Typst string literal.
pub(crate) fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Append `text` escaped for Typst markup. `line_start` says whether the
/// output is at the start of a line, where list, heading and term markers
/// would otherwise be recognised.
fn escape_into(out: &mut String, text: &str, mut line_start: bool) {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        let escape = match c {
            '\\' | '#' | '$' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '@' | '~' | '{' | '}' => true,
            // Comments, en/em dash and soft hyphen shorthands.
            '/' => matches!(next, Some('/' | '*')) || line_start,
            '-' => matches!(next, Some('-' | '?')) || line_start,
            '=' | '+' => line_start,
            '0'..='9' if line_start => {
                // `1.` starts a numbered list.
                let rest: String = chars.clone().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                if rest.contains('.') {
                    out.push(c);
                    for d in chars.by_ref().take_while(|c| c.is_ascii_digit()) {
                        out.push(d);
                    }
                    out.push_str("\\.");
                    line_start = false;
                    continue;
                }
                false
            }
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
        if c == '\n' {
            line_start = true;
        } else if !c.is_whitespace() {
            line_start = false;
        }
    }
}

struct Writer<'a> {
    out: String,
    slugs: HashSet<&'a str>,
    doc: &'a Doc,
    s: &'a Settings,
    image: &'a mut dyn FnMut(&str) -> Option<String>,
    footnotes_seen: HashSet<String>,
    /// `out.len()` right after the last inline `#expression`.
    code_end: usize,
}

impl Writer<'_> {
    fn at_line_start(&self) -> bool {
        let trimmed = self.out.trim_end_matches([' ', '\t']);
        trimmed.is_empty() || trimmed.ends_with('\n') || trimmed.ends_with('[')
    }

    fn text(&mut self, text: &str) {
        // `#f(x).y` and `#f(x)(y)` would continue the expression.
        if self.out.len() == self.code_end && text.starts_with(['.', '(']) {
            self.out.push(';');
        }
        let line_start = self.at_line_start();
        escape_into(&mut self.out, text, line_start);
    }

    fn blocks(&mut self, blocks: &[Block]) {
        for (i, block) in blocks.iter().enumerate() {
            if i > 0 {
                self.out.push_str("\n\n");
            }
            self.block(block);
        }
    }

    /// Blocks inside `[...]`; a single paragraph stays on one line.
    fn content(&mut self, blocks: &[Block]) {
        self.out.push('[');
        self.blocks(blocks);
        self.out.push(']');
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Heading { level, content, slug } => {
                for _ in 0..*level {
                    self.out.push('=');
                }
                self.out.push(' ');
                self.inlines(content);
                let _ = write!(self.out, " <{}>", label(slug));
            }
            Block::Paragraph(inlines) => self.inlines(inlines),
            Block::Figure { url, title, alt } => match (self.image)(url) {
                Some(path) => {
                    let _ = write!(self.out, "#md-figure({}, alt: {}", string(&path), string(alt));
                    if !title.is_empty() {
                        self.out.push_str(", caption: [");
                        self.text(title);
                        self.out.push(']');
                    }
                    self.out.push(')');
                }
                None => self.missing_image(alt),
            },
            Block::Quote(blocks) => {
                self.out.push_str("#quote(block: true)");
                self.content(blocks);
            }
            Block::Callout { kind, title, body } => {
                let _ = write!(self.out, "#md-callout(kind: {}, title: [", string(kind));
                self.inlines(title);
                self.out.push_str("])");
                self.content(body);
            }
            Block::Code { lang, text } => {
                let _ = write!(self.out, "#md-code(lang: {}, (", string(lang));
                for line in highlight(text, lang, &self.s.template.code_theme) {
                    self.out.push_str("\n  (");
                    for span in line {
                        let color = span.color.map(|c| string(&format!("#{c}"))).unwrap_or_else(|| "none".into());
                        let flags = match (span.bold, span.italic) {
                            (true, true) => "bi",
                            (true, false) => "b",
                            (false, true) => "i",
                            (false, false) => "",
                        };
                        let _ = write!(self.out, "({}, {color}, {}), ", string(&span.text), string(flags));
                    }
                    self.out.push_str("),");
                }
                self.out.push_str("\n))");
            }
            Block::List { start, tight, items } => self.list(*start, *tight, items),
            Block::Table { aligns, head, rows } => {
                self.out.push_str("#md-table(\n  align: (");
                for a in aligns {
                    self.out.push_str(match a {
                        Align::None => "auto",
                        Align::Left => "left",
                        Align::Center => "center",
                        Align::Right => "right",
                    });
                    self.out.push_str(", ");
                }
                self.out.push_str("),\n  header: ");
                self.row(head);
                self.out.push_str(",\n");
                for row in rows {
                    self.out.push_str("  ");
                    self.row(row);
                    self.out.push_str(",\n");
                }
                self.out.push(')');
            }
            Block::Rule => self.out.push_str("#md-rule()"),
        }
    }

    fn row(&mut self, cells: &[Vec<Inline>]) {
        self.out.push('(');
        for cell in cells {
            self.out.push('[');
            self.inlines(cell);
            self.out.push_str("], ");
        }
        self.out.push(')');
    }

    fn list(&mut self, start: Option<u64>, tight: bool, items: &[ListItem]) {
        let tasks = items.iter().any(|i| i.task.is_some());
        if tasks {
            let _ = write!(self.out, "#md-tasks(tight: {tight},");
            for item in items {
                let state = match item.task {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "none",
                };
                let _ = write!(self.out, "\n  ({state}, ");
                self.content(&item.blocks);
                self.out.push_str("),");
            }
        } else {
            match start {
                Some(n) => {
                    let _ = write!(self.out, "#enum(start: {n}, tight: {tight},");
                }
                None => {
                    let _ = write!(self.out, "#list(tight: {tight},");
                }
            }
            for item in items {
                self.out.push_str("\n  ");
                self.content(&item.blocks);
                self.out.push(',');
            }
        }
        self.out.push_str("\n)");
    }

    fn missing_image(&mut self, alt: &str) {
        self.out.push_str("#md-missing-image[");
        self.text(alt);
        self.out.push(']');
    }

    fn inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(inline);
        }
    }

    fn inline(&mut self, inline: &Inline) {
        let before = self.out.len();
        self.inline_inner(inline);
        if self.out[before..].starts_with('#') && !self.out.ends_with(['\n', '>']) {
            self.code_end = self.out.len();
        }
    }

    fn inline_inner(&mut self, inline: &Inline) {
        match inline {
            Inline::Text(t) => self.text(t),
            Inline::Code(code) => {
                if code.contains('`') || code.is_empty() {
                    let _ = write!(self.out, "#raw({})", string(code));
                } else {
                    let _ = write!(self.out, "`{code}`");
                }
            }
            Inline::Emph(c) => self.wrapped("#emph[", c),
            Inline::Strong(c) => self.wrapped("#strong[", c),
            Inline::Strike(c) => self.wrapped("#strike[", c),
            Inline::Link { url, content } => {
                if let Some(anchor) = url.strip_prefix('#') {
                    if self.slugs.contains(anchor) {
                        let _ = write!(self.out, "#link(<{}>)[", label(anchor));
                        self.inlines(content);
                        self.out.push(']');
                    } else {
                        self.inlines(content);
                    }
                } else {
                    let _ = write!(self.out, "#link({})[", string(url));
                    self.inlines(content);
                    self.out.push(']');
                }
            }
            Inline::Image { url, alt, .. } => match (self.image)(url) {
                Some(path) => {
                    let _ = write!(self.out, "#md-image({}, alt: {})", string(&path), string(alt));
                }
                None => self.missing_image(alt),
            },
            Inline::FootnoteRef(name) => {
                let fn_label = format!("fn-{}", label(name));
                if !self.footnotes_seen.insert(name.clone()) {
                    let _ = write!(self.out, "#footnote(<{fn_label}>)");
                } else if let Some(blocks) = self.doc.footnotes.get(name) {
                    self.out.push_str("#footnote");
                    self.content(blocks);
                    let _ = write!(self.out, " <{fn_label}>");
                } else {
                    self.text(&format!("[^{name}]"));
                }
            }
            Inline::Comment { author, body } => {
                if self.s.include_comments {
                    let _ = write!(self.out, "#md-comment(author: {})[", string(author));
                    self.text(body);
                    self.out.push(']');
                }
            }
            Inline::SoftBreak => self.out.push('\n'),
            Inline::HardBreak => self.out.push_str("#linebreak()\n"),
        }
    }

    fn wrapped(&mut self, open: &str, content: &[Inline]) {
        self.out.push_str(open);
        self.inlines(content);
        self.out.push(']');
    }
}

/// A Typst label name: letters, digits, `-`, `_`, `.`, `:`.
fn label(s: &str) -> String {
    let out: String =
        s.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':') { c } else { '-' }).collect();
    if out.is_empty() { "x".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(s: &str, line_start: bool) -> String {
        let mut out = String::new();
        escape_into(&mut out, s, line_start);
        out
    }

    #[test]
    fn escaping() {
        assert_eq!(esc("a*b_c #x $y [z] <l> @r ~ `c`", false), r"a\*b\_c \#x \$y \[z\] \<l\> \@r \~ \`c\`");
        assert_eq!(esc("- item", true), r"\- item");
        assert_eq!(esc("a - b -- c", false), r"a - b \-- c");
        assert_eq!(esc("12. May", true), r"12\. May");
        assert_eq!(esc("12 May", true), "12 May");
        assert_eq!(esc("= x\n+ y\n/ z", true), "\\= x\n\\+ y\n\\/ z");
        assert_eq!(esc("a // b", false), r"a \// b");
        assert_eq!(esc(r"c:\path", false), r"c:\\path");
    }

    #[test]
    fn strings_and_labels() {
        assert_eq!(string("a\"b\\c\nd"), r#""a\"b\\c\nd""#);
        assert_eq!(label("my heading!"), "my-heading-");
    }

    #[test]
    fn placeholder_content() {
        assert_eq!(
            placeholders("{title} p. {page}/{pages} {x}", "T*", "D"),
            "[T\\* p. #context counter(page).display()/#context counter(page).final().first() \\{x\\}]"
        );
        assert_eq!(parts("a | b", "", ""), "([a], none, [b])");
        assert_eq!(parts("", "", ""), "(none, none, none)");
    }
}
