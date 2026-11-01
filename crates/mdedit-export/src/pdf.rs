//! PDF export: a minimal Typst `World` (bundled fonts, template fonts,
//! generated sources, project images) compiled with `typst` and written
//! with `typst-pdf` (ARCHITECTURE 8).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use typst::diag::{FileError, FileResult, Severity, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};
use typst_layout::PagedDocument;

use crate::ExportError;
use crate::typst_markup::{CONF_PATH, MAIN_PATH, TEMPLATE_PATH, TypstFiles};

fn bundled_fonts() -> &'static [Font] {
    static FONTS: OnceLock<Vec<Font>> = OnceLock::new();
    FONTS.get_or_init(|| {
        mdedit_theme::assets::fonts().flat_map(|bytes| Font::iter(Bytes::new(bytes))).collect()
    })
}

fn library() -> &'static LazyHash<Library> {
    static LIBRARY: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIBRARY.get_or_init(|| LazyHash::new(Library::default()))
}

fn file_id(path: &str) -> FileId {
    RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).expect("valid virtual path")).intern()
}

pub(crate) struct ExportWorld {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    sources: HashMap<FileId, Source>,
    /// Image virtual path → file on disk.
    images: HashMap<FileId, PathBuf>,
    main: FileId,
    today: Option<Datetime>,
}

impl ExportWorld {
    /// `images` maps the virtual paths the markup uses to real files.
    /// `extra_fonts` are font files (a template's `fonts/` directory).
    pub(crate) fn new(
        files: &TypstFiles,
        template_typ: &str,
        images: &[(String, PathBuf)],
        extra_fonts: &[PathBuf],
        today: Option<(i32, u8, u8)>,
    ) -> Self {
        let mut fonts = bundled_fonts().to_vec();
        for path in extra_fonts {
            if let Ok(data) = std::fs::read(path) {
                fonts.extend(Font::iter(Bytes::new(data)));
            }
        }
        let book = LazyHash::new(FontBook::from_fonts(&fonts));
        let mut sources = HashMap::new();
        for (path, text) in [(CONF_PATH, &files.conf), (TEMPLATE_PATH, &template_typ.to_string()), (MAIN_PATH, &files.main)]
        {
            let id = file_id(path);
            sources.insert(id, Source::new(id, text.clone()));
        }
        let images = images.iter().map(|(vpath, real)| (file_id(vpath), real.clone())).collect();
        let today = today.and_then(|(y, m, d)| Datetime::from_ymd(y, m, d));
        ExportWorld { book, fonts, sources, images, main: file_id(MAIN_PATH), today }
    }

    pub(crate) fn compile_pdf(&self) -> Result<Vec<u8>, ExportError> {
        let warned = typst::compile::<PagedDocument>(self);
        let document = warned.output.map_err(|errors| self.error(&errors))?;
        let options = typst_pdf::PdfOptions::default();
        typst_pdf::pdf(&document, &options).map_err(|errors| self.error(&errors))
    }

    fn error(&self, diagnostics: &[SourceDiagnostic]) -> ExportError {
        let messages: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| {
                let place = d.span.id().map(|id| format!("{:?}", id.vpath())).unwrap_or_default();
                let line = d
                    .span
                    .id()
                    .and_then(|id| self.sources.get(&id))
                    .and_then(|src| self.range(d.span).map(|r| src.lines().byte_to_line(r.start).unwrap_or(0) + 1));
                match line {
                    Some(line) => format!("{place}:{line}: {}", d.message),
                    None => d.message.to_string(),
                }
            })
            .collect();
        ExportError::Typst(messages.join("; "))
    }
}

impl World for ExportWorld {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.sources.get(&id).cloned().ok_or_else(|| not_found(id))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if let Some(source) = self.sources.get(&id) {
            return Ok(Bytes::from_string(source.clone()));
        }
        let path = self.images.get(&id).ok_or_else(|| not_found(id))?;
        std::fs::read(path).map(Bytes::new).map_err(|e| FileError::from_io(e, path))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<typst::foundations::Duration>) -> Option<Datetime> {
        self.today
    }
}

fn not_found(id: FileId) -> FileError {
    FileError::NotFound(PathBuf::from(id.vpath().get_with_slash()))
}
