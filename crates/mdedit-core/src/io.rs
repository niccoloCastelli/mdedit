//! Loading and saving files.
//!
//! SPEC 3: UTF-8 with or without BOM (preserved), LF or CRLF (detected and
//! preserved), other encodings open read-only; saves are atomic (temp file in
//! the same directory, fsync, rename). The buffer keeps the file's line
//! endings as they are, so an unedited document saves byte-identically;
//! [`FileFormat::line_ending`] is what new lines should use.
//!
//! Files above the large-file threshold will open as a `MappedBuffer` once
//! the large-files workstream lands it; until then they load into a rope.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::buffer::TextBuffer;
use crate::document::Document;

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    /// What Enter inserts.
    pub const fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }

    /// The ending of the first line break in `text` (LF when there is none).
    pub fn detect(text: &str) -> Self {
        match text.find('\n') {
            Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => LineEnding::CrLf,
            _ => LineEnding::Lf,
        }
    }
}

/// What must be preserved on save so an unedited file round-trips
/// byte-identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileFormat {
    pub bom: bool,
    pub line_ending: LineEnding,
    /// Not valid UTF-8: opened read-only with a banner (the text shown has
    /// replacement characters, so saving it would corrupt the file).
    pub read_only: bool,
}

/// Default large-file threshold (SPEC 8), overridable in settings/project.
pub const DEFAULT_LARGE_FILE_THRESHOLD: u64 = 20 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("the file is not valid UTF-8 and was opened read-only")]
    ReadOnly,
    #[error("the document has no file name yet")]
    NoPath,
}

/// Split raw file bytes into format and text.
pub fn decode(bytes: Vec<u8>) -> (FileFormat, String) {
    let bom = bytes.starts_with(BOM);
    let body = if bom { bytes[BOM.len()..].to_vec() } else { bytes };
    let (text, read_only) = match String::from_utf8(body) {
        Ok(text) => (text, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    let format = FileFormat { bom, line_ending: LineEnding::detect(&text), read_only };
    (format, text)
}

/// Open a file as a document.
pub fn load(path: &Path, large_file_threshold: u64) -> Result<Document, IoError> {
    load_inner(path, large_file_threshold, false)
}

/// [`load`] with the initial parse left as a job for a worker
/// (`Document::take_parse_job`), so the first frame comes sooner.
pub fn load_deferred(path: &Path, large_file_threshold: u64) -> Result<Document, IoError> {
    load_inner(path, large_file_threshold, true)
}

fn load_inner(path: &Path, large_file_threshold: u64, defer_parse: bool) -> Result<Document, IoError> {
    // Large files will get a MappedBuffer here (large-files workstream).
    let _ = large_file_threshold;
    let (format, text) = decode(fs::read(path)?);
    let mut doc = if defer_parse { Document::from_text_deferred(&text) } else { Document::from_text(&text) };
    doc.format = format;
    doc.path = Some(path.to_path_buf());
    Ok(doc)
}

/// Save the document to `path` (save as) or its own path, atomically, and
/// mark it saved. Blocking; for a background save, take
/// `doc.buffer().snapshot()` and `doc.begin_save()`, call [`write_atomic`]
/// on a worker, then `doc.mark_saved_at(point)`.
pub fn save(doc: &mut Document, path: Option<&Path>) -> Result<(), IoError> {
    if doc.format.read_only {
        return Err(IoError::ReadOnly);
    }
    let target = path.map(Path::to_path_buf).or_else(|| doc.path.clone()).ok_or(IoError::NoPath)?;
    write_atomic(&target, doc.buffer(), doc.format)?;
    doc.path = Some(target);
    doc.mark_saved();
    Ok(())
}

/// Write `buf` to `path` through a temp file in the same directory: BOM if
/// the format has one, then the text chunk by chunk, fsync, rename over the
/// target, fsync the directory. A symlinked target is followed, so the link
/// stays a link. The target's permissions are kept.
pub fn write_atomic(path: &Path, buf: &dyn TextBuffer, format: FileFormat) -> Result<(), IoError> {
    let target = match fs::canonicalize(path) {
        Ok(real) => real,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(e) => return Err(e.into()),
    };
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.mdedit-{}.tmp", std::process::id()));

    let result = (|| -> std::io::Result<()> {
        let file = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
        if let Ok(meta) = fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        let mut out = BufWriter::with_capacity(256 * 1024, file);
        if format.bom {
            out.write_all(BOM)?;
        }
        for chunk in buf.chunks(0..buf.len_bytes()) {
            out.write_all(chunk)?;
        }
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &target)?;
        sync_dir(&dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Make the rename durable. Best effort: not every platform or file system
/// supports syncing a directory.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Edit, EditOrigin, TxnId};

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn round_trip(bytes: &[u8]) -> Vec<u8> {
        let dir = tmpdir();
        let path = dir.path().join("doc.md");
        fs::write(&path, bytes).unwrap();
        let mut doc = load(&path, DEFAULT_LARGE_FILE_THRESHOLD).unwrap();
        save(&mut doc, None).unwrap();
        fs::read(&path).unwrap()
    }

    #[test]
    fn unedited_files_round_trip_byte_identically() {
        let cases: &[&[u8]] = &[
            b"",
            b"# Title\n\nBody\n",
            b"no trailing newline",
            b"# CRLF\r\n\r\nBody\r\n",
            b"mixed\r\nendings\nhere\r",
            b"\xEF\xBB\xBF# BOM\n",
            b"\xEF\xBB\xBF",
            "unicode: 3–5 giorni, 日本語, 🦀\n".as_bytes(),
        ];
        for &case in cases {
            assert_eq!(round_trip(case), case, "case {:?}", String::from_utf8_lossy(case));
        }
    }

    #[test]
    fn detects_format() {
        let (f, text) = decode(b"\xEF\xBB\xBFa\r\nb\n".to_vec());
        assert_eq!(f, FileFormat { bom: true, line_ending: LineEnding::CrLf, read_only: false });
        assert_eq!(text, "a\r\nb\n");
        assert_eq!(LineEnding::detect("a\nb\r\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("\n"), LineEnding::Lf);
    }

    #[test]
    fn invalid_utf8_is_read_only_and_never_saved() {
        let dir = tmpdir();
        let path = dir.path().join("latin1.md");
        let original = b"caf\xE9\n".to_vec();
        fs::write(&path, &original).unwrap();
        let mut doc = load(&path, DEFAULT_LARGE_FILE_THRESHOLD).unwrap();
        assert!(doc.format.read_only);
        assert!(matches!(save(&mut doc, None), Err(IoError::ReadOnly)));
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn save_writes_edits_marks_clean_and_leaves_no_temp_file() {
        let dir = tmpdir();
        let path = dir.path().join("doc.md");
        fs::write(&path, "\u{feff}a\r\n").unwrap();
        let mut doc = load(&path, DEFAULT_LARGE_FILE_THRESHOLD).unwrap();
        let nl = doc.format.line_ending.as_str();
        doc.apply(&Edit::insert(3, format!("b{nl}"), EditOrigin::User, TxnId(0)));
        assert!(doc.is_dirty());
        save(&mut doc, None).unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(fs::read(&path).unwrap(), b"\xEF\xBB\xBFa\r\nb\r\n");
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["doc.md"]);
    }

    #[test]
    fn save_as_and_no_path() {
        let dir = tmpdir();
        let mut doc = Document::from_text("x");
        assert!(matches!(save(&mut doc, None), Err(IoError::NoPath)));
        let path = dir.path().join("new.md");
        save(&mut doc, Some(&path)).unwrap();
        assert_eq!(doc.path.as_deref(), Some(path.as_path()));
        assert_eq!(fs::read(&path).unwrap(), b"x");
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_symlink_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir();
        let real = dir.path().join("real.md");
        let link = dir.path().join("link.md");
        fs::write(&real, "old").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut doc = load(&link, DEFAULT_LARGE_FILE_THRESHOLD).unwrap();
        doc.replace_all("new", EditOrigin::User);
        save(&mut doc, None).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o640);
    }
}
