//! Export (SPEC 10; ARCHITECTURE 8).
//!
//! Input is the markdown source (parsed here with
//! `mdedit_core::parse::markdown_options()`, a full parse is fine for
//! export) plus a resolved [`Template`]. Both outputs go through one
//! document tree ([`ast`]) so they agree on structure. PDF: markdown →
//! Typst markup → `typst` compiled through a minimal `World` (bundled
//! fonts, project files for images) → `typst-pdf`. DOCX: `docx-rs` with
//! named styles from the template TOML. Blocking; the app runs it on a
//! worker.

pub mod ast;
mod diagram;
mod docx;
pub mod highlight;
mod pdf;
pub mod template;
mod typst_markup;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use ast::ExportComment;
pub use docx::DocxParts;
pub use template::Template;
pub use typst_markup::TypstFiles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Pdf,
    Docx,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Pdf => "pdf",
            ExportFormat::Docx => "docx",
        }
    }
}

/// Per-export overrides from the export dialog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportOptions {
    pub page_size: Option<String>,
    pub toc: Option<bool>,
    pub heading_numbers: Option<bool>,
    pub include_comments: bool,
    /// Margins in mm: top, right, bottom, left.
    pub margins_mm: Option<[f32; 4]>,
    /// `{title}` when the document has no title heading (the app passes
    /// the file name without extension).
    pub title: Option<String>,
    /// `{date}`; defaults to today as `YYYY-MM-DD`.
    pub date: Option<String>,
    /// Comments to include, as footnotes, when `include_comments` is set.
    pub comments: Vec<ExportComment>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("template: {0}")]
    Template(String),
    #[error("typst: {0}")]
    Typst(String),
    #[error("docx: {0}")]
    Docx(String),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

/// The template with the dialog's overrides applied, plus values the
/// writers need.
pub(crate) struct Settings {
    pub template: Template,
    pub include_comments: bool,
    /// Fallback for `{title}`.
    pub title: String,
    pub date: String,
}

impl Settings {
    fn new(template: &Template, options: &ExportOptions) -> Result<Self, ExportError> {
        let mut template = template.clone();
        if let Some(size) = &options.page_size {
            if template::page_size_mm(size).is_none() {
                return Err(ExportError::Template(format!("unknown page size {size:?}")));
            }
            template.page_size = size.clone();
        }
        if let Some(toc) = options.toc {
            template.toc = toc;
        }
        if let Some(numbers) = options.heading_numbers {
            template.heading_numbers = numbers;
        }
        if let Some(margins) = options.margins_mm {
            template.margins_mm = margins;
        }
        let date = options.date.clone().unwrap_or_else(|| jiff::Zoned::now().date().to_string());
        Ok(Settings {
            template,
            include_comments: options.include_comments,
            title: options.title.clone().unwrap_or_default(),
            date,
        })
    }

    /// Parse the document. Hand-numbered headings (`## 1. Goals`) keep
    /// their own numbers and turn automatic numbering off.
    fn doc(&mut self, markdown: &str, options: &ExportOptions) -> ast::Doc {
        let comments = if self.include_comments { options.comments.as_slice() } else { &[] };
        let doc = ast::Doc::parse(markdown, comments);
        if doc.has_manual_numbers() {
            self.template.heading_numbers = false;
        }
        doc
    }
}

/// Render `markdown` to bytes. `base_dir` resolves relative image paths.
pub fn export(
    markdown: &str,
    base_dir: &Path,
    template: &Template,
    options: &ExportOptions,
    format: ExportFormat,
) -> Result<Vec<u8>, ExportError> {
    match format {
        ExportFormat::Pdf => pdf_world(markdown, base_dir, template, options)?.compile_pdf(),
        ExportFormat::Docx => {
            let mut settings = Settings::new(template, options)?;
            let doc = settings.doc(markdown, options);
            docx::pack(&doc, &settings, &mut |url| read_image(base_dir, url))
        }
    }
}

/// One rendered page of a [`preview`]: premultiplied RGBA, opaque.
#[derive(Debug, Clone)]
pub struct PreviewPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Preview {
    /// The first `max_pages` pages.
    pub pages: Vec<PreviewPage>,
    /// Pages in the whole document.
    pub page_count: usize,
}

/// The PDF layout rendered to images, for the export dialog's preview.
/// `pixel_per_pt` sets the resolution (A4 is 595 pt wide). DOCX has no
/// layout engine of its own here; the PDF pages are a close stand-in.
pub fn preview(
    markdown: &str,
    base_dir: &Path,
    template: &Template,
    options: &ExportOptions,
    max_pages: usize,
    pixel_per_pt: f32,
) -> Result<Preview, ExportError> {
    let document = pdf_world(markdown, base_dir, template, options)?.compile()?;
    let render = typst_render::RenderOptions {
        pixel_per_pt: typst::utils::Scalar::from(f64::from(pixel_per_pt)),
        ..Default::default()
    };
    let pages = document
        .pages()
        .iter()
        .take(max_pages)
        .map(|page| {
            let pixmap = typst_render::render(page, &render);
            PreviewPage { width: pixmap.width(), height: pixmap.height(), rgba: pixmap.take() }
        })
        .collect();
    Ok(Preview { pages, page_count: document.pages().len() })
}

/// Settings, document tree, generated Typst and images, ready to compile.
fn pdf_world(
    markdown: &str,
    base_dir: &Path,
    template: &Template,
    options: &ExportOptions,
) -> Result<pdf::ExportWorld, ExportError> {
    let mut settings = Settings::new(template, options)?;
    let doc = settings.doc(markdown, options);
    let mut images = ImageResolver::new(base_dir);
    let mut diagrams = diagram::Diagrams::new(&settings.template.code_theme);
    let images = std::cell::RefCell::new(&mut images);
    let files = typst_markup::convert(&doc, &settings, &mut |url| images.borrow_mut().resolve(url), &mut |src| {
        let d = diagrams.get(src)?;
        let path = images.borrow_mut().add_bytes("svg", d.svg.clone());
        Some((path, Some(d.width * diagram::PT_PER_PX)))
    });
    let images = images.into_inner();
    let fonts = template_fonts(template);
    let today = jiff::civil::Date::strptime("%Y-%m-%d", &settings.date)
        .ok()
        .map(|d| (i32::from(d.year()), d.month() as u8, d.day() as u8));
    Ok(pdf::ExportWorld::new(&files, template.typst(), &images.files, &fonts, today))
}

/// [`export`] written to `out` through a temporary file and a rename, so
/// a failed export never leaves a truncated file behind.
pub fn export_to_file(
    markdown: &str,
    base_dir: &Path,
    template: &Template,
    options: &ExportOptions,
    format: ExportFormat,
    out: &Path,
) -> Result<(), ExportError> {
    let bytes = export(markdown, base_dir, template, options, format)?;
    let tmp = out.with_extension(format!("{}.part", format.extension()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}

/// The generated Typst sources (for golden tests, and for template authors
/// debugging a `template.typ`). Images are referenced as `/__img/<url>`
/// without checking that they exist, Mermaid diagrams as
/// `/__diagram/<n>.svg` without rendering them.
pub fn typst_source(markdown: &str, template: &Template, options: &ExportOptions) -> Result<TypstFiles, ExportError> {
    let mut settings = Settings::new(template, options)?;
    let doc = settings.doc(markdown, options);
    let mut n = 0;
    Ok(typst_markup::convert(&doc, &settings, &mut |url| Some(format!("/__img/{url}")), &mut |_| {
        n += 1;
        Some((format!("/__diagram/{}.svg", n - 1), None))
    }))
}

/// The main XML parts of the DOCX (for golden tests). `base_dir` resolves
/// images.
pub fn docx_parts(
    markdown: &str,
    base_dir: &Path,
    template: &Template,
    options: &ExportOptions,
) -> Result<DocxParts, ExportError> {
    let mut settings = Settings::new(template, options)?;
    let doc = settings.doc(markdown, options);
    Ok(docx::parts(&doc, &settings, &mut |url| read_image(base_dir, url)))
}

/// A local image path from a markdown URL, or `None` for remote URLs.
fn local_image_path(base_dir: &Path, url: &str) -> Option<PathBuf> {
    let url = url.trim();
    if url.is_empty() || url.contains("://") || url.starts_with("data:") {
        return None;
    }
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let decoded = percent_decode(url);
    let path = Path::new(&decoded);
    Some(if path.is_absolute() { path.to_path_buf() } else { base_dir.join(path) })
}

fn read_image(base_dir: &Path, url: &str) -> Option<Vec<u8>> {
    std::fs::read(local_image_path(base_dir, url)?).ok()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An image file in the Typst world.
pub(crate) enum ImageData {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// Maps image URLs to virtual paths in the Typst world.
struct ImageResolver<'a> {
    base_dir: &'a Path,
    by_url: HashMap<String, Option<String>>,
    files: Vec<(String, ImageData)>,
}

impl<'a> ImageResolver<'a> {
    fn new(base_dir: &'a Path) -> Self {
        ImageResolver { base_dir, by_url: HashMap::new(), files: Vec::new() }
    }

    fn resolve(&mut self, url: &str) -> Option<String> {
        if let Some(known) = self.by_url.get(url) {
            return known.clone();
        }
        let vpath = local_image_path(self.base_dir, url).filter(|p| p.is_file()).map(|path| {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let vpath = format!("/__img/{}.{ext}", self.files.len());
            self.files.push((vpath.clone(), ImageData::File(path)));
            vpath
        });
        self.by_url.insert(url.to_string(), vpath.clone());
        vpath
    }

    /// A generated image (a rendered diagram).
    fn add_bytes(&mut self, ext: &str, bytes: Vec<u8>) -> String {
        let vpath = format!("/__img/{}.{ext}", self.files.len());
        self.files.push((vpath.clone(), ImageData::Bytes(bytes)));
        vpath
    }
}

/// Font files in the template's `fonts/` directory.
fn template_fonts(template: &Template) -> Vec<PathBuf> {
    let Some(dir) = &template.dir else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir.join("fonts")) else { return Vec::new() };
    let mut fonts: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
        })
        .collect();
    fonts.sort();
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_paths() {
        let base = Path::new("/doc");
        assert_eq!(local_image_path(base, "img/a%20b.png?x=1"), Some(PathBuf::from("/doc/img/a b.png")));
        assert_eq!(local_image_path(base, "https://x/y.png"), None);
        assert_eq!(local_image_path(base, "/abs.png"), Some(PathBuf::from("/abs.png")));
    }

    #[test]
    fn options_override_the_template() {
        let options = ExportOptions {
            page_size: Some("us-letter".into()),
            toc: Some(true),
            margins_mm: Some([1.0, 2.0, 3.0, 4.0]),
            ..Default::default()
        };
        let s = Settings::new(&Template::default(), &options).unwrap();
        assert_eq!(s.template.page_size, "us-letter");
        assert!(s.template.toc);
        assert_eq!(s.template.margins_mm, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(s.date.len(), 10);
        let bad = ExportOptions { page_size: Some("napkin".into()), ..Default::default() };
        assert!(Settings::new(&Template::default(), &bad).is_err());
    }
}
