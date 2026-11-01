//! Debounced file watcher on the project directory (ARCHITECTURE 6, 9).
//!
//! Events arrive in batches on the watcher's own thread; the app forwards
//! them into its message loop (an iced subscription channel). `.git/`,
//! `.mdedit/cache/` and the temp files of atomic saves are filtered out.
//! The app's own saves show up too; it compares against what it wrote.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::{PROJECT_DIR, ProjectError, fsutil};

pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectEvent {
    /// A new path, or an existing file replaced by an atomic save (temp file
    /// renamed over it): treat `Created` of an open file as a change on disk.
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// Events were lost (overflow or watcher error): re-read everything shown.
    Rescan,
}

impl ProjectEvent {
    /// Paths the event touches.
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            Self::Created(p) | Self::Modified(p) | Self::Removed(p) => vec![p],
            Self::Renamed { from, to } => vec![from, to],
            Self::Rescan => Vec::new(),
        }
    }
}

/// Stops watching when dropped.
pub struct ProjectWatcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl std::fmt::Debug for ProjectWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectWatcher").finish_non_exhaustive()
    }
}

impl ProjectWatcher {
    /// Watch `root` recursively; `on_events` gets each non-empty batch after
    /// `debounce` without new activity.
    pub fn start(
        root: &Path,
        debounce: Duration,
        mut on_events: impl FnMut(Vec<ProjectEvent>) + Send + 'static,
    ) -> Result<Self, ProjectError> {
        let root = fsutil::absolute(root)?;
        let filter_root = root.clone();
        let tick = (debounce / 4).max(Duration::from_millis(10));
        let mut debouncer = new_debouncer(debounce, Some(tick), move |result: DebounceEventResult| {
            let events = convert(&filter_root, result);
            if !events.is_empty() {
                on_events(events);
            }
        })
        .map_err(ProjectError::Watch)?;
        debouncer.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self { _debouncer: debouncer })
    }
}

fn ignored(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.starts_with(".git") || rel.starts_with(Path::new(PROJECT_DIR).join("cache")) || fsutil::is_temp_file(path)
}

fn convert(root: &Path, result: DebounceEventResult) -> Vec<ProjectEvent> {
    let events = match result {
        Ok(events) => events,
        Err(_) => return vec![ProjectEvent::Rescan],
    };
    let mut out: Vec<ProjectEvent> = Vec::new();
    for ev in events {
        if ev.need_rescan() {
            out.push(ProjectEvent::Rescan);
            continue;
        }
        let paths = &ev.paths;
        let converted = match ev.kind {
            EventKind::Create(_) => paths.first().cloned().map(ProjectEvent::Created),
            EventKind::Remove(_) => paths.first().cloned().map(ProjectEvent::Removed),
            EventKind::Modify(ModifyKind::Name(mode)) => match (mode, paths.as_slice()) {
                (RenameMode::Both, [from, to, ..]) => {
                    match (ignored(root, from), ignored(root, to)) {
                        (true, true) => None,
                        // An atomic save: temp file renamed over the target.
                        (true, false) => Some(ProjectEvent::Modified(to.clone())),
                        (false, true) => Some(ProjectEvent::Removed(from.clone())),
                        (false, false) => Some(ProjectEvent::Renamed { from: from.clone(), to: to.clone() }),
                    }
                }
                (RenameMode::From, [p, ..]) => Some(ProjectEvent::Removed(p.clone())),
                (RenameMode::To, [p, ..]) => Some(ProjectEvent::Created(p.clone())),
                (_, [p, ..]) => {
                    Some(if p.exists() { ProjectEvent::Created(p.clone()) } else { ProjectEvent::Removed(p.clone()) })
                }
                _ => None,
            },
            EventKind::Modify(ModifyKind::Metadata(_)) | EventKind::Access(_) => None,
            EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                paths.first().cloned().map(ProjectEvent::Modified)
            }
        };
        let Some(e) = converted else { continue };
        if e.paths().iter().any(|p| ignored(root, p)) {
            continue;
        }
        if !out.contains(&e) {
            out.push(e);
        }
    }
    if out.contains(&ProjectEvent::Rescan) {
        return vec![ProjectEvent::Rescan];
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;

    /// Collect batches until `pred` holds over everything received.
    fn wait_for(rx: &mpsc::Receiver<Vec<ProjectEvent>>, pred: impl Fn(&[ProjectEvent]) -> bool) -> Vec<ProjectEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut all = Vec::new();
        while Instant::now() < deadline {
            if let Ok(batch) = rx.recv_timeout(Duration::from_millis(100)) {
                all.extend(batch);
                if pred(&all) {
                    return all;
                }
            }
        }
        panic!("timed out; got {all:?}");
    }

    fn start(root: &Path) -> (ProjectWatcher, mpsc::Receiver<Vec<ProjectEvent>>) {
        let (tx, rx) = mpsc::channel();
        let w = ProjectWatcher::start(root, Duration::from_millis(100), move |b| {
            let _ = tx.send(b);
        })
        .unwrap();
        (w, rx)
    }

    #[test]
    fn reports_create_modify_rename_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fsutil::absolute(tmp.path()).unwrap();
        let (_w, rx) = start(&root);
        let a = root.join("a.md");

        std::fs::write(&a, "1").unwrap();
        wait_for(&rx, |e| e.contains(&ProjectEvent::Created(a.clone())));

        std::fs::write(&a, "2").unwrap();
        wait_for(&rx, |e| e.iter().any(|x| x.paths().contains(&a.as_path()) && !matches!(x, ProjectEvent::Removed(_))));

        let b = root.join("b.md");
        std::fs::rename(&a, &b).unwrap();
        wait_for(&rx, |e| e.contains(&ProjectEvent::Renamed { from: a.clone(), to: b.clone() }));

        std::fs::remove_file(&b).unwrap();
        wait_for(&rx, |e| e.contains(&ProjectEvent::Removed(b.clone())));
    }

    #[test]
    fn batches_are_debounced() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fsutil::absolute(tmp.path()).unwrap();
        let (_w, rx) = start(&root);
        let a = root.join("burst.md");
        for i in 0..20 {
            std::fs::write(&a, i.to_string()).unwrap();
        }
        let got = wait_for(&rx, |e| e.iter().any(|x| x.paths().contains(&a.as_path())));
        let mentions = got.iter().filter(|x| x.paths().contains(&a.as_path())).count();
        assert!(mentions <= 2, "twenty writes collapse into one or two events: {got:?}");
    }

    #[test]
    fn atomic_save_is_a_modify_and_git_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fsutil::absolute(tmp.path()).unwrap();
        let a = root.join("doc.md");
        std::fs::write(&a, "old").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".mdedit/cache")).unwrap();
        let (_w, rx) = start(&root);

        std::fs::write(root.join(".git/index"), "x").unwrap();
        std::fs::write(root.join(".mdedit/cache/c"), "x").unwrap();
        fsutil::write_atomic(&a, b"new").unwrap();
        let got = wait_for(&rx, |e| e.iter().any(|x| x.paths().contains(&a.as_path())));
        for e in &got {
            for p in e.paths() {
                assert!(!ignored(&root, p), "unexpected {e:?}");
            }
        }
        // The debouncer folds "temp created + renamed over" into a create.
        assert!(
            got.contains(&ProjectEvent::Modified(a.clone())) || got.contains(&ProjectEvent::Created(a.clone())),
            "{got:?}"
        );
    }

    #[test]
    fn filter_rules() {
        let root = Path::new("/p");
        assert!(ignored(root, Path::new("/p/.git/HEAD")));
        assert!(ignored(root, Path::new("/p/.mdedit/cache/x")));
        assert!(ignored(root, Path::new("/p/.a.md.mdedit-tmp-12")));
        assert!(!ignored(root, Path::new("/p/.mdedit/project.toml")));
        assert!(!ignored(root, Path::new("/p/.a.md.toml")));
        assert!(!ignored(root, Path::new("/p/.gitignore")));
    }
}
