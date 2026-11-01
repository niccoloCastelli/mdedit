//! Every CommonMark 0.31.2 spec example, laid out block by block through
//! the block index, renders to the same HTML as pulldown-cmark renders the
//! whole document; and every source range points at the text it claims.

use mdedit_core::parse::{BlockIndex, markdown_options};
use mdedit_core::{RopeBuffer, TextBuffer};
use mdedit_render::{Inline, LayoutContext, LayoutNode, RefTable, html, layout_block_with, layout_document};

const EXAMPLES: &str = include_str!("data/commonmark-0.31.2-examples.txt");

fn examples() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in EXAMPLES.split_inclusive('\n') {
        if let Some(name) = line.strip_prefix("@@ ") {
            out.push((name.trim().to_string(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
        }
    }
    out
}

fn pulldown_html(md: &str) -> String {
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, pulldown_cmark::Parser::new_ext(md, markdown_options()));
    out
}

/// Makes two HTML strings comparable despite the model keeping inline
/// styles as flags: style tags are re-emitted in a fixed order per text
/// run, `<br>` spellings unified, and newlines next to tags dropped.
fn normalize(html: &str) -> String {
    const STYLES: [&str; 7] = ["strong", "em", "del", "sup", "sub", "kbd", "code"];
    const BLOCKS: [&str; 14] =
        ["p", "li", "h1", "h2", "h3", "h4", "h5", "h6", "td", "th", "blockquote", "ul", "ol", "div"];
    let html = html.replace(">\n", ">").replace("\n<", "<");
    let mut out = String::new();
    let mut active = [0u32; 7];
    let mut emitted = [false; 7];
    let mut rest = html.as_str();
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut String, active: &[u32; 7], emitted: &mut [bool; 7]| {
        if text.is_empty() {
            return;
        }
        let want: Vec<bool> = active.iter().map(|&n| n > 0).collect();
        if want != emitted.as_slice() {
            for i in (0..7).rev() {
                if emitted[i] {
                    out.push_str(&format!("</{}>", STYLES[i]));
                }
            }
            for i in 0..7 {
                if want[i] {
                    out.push_str(&format!("<{}>", STYLES[i]));
                }
                emitted[i] = want[i];
            }
        }
        out.push_str(text);
        text.clear();
    };
    let close_all = |out: &mut String, emitted: &mut [bool; 7]| {
        for i in (0..7).rev() {
            if emitted[i] {
                out.push_str(&format!("</{}>", STYLES[i]));
                emitted[i] = false;
            }
        }
    };
    while !rest.is_empty() {
        let (tag, next) = match rest.find('<') {
            Some(0) => match rest.find('>') {
                Some(end) => (Some(&rest[..=end]), &rest[end + 1..]),
                None => (None, ""),
            },
            Some(i) => {
                text.push_str(&rest[..i]);
                rest = &rest[i..];
                continue;
            }
            None => (None, ""),
        };
        let Some(tag) = tag else {
            text.push_str(rest);
            rest = next;
            continue;
        };
        rest = next;
        let lower = tag.to_ascii_lowercase();
        let name = lower.trim_start_matches("</").trim_start_matches('<').trim_end_matches('>');
        if let Some(i) = STYLES.iter().position(|s| *s == name) {
            if lower.starts_with("</") {
                if active[i] > 0 {
                    flush(&mut text, &mut out, &active, &mut emitted);
                    active[i] -= 1;
                    continue;
                }
            } else {
                flush(&mut text, &mut out, &active, &mut emitted);
                active[i] += 1;
                continue;
            }
        }
        flush(&mut text, &mut out, &active, &mut emitted);
        let bare = name.split([' ', '/']).next().unwrap_or("");
        if BLOCKS.contains(&bare) {
            close_all(&mut out, &mut emitted);
            active = [0; 7];
        }
        if matches!(lower.as_str(), "<br>" | "<br/>" | "<br />") {
            close_all(&mut out, &mut emitted);
            out.push_str("<br />");
        } else {
            close_all(&mut out, &mut emitted);
            out.push_str(tag);
        }
    }
    flush(&mut text, &mut out, &active, &mut emitted);
    close_all(&mut out, &mut emitted);
    out
}

fn layout_blocks(buf: &RopeBuffer, refs: &RefTable) -> Vec<LayoutNode> {
    let index = BlockIndex::parse(buf);
    let ctx = LayoutContext { links: Some(refs), renderers: &[] };
    (0..).map_while(|i| index.get(i)).map(|b| layout_block_with(&b, buf, &ctx).node).collect()
}

/// Every range lies inside the document, and text that claims to map byte
/// for byte equals its source.
fn check_sources(node: &LayoutNode, buf: &RopeBuffer, errors: &mut Vec<String>) {
    let src = node.source();
    if src.end > buf.len_bytes() || src.start > src.end {
        errors.push(format!("node range {src:?} out of bounds"));
        return;
    }
    let inlines = |inlines: &[Inline], errors: &mut Vec<String>| check_inlines(inlines, buf, errors);
    match node {
        LayoutNode::Paragraph { inlines: i, .. } | LayoutNode::Heading { inlines: i, .. } => inlines(i, errors),
        LayoutNode::BlockQuote { children, .. }
        | LayoutNode::FootnoteDefinition { children, .. }
        | LayoutNode::Group { children, .. } => children.iter().for_each(|c| check_sources(c, buf, errors)),
        LayoutNode::List { items, .. } => {
            for item in items {
                if let Some(r) = &item.task_source {
                    let marker = buf.slice(r.clone());
                    if !matches!(&*marker, "[ ]" | "[x]" | "[X]") {
                        errors.push(format!("task marker {marker:?}"));
                    }
                }
                item.children.iter().for_each(|c| check_sources(c, buf, errors));
            }
        }
        LayoutNode::CodeBlock { code, segments, .. } | LayoutNode::Html { html: code, segments, .. } => {
            for s in segments {
                if s.text.len() == s.source.len() && buf.slice(s.source.clone()) != code[s.text.clone()] {
                    errors.push(format!("segment {:?} != {:?}", &code[s.text.clone()], buf.slice(s.source.clone())));
                }
            }
        }
        LayoutNode::Table { header, rows, .. } => {
            for cell in header.cells.iter().chain(rows.iter().flat_map(|r| &r.cells)) {
                inlines(&cell.inlines, errors);
            }
        }
        LayoutNode::ThematicBreak { .. } | LayoutNode::Custom { .. } => {}
    }
}

fn check_inlines(inlines: &[Inline], buf: &RopeBuffer, errors: &mut Vec<String>) {
    for inline in inlines {
        let src = inline.source();
        if src.end > buf.len_bytes() || src.start > src.end {
            errors.push(format!("inline range {src:?} out of bounds"));
            continue;
        }
        match inline {
            Inline::Text { text, source, .. } if text.len() == source.len() => {
                let s = buf.slice(source.clone());
                if s != *text {
                    errors.push(format!("text {text:?} has source {s:?}"));
                }
            }
            Inline::Link { children, .. } => check_inlines(children, buf, errors),
            _ => {}
        }
    }
}

#[test]
fn normalizer_unifies_style_nesting() {
    assert_eq!(
        normalize("<p><em><strong>a</strong></em></p>\n"),
        normalize("<p><strong><em>a</em></strong></p>")
    );
    assert_eq!(normalize("<p>a<br>\nb</p>"), normalize("<p>a<br />\nb</p>"));
}

#[test]
fn spec_examples_render_like_pulldown() {
    let examples = examples();
    assert_eq!(examples.len(), 652);
    let mut failures = Vec::new();
    for (name, md) in &examples {
        let expected = normalize(&pulldown_html(md));
        let buf = RopeBuffer::from_text(md);
        let refs = RefTable::scan(md);
        let nodes = layout_blocks(&buf, &refs);
        let by_block = normalize(&html::to_html(&nodes));
        let whole = normalize(&html::to_html(&layout_document(md)));
        if by_block != expected || whole != expected {
            failures.push(format!(
                "{name}\n  markdown: {md:?}\n  expected: {expected:?}\n  by block: {by_block:?}\n  whole:    {whole:?}"
            ));
        }
        let mut errors = Vec::new();
        nodes.iter().for_each(|n| check_sources(n, &buf, &mut errors));
        if !errors.is_empty() {
            failures.push(format!("{name} sources\n  markdown: {md:?}\n  {}", errors.join("\n  ")));
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}
