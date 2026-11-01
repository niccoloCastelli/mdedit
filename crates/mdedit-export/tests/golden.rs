//! Golden snapshot tests: Typst markup and DOCX XML for each case in
//! `tests/golden/*.md` with each built-in template. Run with
//! `UPDATE_GOLDEN=1` to rewrite the snapshots after an intended change,
//! then review the diff.

use std::path::{Path, PathBuf};

use mdedit_export::{ExportComment, ExportFormat, ExportOptions, Template, docx_parts, export, typst_source};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// The sample document and its image live with the example binary.
fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/sample")
}

fn cases() -> Vec<(String, String)> {
    let mut cases = vec![("sample".to_string(), std::fs::read_to_string(sample_dir().join("sample.md")).unwrap())];
    let mut files: Vec<_> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    for path in files {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        cases.push((name, std::fs::read_to_string(&path).unwrap()));
    }
    cases
}

fn options(markdown: &str) -> ExportOptions {
    let offset = markdown.find("four steps").or_else(|| markdown.find("Lead paragraph")).unwrap_or(0);
    ExportOptions {
        include_comments: true,
        comments: vec![ExportComment { offset, author: "Ann".into(), body: "A review comment.".into() }],
        title: Some("fallback title".into()),
        date: Some("2026-01-02".into()),
        ..Default::default()
    }
}

/// One element per line, so snapshot diffs are readable.
fn pretty_xml(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() + xml.len() / 8);
    for (i, _) in xml.match_indices('<') {
        let rest = &xml[i..];
        let breaks = [
            "<w:p>",
            "<w:p ",
            "<w:tbl>",
            "<w:tr>",
            "<w:tc>",
            "<w:style ",
            "<w:footnote ",
            "<w:abstractNum ",
            "<w:num ",
            "<w:lvl ",
            "<w:sdt>",
            "<w:hyperlink ",
        ];
        if i > 0 && breaks.iter().any(|b| rest.starts_with(b)) {
            out.push('\n');
        }
        let end = xml[i + 1..].find('<').map_or(xml.len(), |j| i + 1 + j);
        out.push_str(&xml[i..end]);
    }
    out.push('\n');
    out
}

fn check(name: &str, actual: &str, failures: &mut Vec<String>) {
    let path = dir().join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    match std::fs::read_to_string(&path) {
        Ok(expected) if expected == actual => {}
        Ok(_) => {
            let new = path.with_extension(format!("{}.new", path.extension().unwrap().to_string_lossy()));
            std::fs::write(&new, actual).unwrap();
            failures.push(format!("{name} differs (actual written to {})", new.display()));
        }
        Err(_) => failures.push(format!("{name} missing; run with UPDATE_GOLDEN=1")),
    }
}

#[test]
fn golden_snapshots() {
    let mut failures = Vec::new();
    for (case, markdown) in cases() {
        for template in Template::builtins() {
            let t = template.name.to_lowercase();
            let opts = options(&markdown);
            let typ = typst_source(&markdown, &template, &opts).unwrap();
            check(
                &format!("{case}.{t}.typ"),
                &format!("{}\n// ---- main.typ\n{}\n", typ.conf, typ.main),
                &mut failures,
            );
            let parts = docx_parts(&markdown, &sample_dir(), &template, &opts).unwrap();
            let xml = format!(
                "<!-- document.xml -->\n{}<!-- footnotes.xml -->\n{}<!-- numbering.xml -->\n{}<!-- styles.xml -->\n{}",
                pretty_xml(&parts.document),
                pretty_xml(&parts.footnotes),
                pretty_xml(&parts.numbering),
                pretty_xml(&parts.styles),
            );
            check(&format!("{case}.{t}.docx.xml"), &xml, &mut failures);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every golden case compiles to a PDF and packs to a DOCX with every
/// built-in template (the markup above is valid Typst).
#[test]
fn golden_cases_export() {
    for (case, markdown) in cases() {
        for template in Template::builtins() {
            let opts = options(&markdown);
            let pdf = export(&markdown, &sample_dir(), &template, &opts, ExportFormat::Pdf)
                .unwrap_or_else(|e| panic!("{case} {}: {e}", template.name));
            assert!(pdf.starts_with(b"%PDF-"), "{case}");
            let docx = export(&markdown, &sample_dir(), &template, &opts, ExportFormat::Docx).unwrap();
            assert!(docx.starts_with(b"PK"), "{case}");
        }
    }
}

#[test]
fn template_typ_overrides_the_builtin() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mine");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("template.toml"), "style = \"report\"\n").unwrap();
    // Start from the built-in template and change one function.
    let custom = format!("{}\n#let md-rule() = [CUSTOM RULE]\n", mdedit_export::template::BUILTIN_TYPST);
    std::fs::write(dir.join("template.typ"), custom).unwrap();
    let template = Template::resolve("mine", &[tmp.path().to_path_buf()]).unwrap();
    let pdf = export("Text\n\n---\n", tmp.path(), &template, &ExportOptions::default(), ExportFormat::Pdf).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
}

#[test]
fn typst_errors_are_reported() {
    let template =
        Template { typst_source: Some("#let md-document(body) = undefined-thing".into()), ..Template::default() };
    let err = export("Hi", Path::new("."), &template, &ExportOptions::default(), ExportFormat::Pdf).unwrap_err();
    assert!(err.to_string().contains("undefined-thing") || err.to_string().contains("unknown variable"), "{err}");
}
