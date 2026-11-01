//! Asset import (SPEC 6.4): copy into `assets/` next to the markdown file,
//! sanitised name with `-2`, `-3` on collision, `pasted-YYYYMMDD-HHMMSS.png`
//! for pasted images, reuse of identical files by SHA-256.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::ProjectError;
use crate::fsutil;

pub enum AssetSource<'a> {
    File(&'a Path),
    /// Pasted image bytes (PNG).
    Pasted(&'a [u8]),
}

/// Import an asset for `md_file`; returns the relative link to insert
/// (`assets/name.png`, always `/`-separated, needs no escaping).
pub fn import(md_file: &Path, assets_dir: &str, source: AssetSource<'_>) -> Result<PathBuf, ProjectError> {
    import_at(md_file, assets_dir, source, jiff::Zoned::now().datetime())
}

/// [`import`] with the clock given (names pasted images).
pub fn import_at(
    md_file: &Path,
    assets_dir: &str,
    source: AssetSource<'_>,
    now: jiff::civil::DateTime,
) -> Result<PathBuf, ProjectError> {
    check_assets_dir(assets_dir)?;
    let md_dir = fsutil::absolute(md_file)?.parent().map(Path::to_path_buf).unwrap_or_default();
    let dir = md_dir.join(assets_dir);
    let link = |name: &str| PathBuf::from(format!("{}/{name}", assets_dir.trim_end_matches(['/', '\\'])));

    let (name, size, hash) = match &source {
        AssetSource::File(path) => {
            let abs = fsutil::absolute(path)?;
            // Already in this assets folder: link it as it is.
            if abs.parent() == Some(dir.as_path())
                && let Some(n) = abs.file_name()
            {
                return Ok(link(&n.to_string_lossy()));
            }
            let name = sanitize_file_name(&path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
            (name, std::fs::metadata(path)?.len(), hash_file(path)?)
        }
        AssetSource::Pasted(bytes) => (pasted_name(now), bytes.len() as u64, Sha256::digest(bytes).into()),
    };

    std::fs::create_dir_all(&dir)?;
    if let Some(existing) = find_identical(&dir, size, &hash)? {
        return Ok(link(&existing));
    }
    let (path, mut file) = create_unique(&dir, &name)?;
    let written = match &source {
        AssetSource::File(src) => File::open(src).and_then(|mut f| std::io::copy(&mut f, &mut file)).map(|_| ()),
        AssetSource::Pasted(bytes) => file.write_all(bytes),
    }
    .and_then(|()| file.sync_all());
    if let Err(e) = written {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    Ok(link(&path.file_name().unwrap_or_default().to_string_lossy()))
}

fn check_assets_dir(assets_dir: &str) -> Result<(), ProjectError> {
    let p = Path::new(assets_dir);
    if assets_dir.is_empty() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(ProjectError::InvalidPath(format!("assets_dir must be a relative folder: {assets_dir:?}")));
    }
    Ok(())
}

/// `pasted-YYYYMMDD-HHMMSS.png`.
pub fn pasted_name(now: jiff::civil::DateTime) -> String {
    now.strftime("pasted-%Y%m%d-%H%M%S.png").to_string()
}

/// Sanitise a file name for `assets/`: keep letters, digits, `_`, `-` and
/// `.`; everything else (spaces, brackets, `#`, path separators) becomes `-`,
/// runs of `-` collapse, leading/trailing `-` and `.` go (no hidden files, no
/// trailing dot on Windows), the extension is lowercased, Windows device
/// names get a `_` suffix and the stem is cut to 100 characters.
pub fn sanitize_file_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or("");
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    };
    let clean = |s: &str, keep_dot: bool| {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            let c = if c.is_alphanumeric() || c == '_' || c == '-' || (keep_dot && c == '.') { c } else { '-' };
            if !(c == '-' && out.ends_with('-')) {
                out.push(c);
            }
        }
        out.trim_matches(['-', '.']).to_owned()
    };
    let mut stem = clean(stem, true);
    let ext = clean(ext, false).to_lowercase();
    if stem.is_empty() {
        stem = "file".into();
    }
    let upper = stem.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        stem.push('_');
    }
    if let Some((i, _)) = stem.char_indices().nth(100) {
        stem.truncate(i);
    }
    if ext.is_empty() { stem } else { format!("{stem}.{ext}") }
}

/// `name`, `name-2`, `name-3`, ... with the extension kept.
fn candidate(name: &str, n: usize) -> String {
    if n == 1 {
        return name.to_owned();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}-{n}{}", &name[..i], &name[i..]),
        _ => format!("{name}-{n}"),
    }
}

/// Create a new file `dir/name` (or the first free `-N` variant) without ever
/// replacing an existing one.
pub(crate) fn create_unique(dir: &Path, name: &str) -> std::io::Result<(PathBuf, File)> {
    for n in 1.. {
        let path = dir.join(candidate(name, n));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

pub(crate) fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// Name of a file directly in `dir` with this size and SHA-256, if any.
/// Only files of the same size are hashed.
pub(crate) fn find_identical(dir: &Path, size: u64, hash: &[u8; 32]) -> std::io::Result<Option<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut same_size: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() && meta.len() == size && !fsutil::is_temp_file(&entry.path()) {
            same_size.push(entry.path());
        }
    }
    same_size.sort();
    for path in same_size {
        if hash_file(&path)? == *hash {
            return Ok(path.file_name().map(|n| n.to_string_lossy().into_owned()));
        }
    }
    Ok(None)
}

/// Outside project mode images are linked where they are, not copied
/// (SPEC 6.4): relative to the markdown file's folder when possible,
/// otherwise absolute (unsaved document, other drive). Returns an encoded
/// link destination ready to put in `![](...)`.
pub fn link_in_place(md_file: Option<&Path>, image: &Path) -> std::io::Result<String> {
    let image = fsutil::absolute(image)?;
    let rel = match md_file {
        Some(md) => fsutil::absolute(md)?.parent().and_then(|dir| fsutil::relative(dir, &image)),
        None => None,
    };
    let path = rel.unwrap_or_else(|| image.to_string_lossy().replace('\\', "/"));
    Ok(crate::links::encode_dest(&path, false))
}

/// `![](link)` for an imported asset.
pub fn image_markdown(link: &Path) -> String {
    format!("![]({})", link.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> jiff::civil::DateTime {
        s.parse().unwrap()
    }

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path().join("notes/intro.md");
        std::fs::create_dir_all(md.parent().unwrap()).unwrap();
        std::fs::write(&md, "# Intro\n").unwrap();
        (tmp, md)
    }

    fn write(path: &Path, bytes: &[u8]) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        path.to_path_buf()
    }

    #[test]
    fn copies_into_assets_next_to_the_markdown_file() {
        let (tmp, md) = setup();
        let src = write(&tmp.path().join("Downloads/diagram.png"), b"png-1");
        let link = import(&md, "assets", AssetSource::File(&src)).unwrap();
        assert_eq!(link, Path::new("assets/diagram.png"));
        assert_eq!(std::fs::read(tmp.path().join("notes/assets/diagram.png")).unwrap(), b"png-1");
        assert!(src.exists(), "the original stays");
        assert_eq!(image_markdown(&link), "![](assets/diagram.png)");
    }

    #[test]
    fn sanitises_names() {
        assert_eq!(sanitize_file_name("My Photo (1).PNG"), "My-Photo-1.png");
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("C:\\Users\\n\\scan #3.jpeg"), "scan-3.jpeg");
        assert_eq!(sanitize_file_name(".hidden.png"), "hidden.png");
        assert_eq!(sanitize_file_name("caffè latte.jpg"), "caffè-latte.jpg");
        assert_eq!(sanitize_file_name("con.png"), "con_.png");
        assert_eq!(sanitize_file_name("???.gif"), "file.gif");
        assert_eq!(sanitize_file_name("trailing. .png"), "trailing.png");
        assert_eq!(sanitize_file_name("noext"), "noext");
        assert_eq!(sanitize_file_name(&format!("{}.png", "a".repeat(300))), format!("{}.png", "a".repeat(100)));

        let (tmp, md) = setup();
        let src = write(&tmp.path().join("in/Screen Shot [final].PNG"), b"x");
        let link = import(&md, "assets", AssetSource::File(&src)).unwrap();
        assert_eq!(link, Path::new("assets/Screen-Shot-final.png"));
    }

    #[test]
    fn collisions_get_numbered_suffixes() {
        let (tmp, md) = setup();
        let a = write(&tmp.path().join("a/pic.png"), b"one");
        let b = write(&tmp.path().join("b/pic.png"), b"two");
        let c = write(&tmp.path().join("c/pic.png"), b"three");
        assert_eq!(import(&md, "assets", AssetSource::File(&a)).unwrap(), Path::new("assets/pic.png"));
        assert_eq!(import(&md, "assets", AssetSource::File(&b)).unwrap(), Path::new("assets/pic-2.png"));
        assert_eq!(import(&md, "assets", AssetSource::File(&c)).unwrap(), Path::new("assets/pic-3.png"));
        assert_eq!(std::fs::read(tmp.path().join("notes/assets/pic-3.png")).unwrap(), b"three");
        let n = write(&tmp.path().join("d/README"), b"r1");
        let m = write(&tmp.path().join("e/README"), b"r2");
        import(&md, "assets", AssetSource::File(&n)).unwrap();
        assert_eq!(import(&md, "assets", AssetSource::File(&m)).unwrap(), Path::new("assets/README-2"));
    }

    #[test]
    fn pasted_images_are_named_by_time() {
        let (tmp, md) = setup();
        let now = dt("2026-11-01T15:30:07");
        assert_eq!(pasted_name(now), "pasted-20261101-153007.png");
        let l1 = import_at(&md, "assets", AssetSource::Pasted(b"img-a"), now).unwrap();
        assert_eq!(l1, Path::new("assets/pasted-20261101-153007.png"));
        // A second, different paste in the same second.
        let l2 = import_at(&md, "assets", AssetSource::Pasted(b"img-b"), now).unwrap();
        assert_eq!(l2, Path::new("assets/pasted-20261101-153007-2.png"));
        assert_eq!(std::fs::read(tmp.path().join("notes").join(&l2)).unwrap(), b"img-b");
        // The live clock gives the same shape.
        let live = import(&md, "assets", AssetSource::Pasted(b"img-c")).unwrap();
        let name = live.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("pasted-") && name.ends_with(".png") && name.len() == 26, "{name}");
    }

    #[test]
    fn identical_files_are_reused() {
        let (tmp, md) = setup();
        let a = write(&tmp.path().join("a/first.png"), b"same bytes");
        let b = write(&tmp.path().join("b/second-name.png"), b"same bytes");
        let first = import(&md, "assets", AssetSource::File(&a)).unwrap();
        assert_eq!(import(&md, "assets", AssetSource::File(&b)).unwrap(), first);
        assert_eq!(
            import_at(&md, "assets", AssetSource::Pasted(b"same bytes"), dt("2026-01-01T00:00")).unwrap(),
            first
        );
        // Same size but different content is not reused.
        let c = write(&tmp.path().join("c/first.png"), b"diff bytes");
        assert_eq!(import(&md, "assets", AssetSource::File(&c)).unwrap(), Path::new("assets/first-2.png"));
        assert_eq!(std::fs::read_dir(tmp.path().join("notes/assets")).unwrap().count(), 2);
    }

    #[test]
    fn file_already_in_assets_is_linked_not_copied() {
        let (tmp, md) = setup();
        let inside = write(&tmp.path().join("notes/assets/x.png"), b"x");
        assert_eq!(import(&md, "assets", AssetSource::File(&inside)).unwrap(), Path::new("assets/x.png"));
        assert_eq!(std::fs::read_dir(tmp.path().join("notes/assets")).unwrap().count(), 1);
    }

    #[test]
    fn custom_and_invalid_assets_dirs() {
        let (tmp, md) = setup();
        let src = write(&tmp.path().join("a/p.png"), b"p");
        assert_eq!(import(&md, "media/img", AssetSource::File(&src)).unwrap(), Path::new("media/img/p.png"));
        assert!(tmp.path().join("notes/media/img/p.png").is_file());
        for bad in ["", "../out", "/abs", "a/../b"] {
            assert!(matches!(import(&md, bad, AssetSource::File(&src)), Err(ProjectError::InvalidPath(_))), "{bad}");
        }
    }

    #[test]
    fn missing_source_is_an_error_and_leaves_nothing() {
        let (tmp, md) = setup();
        let r = import(&md, "assets", AssetSource::File(&tmp.path().join("nope.png")));
        assert!(matches!(r, Err(ProjectError::Io(_))));
        assert!(!tmp.path().join("notes/assets/nope.png").exists());
    }

    #[test]
    fn outside_project_mode_links_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let md = tmp.path().join("docs/a.md");
        let img = tmp.path().join("pics/my shot.png");
        assert_eq!(link_in_place(Some(&md), &img).unwrap(), "../pics/my%20shot.png");
        assert_eq!(link_in_place(Some(&tmp.path().join("a.md")), &img).unwrap(), "pics/my%20shot.png");
        let abs = link_in_place(None, &img).unwrap();
        assert!(abs.starts_with('/') || abs.chars().nth(1) == Some(':'), "{abs}");
        assert!(abs.ends_with("pics/my%20shot.png"));
        assert!(!img.exists(), "nothing is copied or created");
    }
}
