//! The layout model as HTML, in pulldown-cmark's dialect (copy as HTML,
//! and the spec tests compare this with pulldown-cmark's renderer).
//!
//! Inline styles are flags in the model, so nested styles come out in a
//! fixed order (`strong`, `em`, `del`, `sup`, `sub`, `kbd`, `code`) and
//! are reopened per text run.

use std::collections::HashMap;
use std::fmt::Write;

use pulldown_cmark_escape::{escape_href, escape_html, escape_html_body_text};

use crate::{Align, Inline, LayoutNode, SpanStyle};

/// Write top-level nodes as one HTML document. Footnotes are numbered in
/// order of first use across all nodes.
pub fn to_html<'a>(nodes: impl IntoIterator<Item = &'a LayoutNode>) -> String {
    let mut w = Writer::default();
    for node in nodes {
        w.node(node, false);
    }
    w.out
}

#[derive(Default)]
struct Writer {
    out: String,
    footnotes: HashMap<String, usize>,
}

type StyleTag = (fn(&SpanStyle) -> bool, &'static str);

const TAGS: [StyleTag; 7] = [
    (|s| s.strong, "strong"),
    (|s| s.emphasis, "em"),
    (|s| s.strikethrough, "del"),
    (|s| s.superscript, "sup"),
    (|s| s.subscript, "sub"),
    (|s| s.kbd, "kbd"),
    (|s| s.code, "code"),
];

impl Writer {
    fn footnote_number(&mut self, label: &str) -> usize {
        let next = self.footnotes.len() + 1;
        *self.footnotes.entry(label.to_string()).or_insert(next)
    }

    fn node(&mut self, node: &LayoutNode, tight: bool) {
        match node {
            LayoutNode::Paragraph { inlines, .. } if tight => self.inlines(inlines),
            LayoutNode::Paragraph { inlines, .. } => {
                self.out.push_str("<p>");
                self.inlines(inlines);
                self.out.push_str("</p>\n");
            }
            LayoutNode::Heading { level, inlines, id, classes, attrs, .. } => {
                let _ = write!(self.out, "<h{level}");
                if let Some(id) = id {
                    self.out.push_str(" id=\"");
                    let _ = escape_html(&mut self.out, id);
                    self.out.push('"');
                }
                if !classes.is_empty() {
                    self.out.push_str(" class=\"");
                    let _ = escape_html(&mut self.out, &classes.join(" "));
                    self.out.push('"');
                }
                for (k, v) in attrs {
                    self.out.push(' ');
                    let _ = escape_html(&mut self.out, k);
                    self.out.push_str("=\"");
                    let _ = escape_html(&mut self.out, v.as_deref().unwrap_or(""));
                    self.out.push('"');
                }
                self.out.push('>');
                self.inlines(inlines);
                let _ = writeln!(self.out, "</h{level}>");
            }
            LayoutNode::BlockQuote { callout, children, .. } => {
                match callout {
                    Some(kind) => {
                        let class = kind.title().to_lowercase();
                        let _ = writeln!(self.out, "<blockquote class=\"markdown-alert-{class}\">");
                    }
                    None => self.out.push_str("<blockquote>\n"),
                }
                for c in children {
                    self.node(c, false);
                }
                self.out.push_str("</blockquote>\n");
            }
            LayoutNode::List { start, tight, items, .. } => {
                let close = match start {
                    None => {
                        self.out.push_str("<ul>\n");
                        "</ul>\n"
                    }
                    Some(1) => {
                        self.out.push_str("<ol>\n");
                        "</ol>\n"
                    }
                    Some(n) => {
                        let _ = writeln!(self.out, "<ol start=\"{n}\">");
                        "</ol>\n"
                    }
                };
                for item in items {
                    self.out.push_str("<li>");
                    let checkbox = item.task.map(|checked| {
                        if checked {
                            "<input disabled=\"\" type=\"checkbox\" checked=\"\"/>\n"
                        } else {
                            "<input disabled=\"\" type=\"checkbox\"/>\n"
                        }
                    });
                    let mut checkbox = checkbox;
                    for (i, c) in item.children.iter().enumerate() {
                        match (checkbox.take(), c) {
                            // In loose items the marker sits inside the first paragraph.
                            (Some(cb), LayoutNode::Paragraph { inlines, .. }) if i == 0 && !tight => {
                                self.out.push_str("<p>");
                                self.out.push_str(cb);
                                self.inlines(inlines);
                                self.out.push_str("</p>\n");
                                continue;
                            }
                            (Some(cb), _) => self.out.push_str(cb),
                            (None, _) => {}
                        }
                        self.node(c, *tight);
                    }
                    if let Some(cb) = checkbox {
                        self.out.push_str(cb);
                    }
                    self.out.push_str("</li>\n");
                }
                self.out.push_str(close);
            }
            LayoutNode::CodeBlock { info, code, .. } => {
                let lang = info.split(' ').next().unwrap_or("");
                if lang.is_empty() {
                    self.out.push_str("<pre><code>");
                } else {
                    self.out.push_str("<pre><code class=\"language-");
                    let _ = escape_html(&mut self.out, lang);
                    self.out.push_str("\">");
                }
                let _ = escape_html_body_text(&mut self.out, code);
                self.out.push_str("</code></pre>\n");
            }
            LayoutNode::Table { align, header, rows, .. } => {
                self.out.push_str("<table><thead><tr>");
                for (i, cell) in header.cells.iter().enumerate() {
                    self.cell("th", align.get(i), &cell.inlines);
                }
                self.out.push_str("</tr></thead><tbody>\n");
                for row in rows {
                    self.out.push_str("<tr>");
                    for (i, cell) in row.cells.iter().enumerate() {
                        self.cell("td", align.get(i), &cell.inlines);
                    }
                    self.out.push_str("</tr>\n");
                }
                self.out.push_str("</tbody></table>\n");
            }
            LayoutNode::ThematicBreak { .. } => self.out.push_str("<hr />\n"),
            LayoutNode::FootnoteDefinition { label, children, .. } => {
                self.out.push_str("<div class=\"footnote-definition\" id=\"");
                let _ = escape_html(&mut self.out, label);
                let n = self.footnote_number(label);
                let _ = write!(self.out, "\"><sup class=\"footnote-definition-label\">{n}</sup>");
                for c in children {
                    self.node(c, false);
                }
                self.out.push_str("</div>\n");
            }
            LayoutNode::Html { html, .. } => self.out.push_str(html),
            LayoutNode::Custom { renderer, payload, .. } => {
                let _ = write!(self.out, "<div class=\"{renderer}\">");
                let _ = escape_html_body_text(&mut self.out, payload);
                self.out.push_str("</div>\n");
            }
            LayoutNode::Group { children, .. } => {
                for c in children {
                    self.node(c, tight);
                }
            }
        }
    }

    fn cell(&mut self, tag: &str, align: Option<&Align>, inlines: &[Inline]) {
        let style = match align {
            Some(Align::Left) => " style=\"text-align: left\"",
            Some(Align::Center) => " style=\"text-align: center\"",
            Some(Align::Right) => " style=\"text-align: right\"",
            _ => "",
        };
        let _ = write!(self.out, "<{tag}{style}>");
        self.inlines(inlines);
        let _ = write!(self.out, "</{tag}>");
    }

    fn inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            match inline {
                Inline::Text { text, style, .. } => {
                    for (on, tag) in TAGS {
                        if on(style) {
                            let _ = write!(self.out, "<{tag}>");
                        }
                    }
                    let _ = escape_html_body_text(&mut self.out, text);
                    for (on, tag) in TAGS.iter().rev() {
                        if on(style) {
                            let _ = write!(self.out, "</{tag}>");
                        }
                    }
                }
                Inline::Link { url, title, children, .. } => {
                    self.out.push_str("<a href=\"");
                    if let Some(addr) = url.strip_prefix("mailto:") {
                        self.out.push_str("mailto:");
                        let _ = escape_href(&mut self.out, addr);
                    } else {
                        let _ = escape_href(&mut self.out, url);
                    }
                    if !title.is_empty() {
                        self.out.push_str("\" title=\"");
                        let _ = escape_html(&mut self.out, title);
                    }
                    self.out.push_str("\">");
                    self.inlines(children);
                    self.out.push_str("</a>");
                }
                Inline::Image { src, alt, title, .. } => {
                    self.out.push_str("<img src=\"");
                    let _ = escape_href(&mut self.out, src);
                    self.out.push_str("\" alt=\"");
                    let _ = escape_html(&mut self.out, alt);
                    if !title.is_empty() {
                        self.out.push_str("\" title=\"");
                        let _ = escape_html(&mut self.out, title);
                    }
                    self.out.push_str("\" />");
                }
                Inline::FootnoteRef { label, .. } => {
                    self.out.push_str("<sup class=\"footnote-reference\"><a href=\"#");
                    let _ = escape_html(&mut self.out, label);
                    let n = self.footnote_number(label);
                    let _ = write!(self.out, "\">{n}</a></sup>");
                }
                Inline::SoftBreak { .. } => self.out.push('\n'),
                Inline::HardBreak { .. } => self.out.push_str("<br />\n"),
                Inline::Math { tex, display, .. } => {
                    let class = if *display { "math math-display" } else { "math math-inline" };
                    let _ = write!(self.out, "<span class=\"{class}\">");
                    let _ = escape_html(&mut self.out, tex);
                    self.out.push_str("</span>");
                }
                Inline::Html { html, .. } => self.out.push_str(html),
            }
        }
    }
}
