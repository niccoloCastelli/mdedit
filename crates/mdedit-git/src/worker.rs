//! Debounced autocommit worker (SPEC 6.3): every save restarts a timer; when
//! it fires, all changed files are committed in one `auto: <files>` commit.
//! Flushing (or dropping the worker, e.g. on quit) commits what is pending.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{Author, CommitInfo, GitError, Repo};

/// What the worker reports after each attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutocommitEvent {
    Committed(CommitInfo),
    /// The timer fired but nothing had changed.
    NothingToCommit,
    Failed(String),
}

enum Msg {
    Saved,
    Flush(Sender<()>),
    SetDebounce(Duration),
    Stop,
}

/// One per open project. Runs on its own thread; all methods are cheap.
pub struct AutocommitWorker {
    tx: Sender<Msg>,
    deadline: Arc<Mutex<Option<Instant>>>,
    thread: Option<JoinHandle<()>>,
}

impl AutocommitWorker {
    /// Start a worker for the repository at `root`. `on_event` runs on the
    /// worker thread after every commit attempt.
    pub fn spawn(
        root: PathBuf,
        author: Author,
        debounce: Duration,
        mut on_event: impl FnMut(AutocommitEvent) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let deadline = Arc::new(Mutex::new(None::<Instant>));
        let shared = deadline.clone();
        let thread = std::thread::Builder::new()
            .name("mdedit-autocommit".into())
            .spawn(move || {
                let mut debounce = debounce;
                let mut commit = |deadline: &Mutex<Option<Instant>>| {
                    *deadline.lock().unwrap() = None;
                    let event = match Repo::open(&root).and_then(|r| r.autocommit_all(&author)) {
                        Ok(info) => AutocommitEvent::Committed(info),
                        Err(GitError::NothingToCommit) => AutocommitEvent::NothingToCommit,
                        Err(e) => AutocommitEvent::Failed(e.to_string()),
                    };
                    on_event(event);
                };
                loop {
                    let due = *shared.lock().unwrap();
                    let msg = match due {
                        Some(due) => match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                            Ok(msg) => msg,
                            Err(RecvTimeoutError::Timeout) => {
                                commit(&shared);
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => Msg::Stop,
                        },
                        None => rx.recv().unwrap_or(Msg::Stop),
                    };
                    let pending = shared.lock().unwrap().is_some();
                    match msg {
                        Msg::Saved => *shared.lock().unwrap() = Some(Instant::now() + debounce),
                        Msg::SetDebounce(d) => {
                            debounce = d;
                            if pending {
                                *shared.lock().unwrap() = Some(Instant::now() + debounce);
                            }
                        }
                        Msg::Flush(done) => {
                            if pending {
                                commit(&shared);
                            }
                            let _ = done.send(());
                        }
                        Msg::Stop => {
                            if pending {
                                commit(&shared);
                            }
                            break;
                        }
                    }
                }
            })
            .expect("spawn autocommit thread");
        Self { tx, deadline, thread: Some(thread) }
    }

    /// A file was saved: (re)start the timer.
    pub fn saved(&self) {
        let _ = self.tx.send(Msg::Saved);
    }

    /// Commit now if a commit is pending, and wait for it.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.tx.send(Msg::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }

    pub fn set_debounce(&self, debounce: Duration) {
        let _ = self.tx.send(Msg::SetDebounce(debounce));
    }

    /// Time left before the pending autocommit (for the status bar).
    pub fn due_in(&self) -> Option<Duration> {
        self.deadline.lock().unwrap().map(|d| d.saturating_duration_since(Instant::now()))
    }
}

impl Drop for AutocommitWorker {
    /// Flushes the pending autocommit, then stops the thread.
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
