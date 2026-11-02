//! Mermaid blocks in PDF and DOCX exports.

use std::path::Path;

use mdedit_export::{ExportFormat, ExportOptions, Template, docx_parts, export, typst_source};

const DOC: &str = "# Flow\n\nBefore.\n\n```mermaid\nflowchart LR\n  A[Write] --> B[Export]\n```\n\n\
```mermaid\nflowchart LR\n  A -->\n```\n\nAfter.\n";

fn options() -> ExportOptions {
    ExportOptions { date: Some("2026-01-02".into()), ..Default::default() }
}

#[test]
fn typst_calls_md_diagram_for_blocks_that_render() {
    let files = typst_source(DOC, &Template::default(), &options()).unwrap();
    assert!(files.main.starts_with("#let md-diagram("), "fallback first: {}", &files.main[..80]);
    assert!(files.main.contains("#md-diagram(\"/__diagram/0.svg\", alt: \"Mermaid diagram\")"), "{}", files.main);
    // Without rendering, typst_source can't know the second block is
    // broken; documents without diagrams are unchanged.
    let plain = typst_source("# T\n\nText.\n", &Template::default(), &options()).unwrap();
    assert!(plain.main.starts_with("#import"));
}

#[test]
fn pdf_embeds_the_diagram_and_keeps_broken_ones_as_code() {
    let pdf = export(DOC, Path::new("."), &Template::default(), &options(), ExportFormat::Pdf).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let no_diagram = DOC.replace("```mermaid\nflowchart LR\n  A[Write] --> B[Export]\n```\n\n", "");
    let smaller = export(&no_diagram, Path::new("."), &Template::default(), &options(), ExportFormat::Pdf).unwrap();
    assert!(pdf.len() > smaller.len() + 1000, "{} vs {}", pdf.len(), smaller.len());
}

#[test]
fn custom_templates_without_md_diagram_still_compile() {
    let mut template = Template::default();
    let builtin = template.typst().to_string();
    let start = builtin.find("#let md-diagram(").unwrap();
    let end = start + builtin[start..].find("\n)\n").unwrap() + 3;
    template.typst_source = Some(format!("{}{}", &builtin[..start], &builtin[end..]));
    let pdf = export(DOC, Path::new("."), &template, &options(), ExportFormat::Pdf).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn docx_has_one_picture_per_rendered_diagram() {
    let parts = docx_parts(DOC, Path::new("."), &Template::default(), &options()).unwrap();
    assert_eq!(parts.document.matches("<pic:pic").count(), 1, "{}", parts.document);
    assert!(parts.document.contains("A --&gt;"), "broken block kept as code");
    let docx = export(DOC, Path::new("."), &Template::default(), &options(), ExportFormat::Docx).unwrap();
    assert!(docx.starts_with(b"PK"));
}
