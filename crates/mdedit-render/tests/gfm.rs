//! GFM extensions, inline HTML styles, cross-block definitions and source
//! ranges on hand-written documents.

use mdedit_core::parse::BlockIndex;
use mdedit_core::{RopeBuffer, TextBuffer};
use mdedit_render::{
    Align, IndexRefs, Inline, LayoutContext, LayoutNode, RefTable, SpanStyle, html, layout_block, layout_block_with,
};

fn layout(md: &str) -> (RopeBuffer, Vec<LayoutNode>) {
    let buf = RopeBuffer::from_text(md);
    let index = BlockIndex::parse(&buf);
    let refs = IndexRefs::new(&index, &buf);
    let ctx = LayoutContext { links: Some(&refs), renderers: &[] };
    let nodes = index.blocks().iter().map(|b| layout_block_with(&b, &buf, &ctx).node).collect();
    (buf, nodes)
}

fn texts(inlines: &[Inline]) -> Vec<(String, SpanStyle)> {
    inlines
        .iter()
        .filter_map(|i| match i {
            Inline::Text { text, style, .. } => Some((text.clone(), *style)),
            _ => None,
        })
        .collect()
}

#[test]
fn table_cells_have_alignment_and_sources() {
    let md = "| Name | Qty |\n|:-----|----:|\n| *a*  | 1   |\n| b    | 22  |\n";
    let (buf, nodes) = layout(md);
    let LayoutNode::Table { align, header, rows, source } = &nodes[0] else { panic!("{nodes:?}") };
    assert_eq!(align, &[Align::Left, Align::Right]);
    assert_eq!(buf.slice(source.clone()), md.trim_end());
    assert_eq!(header.cells.len(), 2);
    assert_eq!(rows.len(), 2);
    assert_eq!(buf.slice(rows[1].source.clone()), "| b    | 22  |");
    let cell = &rows[0].cells[0];
    assert_eq!(texts(&cell.inlines), vec![("a".into(), SpanStyle { emphasis: true, ..Default::default() })]);
    assert!(buf.slice(cell.source.clone()).contains("*a*"));
    assert_eq!(buf.slice(rows[1].cells[1].source.clone()).trim(), "22");
}

#[test]
fn task_lists_strikethrough_and_tightness() {
    let (buf, nodes) = layout("- [ ] todo\n- [x] ~~done~~\n\n1. one\n\n2. two\n");
    let LayoutNode::List { start: None, tight: true, items, .. } = &nodes[0] else { panic!("{nodes:?}") };
    assert_eq!(items[0].task, Some(false));
    assert_eq!(items[1].task, Some(true));
    assert_eq!(buf.slice(items[1].task_source.clone().unwrap()), "[x]");
    let LayoutNode::Paragraph { inlines, .. } = &items[1].children[0] else { panic!() };
    assert_eq!(texts(inlines), vec![("done".into(), SpanStyle { strikethrough: true, ..Default::default() })]);
    let LayoutNode::List { start: Some(1), tight: false, items, .. } = &nodes[1] else { panic!("{nodes:?}") };
    assert_eq!(items.len(), 2);
}

#[test]
fn footnotes_and_link_refs_resolve_across_blocks() {
    let md = "See [the docs][d] and a note[^n].\n\n[d]: https://example.com \"Docs\"\n\n[^n]: The note.\n";
    let (_, nodes) = layout(md);
    let LayoutNode::Paragraph { inlines, .. } = &nodes[0] else { panic!() };
    assert!(matches!(&inlines[1], Inline::Link { url, title, .. } if url == "https://example.com" && title == "Docs"));
    assert!(inlines.iter().any(|i| matches!(i, Inline::FootnoteRef { label, .. } if label == "n")));
    assert!(matches!(&nodes[1], LayoutNode::FootnoteDefinition { label, .. } if label == "n"));
    let out = html::to_html(&nodes);
    assert!(out.contains("<sup class=\"footnote-reference\"><a href=\"#n\">1</a></sup>"), "{out}");

    // Without context the reference stays text.
    let buf = RopeBuffer::from_text(md);
    let index = BlockIndex::parse(&buf);
    let LayoutNode::Paragraph { inlines, .. } = layout_block(&index.get(0).unwrap(), &buf).node else { panic!() };
    assert!(!inlines.iter().any(|i| matches!(i, Inline::Link { .. } | Inline::FootnoteRef { .. })));
}

#[test]
fn inline_html_styles_and_breaks() {
    let (_, nodes) = layout("x<sup>2</sup> H<sub>2</sub>O <kbd>Ctrl</kbd><br>next <span>raw</span>\n");
    let LayoutNode::Paragraph { inlines, .. } = &nodes[0] else { panic!() };
    let t = texts(inlines);
    assert!(t.contains(&("2".into(), SpanStyle { superscript: true, ..Default::default() })));
    assert!(t.contains(&("Ctrl".into(), SpanStyle { kbd: true, ..Default::default() })));
    assert!(inlines.iter().any(|i| matches!(i, Inline::HardBreak { .. })));
    assert!(inlines.iter().any(|i| matches!(i, Inline::Html { html, .. } if html == "<span>")));
}

#[test]
fn heading_attributes_code_spans_and_mermaid() {
    let md = "## Title {#intro .big}\n\nUse `let x` now.\n\n```mermaid\ngraph TD\n```\n";
    let (buf, nodes) = layout(md);
    let LayoutNode::Heading { level: 2, id, classes, .. } = &nodes[0] else { panic!() };
    assert_eq!(id.as_deref(), Some("intro"));
    assert_eq!(classes, &["big"]);
    let LayoutNode::Paragraph { inlines, .. } = &nodes[1] else { panic!() };
    let Inline::Text { source, exact: true, .. } = &inlines[1] else { panic!("{inlines:?}") };
    assert_eq!(buf.slice(source.clone()), "let x");
    let LayoutNode::CodeBlock { lang, code, segments, .. } = &nodes[2] else { panic!() };
    assert_eq!(lang.as_deref(), Some("mermaid"));
    assert_eq!(code, "graph TD\n");
    assert_eq!(buf.slice(segments[0].source.clone()), "graph TD\n");
}

#[test]
fn indented_code_block_keeps_meaning_alone() {
    let (_, nodes) = layout("para\n\n      code\n");
    assert!(matches!(&nodes[1], LayoutNode::CodeBlock { code, .. } if code == "  code\n"), "{nodes:?}");
}

#[test]
fn ref_table_matches_index_refs() {
    let md = "[a]: /x\n\n[A]: /y\n\n[^f]: n\n";
    let buf = RopeBuffer::from_text(md);
    let index = BlockIndex::parse(&buf);
    let a = RefTable::scan(md);
    let b = IndexRefs::new(&index, &buf);
    use mdedit_render::LinkResolver;
    assert_eq!(a.link_ref("a"), b.link_ref("a"));
    assert_eq!(a.link_ref("a").unwrap().dest, "/x");
    assert!(a.has_footnote("f") && b.has_footnote("F"));
}
