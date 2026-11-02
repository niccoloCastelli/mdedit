//! Page headers and footers with logos and styling (template `[header]` /
//! `[footer]` tables).

use std::io::Read;
use std::path::Path;

use mdedit_export::{ExportFormat, ExportOptions, Template, export};

const DOC: &str = "# Branded\n\nBody text.\n";

fn branded(logo: &str) -> (tempfile::TempDir, Template) {
    let dir = tempfile::tempdir().unwrap();
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons");
    let name = Path::new(logo).file_name().unwrap().to_str().unwrap();
    std::fs::copy(assets.join(logo), dir.path().join(name)).unwrap();
    let toml = format!(
        "name = \"Branded\"\n\
         [header]\ntext = \"{{title}} | | {{date}}\"\nlogo = \"{name}\"\nlogo_height_mm = 9\nrule = true\n\
         [footer]\ntext = \"| {{page}} / {{pages}} |\"\ncolor = \"#D9542B\"\nsize_pt = 9\nbold = true\n"
    );
    std::fs::write(dir.path().join("template.toml"), toml).unwrap();
    let template = Template::load_dir(dir.path()).unwrap();
    (dir, template)
}

fn zip_entry(bytes: &[u8], name: &str) -> Option<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut out = String::new();
    zip.by_name(name).ok()?.read_to_string(&mut out).ok()?;
    Some(out)
}

#[test]
fn logos_in_pdf_and_docx() {
    for logo in ["mdedit.svg", "png/mdedit-64.png"] {
        let (dir, template) = branded(logo);
        let options = ExportOptions { date: Some("2026-11-02".into()), ..Default::default() };
        let pdf = export(DOC, dir.path(), &template, &options, ExportFormat::Pdf).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        let docx = export(DOC, dir.path(), &template, &options, ExportFormat::Docx).unwrap();
        // `RUNNING_OUT=dir` keeps the files for a visual check.
        if let Some(out) = std::env::var_os("RUNNING_OUT") {
            let stem = Path::new(logo).file_stem().unwrap().to_string_lossy().into_owned();
            std::fs::write(Path::new(&out).join(format!("{stem}.pdf")), &pdf).unwrap();
            std::fs::write(Path::new(&out).join(format!("{stem}.docx")), &docx).unwrap();
        }
        let rels = zip_entry(&docx, "word/_rels/header1.xml.rels").unwrap_or_default();
        assert!(rels.contains("relationships/image"), "{logo}: header has no image: {rels}");
        let styles = zip_entry(&docx, "word/styles.xml").unwrap();
        assert!(styles.contains("w:styleId=\"PageFooter\""));
        assert!(styles.contains("D9542B"));
    }
}

#[test]
fn missing_logo_is_skipped() {
    let (dir, mut template) = branded("mdedit.svg");
    template.header.logo = "nope.png".into();
    let options = ExportOptions::default();
    assert!(export(DOC, dir.path(), &template, &options, ExportFormat::Pdf).is_ok());
    assert!(export(DOC, dir.path(), &template, &options, ExportFormat::Docx).is_ok());
}

#[test]
fn preview_renders_pages() {
    let (dir, template) = branded("mdedit.svg");
    let long = format!("# Long\n\n{}", "A paragraph of text that goes on.\n\n".repeat(200));
    let p = mdedit_export::preview(&long, dir.path(), &template, &ExportOptions::default(), 2, 0.5).unwrap();
    assert!(p.page_count > 2, "{}", p.page_count);
    assert_eq!(p.pages.len(), 2);
    let page = &p.pages[0];
    // A4 is 595 × 842 pt.
    assert_eq!((page.width, page.height), (298, 421));
    assert_eq!(page.rgba.len(), (page.width * page.height * 4) as usize);
    // White paper with some ink on it.
    assert_eq!(&page.rgba[..4], &[255, 255, 255, 255]);
    assert!(page.rgba.chunks(4).any(|px| px[0] < 128));
}
