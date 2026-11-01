//! Watching open files for external changes (SPEC 3).
//!
//! One `notify` watcher per set of open paths, restarted by iced whenever
//! the set changes. Parent directories are watched (not the files), because
//! many editors save by writing a new file and renaming it over the old one,
//! which a watch on the old inode would miss. The app compares the file's
//! size and mtime with what it last saw, so its own saves are ignored.

use std::collections::BTreeSet;
use std::path::PathBuf;

use iced::Subscription;
use iced::futures::{SinkExt, StreamExt, channel::mpsc};
use notify::{RecursiveMode, Watcher};

/// Emits a path each time one of `paths` may have changed on disk.
pub fn changes(paths: Vec<PathBuf>) -> Subscription<PathBuf> {
    if paths.is_empty() {
        return Subscription::none();
    }
    Subscription::run_with(paths, |paths| {
        let paths = paths.clone();
        iced::stream::channel(64, async move |mut output: mpsc::Sender<PathBuf>| {
            let (tx, mut rx) = mpsc::unbounded::<PathBuf>();
            let wanted: BTreeSet<PathBuf> = paths.iter().cloned().collect();
            let handler = move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    for p in event.paths {
                        if wanted.contains(&p) {
                            let _ = tx.unbounded_send(p);
                        }
                    }
                }
            };
            let Ok(mut watcher) = notify::recommended_watcher(handler) else {
                return;
            };
            let dirs: BTreeSet<PathBuf> = paths.iter().filter_map(|p| p.parent().map(PathBuf::from)).collect();
            for dir in dirs {
                let _ = watcher.watch(&dir, RecursiveMode::NonRecursive);
            }
            while let Some(path) = rx.next().await {
                if output.send(path).await.is_err() {
                    break;
                }
            }
            drop(watcher);
        })
    })
}
