//! Git versioning for projects (SPEC 6.3, 6.5; ARCHITECTURE 7).
//!
//! Git workstream: implement [`Repo`] with `gix` (add `gix.workspace = true`
//! with the features you need), the debounced autocommit worker, checkpoint
//! squash + annotated version tags, branches (create/switch/rename/delete),
//! history for the Git panel, clone/fetch/fast-forward pull, and
//! [`PushBackend`] in `push.rs` behind the `push` feature. All tests run
//! against temp repos. Everything here is blocking; the app calls it from a
//! background worker.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),
    #[error("would need a merge; sync manually")]
    NeedsMerge,
    #[error("push is not available in this build")]
    PushUnavailable,
    #[error("{0}")]
    Other(String),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: String,
    pub summary: String,
    pub time_unix: i64,
    /// Autocommits are folded under the checkpoint that squashed them.
    pub is_checkpoint: bool,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoStatus {
    pub branch: String,
    pub changed_files: Vec<PathBuf>,
    /// Autocommits since the last checkpoint (they block push).
    pub pending_autocommits: usize,
    pub ahead: usize,
    pub behind: usize,
}

/// A project's repository.
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    pub fn open(root: &Path) -> Result<Self> {
        Ok(Self { root: root.to_path_buf() })
    }

    pub fn init(root: &Path, default_branch: &str) -> Result<Self> {
        let _ = (root, default_branch);
        Err(GitError::Unimplemented("Repo::init"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn status(&self) -> Result<RepoStatus> {
        Err(GitError::Unimplemented("Repo::status"))
    }

    /// Commit `files` as `auto: <files>`.
    pub fn autocommit(&self, files: &[PathBuf], author: &Author) -> Result<CommitInfo> {
        let _ = (files, author);
        Err(GitError::Unimplemented("Repo::autocommit"))
    }

    /// Squash autocommits since the last checkpoint into one commit, with an
    /// optional annotated tag.
    pub fn checkpoint(&self, message: &str, tag: Option<&str>, author: &Author) -> Result<CommitInfo> {
        let _ = (message, tag, author);
        Err(GitError::Unimplemented("Repo::checkpoint"))
    }

    pub fn branches(&self) -> Result<Vec<String>> {
        Err(GitError::Unimplemented("Repo::branches"))
    }

    pub fn create_branch(&self, name: &str) -> Result<()> {
        let _ = name;
        Err(GitError::Unimplemented("Repo::create_branch"))
    }

    pub fn switch_branch(&self, name: &str) -> Result<()> {
        let _ = name;
        Err(GitError::Unimplemented("Repo::switch_branch"))
    }

    pub fn history(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let _ = limit;
        Err(GitError::Unimplemented("Repo::history"))
    }

    /// Fetch and fast-forward; `NeedsMerge` if it can't.
    pub fn pull(&self) -> Result<()> {
        Err(GitError::Unimplemented("Repo::pull"))
    }
}

/// Push implementation, swappable so builds without libgit2 still work.
pub trait PushBackend: Send + Sync {
    /// Push the current branch up to the last checkpoint.
    fn push(&self, repo: &Path, remote: &str, branch: &str) -> Result<()>;
}

/// Used when the `push` feature is off.
pub struct NoPush;

impl PushBackend for NoPush {
    fn push(&self, _repo: &Path, _remote: &str, _branch: &str) -> Result<()> {
        Err(GitError::PushUnavailable)
    }
}

/// The push backend compiled into this build.
pub fn push_backend() -> Box<dyn PushBackend> {
    Box::new(NoPush)
}

pub fn push_available() -> bool {
    cfg!(feature = "push")
}
