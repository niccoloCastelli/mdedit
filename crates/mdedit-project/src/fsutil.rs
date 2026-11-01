//! Small filesystem helpers shared by the modules.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Write `bytes` to a temp file next to `path`, fsync, rename over `path`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.mdedit-tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// True for the temp files of [`write_atomic`] (the watcher ignores them).
pub(crate) fn is_temp_file(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().contains(".mdedit-tmp-"))
}

/// Lexically resolve `.` and `..` (no symlink resolution, no disk access).
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Absolute, normalised form of `path`.
pub(crate) fn absolute(path: &Path) -> std::io::Result<PathBuf> {
    Ok(normalize(&std::path::absolute(path)?))
}

/// Relative path from directory `from_dir` to `target`, both absolute and
/// normalised, joined with `/`. `None` when they share no root (another
/// drive on Windows).
pub(crate) fn relative(from_dir: &Path, target: &Path) -> Option<String> {
    let a: Vec<Component> = from_dir.components().collect();
    let b: Vec<Component> = target.components().collect();
    if a.first() != b.first() {
        return None;
    }
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), a.len() - common).collect();
    parts.extend(b[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    Some(if parts.is_empty() { ".".to_owned() } else { parts.join("/") })
}

/// Move a file, falling back to copy + remove across filesystems.
pub(crate) fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// True if `a` and `b` name the same existing file (hard links, or a case-only
/// rename on a case-insensitive filesystem).
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase(),
            _ => false,
        }
    }
}

/// Markdown extensions recognised for link scanning and the file tree.
pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown" | "mkd" | "mkdn"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        let r = |a: &str, b: &str| relative(Path::new(a), Path::new(b)).unwrap();
        assert_eq!(r("/p", "/p/assets/x.png"), "assets/x.png");
        assert_eq!(r("/p/ch1", "/p/assets/x.png"), "../assets/x.png");
        assert_eq!(r("/p/a/b", "/p/c/d.md"), "../../c/d.md");
        assert_eq!(r("/p", "/p"), ".");
    }

    #[test]
    fn normalize_dots() {
        assert_eq!(normalize(Path::new("/p/a/../b/./c.md")), Path::new("/p/b/c.md"));
    }
}
