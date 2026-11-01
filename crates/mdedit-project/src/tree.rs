//! Lazy project file tree for the sidebar (hidden sidecars never listed,
//! DESIGN_SYSTEM "Tree (files)"). A directory is read only when it is
//! expanded; watcher events invalidate just the affected listing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::fsutil::is_markdown;
use crate::watch::ProjectEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

impl TreeEntry {
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn is_markdown(&self) -> bool {
        !self.is_dir && is_markdown(&self.path)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListOptions {
    /// Show dotfiles. Sidecars, `.mdedit/` and `.git/` stay hidden anyway.
    pub show_hidden: bool,
}

/// True for a sidecar file name: `.name.md.toml` where `name.md` is a
/// markdown file name (SPEC 7, `mdedit_sidecar::sidecar_path`).
pub fn is_sidecar(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    name.strip_prefix('.').and_then(|n| n.strip_suffix(".toml")).is_some_and(|md| is_markdown(Path::new(md)))
}

fn always_hidden(name: &str, path: &Path) -> bool {
    name == crate::PROJECT_DIR || name == ".git" || is_sidecar(path)
}

/// Direct children of `dir`, directories first, sidecars, `.mdedit/` and
/// dotfiles hidden.
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<TreeEntry>> {
    list_dir_with(dir, ListOptions::default())
}

pub fn list_dir_with(dir: &Path, options: ListOptions) -> std::io::Result<Vec<TreeEntry>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if always_hidden(&name, &path) || (!options.show_hidden && name.starts_with('.')) {
            continue;
        }
        // Follows symlinks: a link to a directory is listed as a directory.
        let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
        out.push(TreeEntry { path, is_dir });
    }
    out.sort_by_cached_key(|e| (!e.is_dir, e.name().to_lowercase(), e.name()));
    Ok(out)
}

/// One visible row of the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeRow<'a> {
    pub depth: usize,
    pub entry: &'a TreeEntry,
    pub expanded: bool,
}

/// Lazily loaded tree: listings are cached per directory and only read for
/// the root and expanded directories.
#[derive(Debug, Clone)]
pub struct FileTree {
    root: PathBuf,
    options: ListOptions,
    expanded: HashSet<PathBuf>,
    children: HashMap<PathBuf, Vec<TreeEntry>>,
}

impl FileTree {
    pub fn new(root: &Path) -> std::io::Result<Self> {
        Self::with_options(root, ListOptions::default())
    }

    pub fn with_options(root: &Path, options: ListOptions) -> std::io::Result<Self> {
        let mut tree = Self { root: root.to_path_buf(), options, expanded: HashSet::new(), children: HashMap::new() };
        tree.load(root.to_path_buf())?;
        Ok(tree)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn load(&mut self, dir: PathBuf) -> std::io::Result<()> {
        let list = list_dir_with(&dir, self.options)?;
        self.children.insert(dir, list);
        Ok(())
    }

    /// Cached children of `dir` (`None` if never loaded).
    pub fn children(&self, dir: &Path) -> Option<&[TreeEntry]> {
        self.children.get(dir).map(Vec::as_slice)
    }

    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// Expand `dir`, reading it from disk if it is not cached.
    pub fn expand(&mut self, dir: &Path) -> std::io::Result<()> {
        if !self.children.contains_key(dir) {
            self.load(dir.to_path_buf())?;
        }
        self.expanded.insert(dir.to_path_buf());
        Ok(())
    }

    pub fn collapse(&mut self, dir: &Path) {
        self.expanded.remove(dir);
    }

    pub fn toggle(&mut self, dir: &Path) -> std::io::Result<()> {
        if self.is_expanded(dir) {
            self.collapse(dir);
            Ok(())
        } else {
            self.expand(dir)
        }
    }

    /// Visible rows in display order (root's children at depth 0).
    pub fn rows(&self) -> Vec<TreeRow<'_>> {
        let mut out = Vec::new();
        self.push_rows(&self.root, 0, &mut out);
        out
    }

    fn push_rows<'a>(&'a self, dir: &Path, depth: usize, out: &mut Vec<TreeRow<'a>>) {
        let Some(list) = self.children.get(dir) else { return };
        for entry in list {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            out.push(TreeRow { depth, entry, expanded });
            if expanded {
                self.push_rows(&entry.path, depth + 1, out);
            }
        }
    }

    /// Re-read a cached directory listing (no-op if it was never loaded).
    /// Cached descendants that no longer exist are dropped.
    pub fn refresh(&mut self, dir: &Path) -> std::io::Result<()> {
        if !self.children.contains_key(dir) {
            return Ok(());
        }
        if !dir.is_dir() {
            self.forget(dir);
            return Ok(());
        }
        self.load(dir.to_path_buf())
    }

    fn forget(&mut self, dir: &Path) {
        self.children.retain(|p, _| !p.starts_with(dir));
        self.expanded.retain(|p| !p.starts_with(dir));
    }

    /// Update the cached listings for a watcher event.
    pub fn apply(&mut self, event: &ProjectEvent) -> std::io::Result<()> {
        let parents = |p: &Path| p.parent().map(Path::to_path_buf);
        let dirs: Vec<PathBuf> = match event {
            ProjectEvent::Created(p) | ProjectEvent::Removed(p) => parents(p).into_iter().collect(),
            ProjectEvent::Renamed { from, to } => {
                if self.children.contains_key(from) {
                    self.forget(from);
                }
                parents(from).into_iter().chain(parents(to)).collect()
            }
            ProjectEvent::Modified(_) => Vec::new(),
            ProjectEvent::Rescan => {
                let loaded: Vec<PathBuf> = self.children.keys().cloned().collect();
                for dir in loaded {
                    self.refresh(&dir)?;
                }
                return Ok(());
            }
        };
        if let ProjectEvent::Removed(p) = event {
            self.forget(p);
        }
        for dir in dirs {
            self.refresh(&dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p, "x").unwrap();
    }

    fn names(list: &[TreeEntry]) -> Vec<String> {
        list.iter().map(TreeEntry::name).collect()
    }

    #[test]
    fn sidecar_names() {
        assert!(is_sidecar(Path::new("/p/.intro.md.toml")));
        assert!(is_sidecar(Path::new(".notes.markdown.toml")));
        assert!(!is_sidecar(Path::new("/p/intro.md.toml")));
        assert!(!is_sidecar(Path::new("/p/.config.toml")));
        assert!(!is_sidecar(Path::new("/p/intro.md")));
        // Agrees with the sidecar crate's naming.
        assert!(is_sidecar(&mdedit_sidecar::sidecar_path(Path::new("/p/draft.md"))));
    }

    #[test]
    fn lists_dirs_first_and_hides_sidecars_and_project_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        for f in ["intro.md", ".intro.md.toml", "Zeta.md", "alpha.txt", ".gitignore", "chapter-1/draft.md"] {
            touch(&r.join(f));
        }
        std::fs::create_dir_all(r.join(".mdedit")).unwrap();
        std::fs::create_dir_all(r.join(".git")).unwrap();
        std::fs::create_dir_all(r.join("assets")).unwrap();

        assert_eq!(names(&list_dir(r).unwrap()), ["assets", "chapter-1", "alpha.txt", "intro.md", "Zeta.md"]);
        let all = list_dir_with(r, ListOptions { show_hidden: true }).unwrap();
        assert_eq!(names(&all), ["assets", "chapter-1", ".gitignore", "alpha.txt", "intro.md", "Zeta.md"]);
    }

    #[test]
    fn tree_is_lazy_and_follows_events() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        touch(&r.join("a.md"));
        touch(&r.join("ch/b.md"));
        touch(&r.join("ch/.b.md.toml"));
        touch(&r.join("ch/deep/c.md"));

        let mut tree = FileTree::new(r).unwrap();
        let ch = r.join("ch");
        assert!(tree.children(&ch).is_none(), "subdirectories are read on expand");
        let rows: Vec<_> = tree.rows().iter().map(|row| (row.depth, row.entry.name())).collect();
        assert_eq!(rows, [(0, "ch".into()), (0, "a.md".into())]);

        tree.expand(&ch).unwrap();
        let rows: Vec<_> = tree.rows().iter().map(|row| (row.depth, row.entry.name(), row.expanded)).collect();
        assert_eq!(
            rows,
            [(0, "ch".into(), true), (1, "deep".into(), false), (1, "b.md".into(), false), (0, "a.md".into(), false)]
        );

        touch(&ch.join("new.md"));
        tree.apply(&ProjectEvent::Created(ch.join("new.md"))).unwrap();
        assert_eq!(names(tree.children(&ch).unwrap()), ["deep", "b.md", "new.md"]);

        std::fs::rename(&ch, r.join("chapter")).unwrap();
        tree.apply(&ProjectEvent::Renamed { from: ch.clone(), to: r.join("chapter") }).unwrap();
        assert!(tree.children(&ch).is_none());
        assert_eq!(names(tree.children(r).unwrap()), ["chapter", "a.md"]);

        tree.toggle(&r.join("chapter")).unwrap();
        assert!(tree.is_expanded(&r.join("chapter")));
        tree.toggle(&r.join("chapter")).unwrap();
        assert!(!tree.is_expanded(&r.join("chapter")));

        std::fs::remove_file(r.join("a.md")).unwrap();
        tree.apply(&ProjectEvent::Rescan).unwrap();
        assert_eq!(names(tree.children(r).unwrap()), ["chapter"]);
    }
}
