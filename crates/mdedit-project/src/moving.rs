//! Rename or move a markdown file inside the app (SPEC 6.4): its referenced
//! assets and its sidecar move with it, and links are rewritten, both in the
//! moved file and in other project files that link to it.
//!
//! The app saves the file before calling this and reloads it (and every file
//! in [`MoveReport::updated_files`]) afterwards.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use mdedit_sidecar::sidecar_path;

use crate::ProjectError;
use crate::assets::{create_unique, find_identical, hash_file};
use crate::fsutil::{self, absolute, normalize, relative};
use crate::links::{LinkDest, encode_dest, find_link_dests, local_target};

/// Markdown files bigger than this are not scanned for links to the moved
/// file (they open as large files anyway).
const SCAN_LIMIT: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveReport {
    /// Where the file ended up (a directory `to` gets the file name appended).
    pub to: PathBuf,
    /// Assets that moved with the file, `(old, new)`. When an identical file
    /// already existed at the destination it is reused and the old one removed.
    pub moved_assets: Vec<(PathBuf, PathBuf)>,
    /// Assets other files still link to: copied (or an identical file at the
    /// destination reused); the originals stay.
    pub copied_assets: Vec<(PathBuf, PathBuf)>,
    pub sidecar_moved: bool,
    /// Link destinations rewritten in the moved file.
    pub links_rewritten: usize,
    /// Other markdown files whose links to the moved file were rewritten.
    pub updated_files: Vec<PathBuf>,
}

struct Resolved {
    dest: LinkDest,
    /// Decoded path as written (relative to the file's folder).
    path: String,
    suffix: String,
    target: PathBuf,
}

fn resolve_all(text: &str, dir: &Path) -> Vec<Resolved> {
    find_link_dests(text)
        .into_iter()
        .filter_map(|dest| {
            let (path, suffix) = local_target(&text[dest.range.clone()])?;
            let target = normalize(&dir.join(&path));
            Some(Resolved { dest, path, suffix, target })
        })
        .collect()
}

fn apply_edits(text: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, new) in edits {
        out.push_str(&text[at..range.start]);
        out.push_str(&new);
        at = range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// The link from a file in `dir` to `target`, or `None` if the link as
/// written (`path`) already points there.
fn new_link(dir: &Path, r: &Resolved, target: &Path) -> Option<String> {
    if normalize(&dir.join(&r.path)) == target {
        return None;
    }
    let rel = relative(dir, target).unwrap_or_else(|| target.to_string_lossy().replace('\\', "/"));
    Some(format!("{}{}", encode_dest(&rel, r.dest.angle), r.suffix))
}

/// Markdown files under `root`, skipping hidden folders (`.git`, `.mdedit`)
/// and symlinked folders.
fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    stack.push(path);
                }
            } else if fsutil::is_markdown(&path) && entry.metadata().is_ok_and(|m| m.len() <= SCAN_LIMIT) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Put asset `src` into `dest_dir`: reuse an identical file there, else take
/// its name (or `-2`, `-3`...). Moves unless `shared`, then copies.
fn place_asset(src: &Path, dest_dir: &Path, shared: bool) -> Result<PathBuf, ProjectError> {
    std::fs::create_dir_all(dest_dir)?;
    let size = std::fs::metadata(src)?.len();
    if let Some(existing) = find_identical(dest_dir, size, &hash_file(src)?)? {
        if !shared {
            std::fs::remove_file(src)?;
        }
        return Ok(dest_dir.join(existing));
    }
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Reserve the name with an empty file, then fill or replace it.
    let (path, mut file) = create_unique(dest_dir, &name)?;
    let result = if shared {
        std::fs::File::open(src).and_then(|mut f| std::io::copy(&mut f, &mut file)).map(|_| ())
    } else {
        drop(file);
        fsutil::move_file(src, &path)
    };
    if let Err(e) = result {
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    Ok(path)
}

/// Rename or move the markdown file `from` to `to` (a file path, or an
/// existing directory to move into) within the project at `root`.
///
/// - Links in the moved file are rewritten so they still point at the same
///   files; links to the file itself follow it.
/// - Assets it links to inside `<old dir>/<assets_dir>/` move to
///   `<new dir>/<assets_dir>/` (same sub-path), keeping their names, with
///   `-2`, `-3` on collision and SHA-256 reuse of identical files. An asset
///   another project file also links to is copied instead of moved. Nothing
///   moves when the folder stays the same. Folders emptied by the move are
///   removed.
/// - The sidecar `.name.md.toml` is renamed along.
/// - Links to the file from other markdown files in the project are rewritten.
///
/// Fails without changing anything if `to` or its sidecar already exists.
pub fn move_markdown(root: &Path, from: &Path, to: &Path, assets_dir: &str) -> Result<MoveReport, ProjectError> {
    let root = absolute(root)?;
    let from = absolute(from)?;
    let mut to = absolute(to)?;
    if to.is_dir() {
        to = to.join(from.file_name().ok_or_else(|| ProjectError::InvalidPath(from.display().to_string()))?);
    }
    if !from.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, from.display().to_string()).into());
    }
    let mut report = MoveReport { to: to.clone(), ..MoveReport::default() };
    if from == to {
        return Ok(report);
    }
    if to.exists() && !fsutil::same_file(&from, &to) {
        return Err(ProjectError::AlreadyExists(to));
    }
    let (from_sc, to_sc) = (sidecar_path(&from), sidecar_path(&to));
    if to_sc.exists() && !fsutil::same_file(&from_sc, &to_sc) {
        return Err(ProjectError::AlreadyExists(to_sc));
    }
    let check =
        |d: &str| d.is_empty() || Path::new(d).components().any(|c| !matches!(c, std::path::Component::Normal(_)));
    if check(assets_dir) {
        return Err(ProjectError::InvalidPath(format!("assets_dir must be a relative folder: {assets_dir:?}")));
    }

    let old_dir = from.parent().map(Path::to_path_buf).unwrap_or_default();
    let new_dir = to.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&new_dir)?;

    let text = String::from_utf8(std::fs::read(&from)?).ok();
    let others: Vec<(PathBuf, String, Vec<Resolved>)> = markdown_files(&root)
        .into_iter()
        .filter(|p| *p != from)
        .filter_map(|p| {
            let t = std::fs::read_to_string(&p).ok()?;
            let res = resolve_all(&t, p.parent()?);
            Some((p, t, res))
        })
        .collect();

    // Plan and carry out asset moves, collect link edits for the moved file.
    let mut edits = Vec::new();
    if let Some(text) = &text {
        let old_assets = old_dir.join(assets_dir);
        let new_assets = new_dir.join(assets_dir);
        let mut placed: HashMap<PathBuf, PathBuf> = HashMap::new();
        for r in resolve_all(text, &old_dir) {
            let new_target = if r.target == from {
                to.clone()
            } else if old_dir != new_dir && r.target.starts_with(&old_assets) && r.target.is_file() {
                match placed.get(&r.target) {
                    Some(p) => p.clone(),
                    None => {
                        let shared = others.iter().any(|(_, _, res)| res.iter().any(|x| x.target == r.target));
                        let sub = r.target.strip_prefix(&old_assets).unwrap_or(&r.target);
                        let dest_dir = new_assets.join(sub.parent().unwrap_or(Path::new("")));
                        let new_path = place_asset(&r.target, &dest_dir, shared)?;
                        let pair = (r.target.clone(), new_path.clone());
                        if shared {
                            report.copied_assets.push(pair)
                        } else {
                            report.moved_assets.push(pair)
                        }
                        placed.insert(r.target.clone(), new_path.clone());
                        new_path
                    }
                }
            } else {
                r.target.clone()
            };
            if let Some(link) = new_link(&new_dir, &r, &new_target) {
                edits.push((r.dest.range.clone(), link));
            }
        }
        // Remove folders the moves emptied, up to the assets folder itself.
        for (old, _) in &report.moved_assets {
            for dir in old.ancestors().skip(1).take_while(|d| d.starts_with(&old_assets)) {
                if std::fs::remove_dir(dir).is_err() {
                    break; // not empty
                }
            }
        }
    }

    fsutil::move_file(&from, &to)?;
    if let Some(text) = &text
        && !edits.is_empty()
    {
        report.links_rewritten = edits.len();
        fsutil::write_atomic(&to, apply_edits(text, edits).as_bytes())?;
    }
    if from_sc.is_file() {
        fsutil::move_file(&from_sc, &to_sc)?;
        report.sidecar_moved = true;
    }

    for (path, text, res) in &others {
        let dir = path.parent().unwrap_or(Path::new(""));
        let edits: Vec<_> = res
            .iter()
            .filter(|r| r.target == from)
            .filter_map(|r| new_link(dir, r, &to).map(|l| (r.dest.range.clone(), l)))
            .collect();
        if !edits.is_empty() {
            fsutil::write_atomic(path, apply_edits(text, edits).as_bytes())?;
            report.updated_files.push(path.clone());
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().to_path_buf();
            crate::Project::init(&root, "t").unwrap();
            Self { _tmp: tmp, root }
        }
        fn p(&self, rel: &str) -> PathBuf {
            self.root.join(rel)
        }
        fn write(&self, rel: &str, content: &str) -> PathBuf {
            let p = self.p(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
            p
        }
        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.p(rel)).unwrap()
        }
        fn mv(&self, from: &str, to: &str) -> Result<MoveReport, ProjectError> {
            move_markdown(&self.root, &self.p(from), &self.p(to), "assets")
        }
    }

    #[test]
    fn rename_in_place_moves_sidecar_and_keeps_asset_links() {
        let f = Fixture::new();
        f.write("intro.md", "![d](assets/d.png)\n\nSee [top](intro.md#top) and [o](other.md).\n");
        f.write(".intro.md.toml", "version = 1\n");
        f.write("assets/d.png", "D");
        f.write("other.md", "");

        let r = f.mv("intro.md", "start.md").unwrap();
        assert_eq!(r.to, f.p("start.md"));
        assert!(!f.p("intro.md").exists() && !f.p(".intro.md.toml").exists());
        assert_eq!(f.read(".start.md.toml"), "version = 1\n");
        assert!(r.sidecar_moved);
        assert!(r.moved_assets.is_empty() && r.copied_assets.is_empty());
        assert!(f.p("assets/d.png").exists());
        assert_eq!(f.read("start.md"), "![d](assets/d.png)\n\nSee [top](start.md#top) and [o](other.md).\n");
        assert_eq!(r.links_rewritten, 1);
    }

    #[test]
    fn move_to_subfolder_moves_assets_and_rewrites_links() {
        let f = Fixture::new();
        f.write(
            "intro.md",
            "![d](assets/d.png) ![s](assets/sub/s.png) [o](other.md) [w](https://x.y/a.png) [h](#h)\n\n\
             ![p](../outside.png) [ref]\n\n[ref]: <other.md> \"t\"\n",
        );
        f.write("assets/d.png", "D");
        f.write("assets/sub/s.png", "S");
        f.write("other.md", "");
        f.write(".intro.md.toml", "version = 1\n");

        let r = f.mv("intro.md", "chapter-1/intro.md").unwrap();
        assert_eq!(
            f.read("chapter-1/intro.md"),
            "![d](assets/d.png) ![s](assets/sub/s.png) [o](../other.md) [w](https://x.y/a.png) [h](#h)\n\n\
             ![p](../../outside.png) [ref]\n\n[ref]: <../other.md> \"t\"\n"
        );
        assert_eq!(r.links_rewritten, 3);
        assert_eq!(f.read("chapter-1/assets/d.png"), "D");
        assert_eq!(f.read("chapter-1/assets/sub/s.png"), "S");
        assert!(!f.p("assets/d.png").exists());
        assert_eq!(r.moved_assets.len(), 2);
        assert!(f.p("chapter-1/.intro.md.toml").exists());
        assert!(!f.p("assets").exists(), "emptied folders are removed");
    }

    #[test]
    fn assets_folder_with_other_files_stays() {
        let f = Fixture::new();
        f.write("a.md", "![x](assets/x.png)");
        f.write("assets/x.png", "X");
        f.write("assets/unrelated.png", "U");
        f.mv("a.md", "sub/a.md").unwrap();
        assert_eq!(f.read("assets/unrelated.png"), "U");
        assert!(!f.p("sub/assets/unrelated.png").exists(), "only referenced assets move");
        assert_eq!(f.read("sub/assets/x.png"), "X");
    }

    #[test]
    fn shared_assets_are_copied_not_moved() {
        let f = Fixture::new();
        f.write("a.md", "![x](assets/x.png) ![y](assets/y.png)");
        f.write("b.md", "uses ![x](assets/x.png) too");
        f.write("assets/x.png", "X");
        f.write("assets/y.png", "Y");

        let r = f.mv("a.md", "ch/a.md").unwrap();
        assert_eq!(r.copied_assets, [(f.p("assets/x.png"), f.p("ch/assets/x.png"))]);
        assert_eq!(r.moved_assets, [(f.p("assets/y.png"), f.p("ch/assets/y.png"))]);
        assert_eq!(f.read("assets/x.png"), "X");
        assert_eq!(f.read("ch/assets/x.png"), "X");
        assert_eq!(f.read("b.md"), "uses ![x](assets/x.png) too");
        assert!(r.updated_files.is_empty());
    }

    #[test]
    fn asset_collisions_at_destination_get_suffix_or_are_reused() {
        let f = Fixture::new();
        f.write("a.md", "![x](assets/x.png) ![y](assets/y.png)\n");
        f.write("assets/x.png", "new X");
        f.write("assets/y.png", "Y");
        f.write("ch/assets/x.png", "old X, different");
        f.write("ch/assets/same-as-y.png", "Y");

        let r = f.mv("a.md", "ch/a.md").unwrap();
        assert_eq!(f.read("ch/a.md"), "![x](assets/x-2.png) ![y](assets/same-as-y.png)\n");
        assert_eq!(f.read("ch/assets/x-2.png"), "new X");
        assert_eq!(f.read("ch/assets/x.png"), "old X, different");
        assert!(!f.p("ch/assets/y.png").exists(), "identical file reused, not duplicated");
        assert!(!f.p("assets/y.png").exists());
        assert_eq!(r.moved_assets.len(), 2);
    }

    #[test]
    fn inbound_links_from_other_files_are_rewritten() {
        let f = Fixture::new();
        f.write("notes/a.md", "# A\n");
        f.write("index.md", "[A](notes/a.md#a) and [B](<notes/a.md>) and [C](notes/other.md)\n");
        f.write("notes/sib.md", "[a](a.md)\n");
        f.write(".mdedit/ignored.md", "[a](../notes/a.md)\n");

        let r = f.mv("notes/a.md", "archive/old a.md").unwrap();
        assert_eq!(
            f.read("index.md"),
            "[A](archive/old%20a.md#a) and [B](<archive/old a.md>) and [C](notes/other.md)\n"
        );
        assert_eq!(f.read("notes/sib.md"), "[a](../archive/old%20a.md)\n");
        assert_eq!(f.read(".mdedit/ignored.md"), "[a](../notes/a.md)\n");
        let mut updated = r.updated_files.clone();
        updated.sort();
        assert_eq!(updated, [f.p("index.md"), f.p("notes/sib.md")]);
    }

    #[test]
    fn percent_encoded_and_escaped_links_resolve() {
        let f = Fixture::new();
        f.write("a.md", "![x](assets/my%20pic.png) [o](other\\(1\\).md)\n");
        f.write("assets/my pic.png", "P");
        f.write("other(1).md", "");
        f.mv("a.md", "d/a.md").unwrap();
        assert_eq!(f.read("d/a.md"), "![x](assets/my%20pic.png) [o](../other%281%29.md)\n");
        assert_eq!(f.read("d/assets/my pic.png"), "P");
    }

    #[test]
    fn existing_target_or_sidecar_is_refused_untouched() {
        let f = Fixture::new();
        f.write("a.md", "![x](assets/x.png)");
        f.write("assets/x.png", "X");
        f.write("sub/a.md", "taken");
        assert!(matches!(f.mv("a.md", "sub/a.md"), Err(ProjectError::AlreadyExists(_))));
        assert!(matches!(f.mv("a.md", "sub"), Err(ProjectError::AlreadyExists(_))));
        f.write("other/.b.md.toml", "orphan");
        assert!(matches!(f.mv("a.md", "other/b.md"), Err(ProjectError::AlreadyExists(_))));
        assert_eq!(f.read("a.md"), "![x](assets/x.png)");
        assert!(f.p("assets/x.png").exists());
        assert!(matches!(f.mv("missing.md", "z.md"), Err(ProjectError::Io(_))));
    }

    #[test]
    fn move_into_directory_keeps_name() {
        let f = Fixture::new();
        f.write("a.md", "x");
        std::fs::create_dir_all(f.p("dir")).unwrap();
        let r = f.mv("a.md", "dir").unwrap();
        assert_eq!(r.to, f.p("dir/a.md"));
        assert_eq!(f.read("dir/a.md"), "x");
    }

    #[test]
    fn non_utf8_file_still_moves_with_sidecar() {
        let f = Fixture::new();
        std::fs::write(f.p("bin.md"), [0xff, 0xfe, b'x']).unwrap();
        f.write(".bin.md.toml", "version = 1\n");
        let r = f.mv("bin.md", "sub/bin.md").unwrap();
        assert_eq!(std::fs::read(f.p("sub/bin.md")).unwrap(), [0xff, 0xfe, b'x']);
        assert!(r.sidecar_moved && f.p("sub/.bin.md.toml").exists());
    }

    #[test]
    fn project_method_uses_configured_assets_dir() {
        let f = Fixture::new();
        let mut p = crate::Project::open(&f.root).unwrap();
        p.config.files.assets_dir = "img".into();
        f.write("a.md", "![x](img/x.png)");
        f.write("img/x.png", "X");
        p.move_markdown(&f.p("a.md"), &f.p("b/a.md")).unwrap();
        assert_eq!(f.read("b/img/x.png"), "X");
    }
}
