//! Git versioning for projects (SPEC 6.3, 6.5; ARCHITECTURE 7).
//!
//! [`Repo`] is built on `gix`: init/open/clone, status, autocommits
//! (`auto: <files>`), checkpoints that squash the autocommits since the
//! previous checkpoint (optionally with an annotated version tag), branches,
//! history with autocommits folded under their checkpoint, fetch and
//! fast-forward pull. [`AutocommitWorker`] debounces saves into autocommits.
//! Push goes through a [`PushBackend`]; the libgit2 one lives in `push.rs`
//! behind the `push` feature and only ever sends checkpoints.
//!
//! Everything here is blocking; the app calls it from a background worker.
//!
//! How autocommits and checkpoints are told apart: a commit whose message
//! starts with [`AUTOCOMMIT_PREFIX`] is an autocommit, every other commit is a
//! checkpoint (including history made by other tools). When a checkpoint
//! squashes autocommits, the old chain is kept under the local-only ref
//! `refs/mdedit/autocommits/<checkpoint id>` so the Git panel can still show
//! (and restore) them. Those refs are never pushed.

#[cfg(feature = "push")]
mod push;
mod repo;
mod worker;
mod worktree;

use std::path::{Path, PathBuf};

pub use repo::{PullOutcome, Repo};
pub use worker::{AutocommitEvent, AutocommitWorker};

/// Message prefix that marks an autocommit.
pub const AUTOCOMMIT_PREFIX: &str = "auto: ";
/// Where the autocommits squashed by a checkpoint are kept (local only).
pub const AUTOCOMMIT_REF_PREFIX: &str = "refs/mdedit/autocommits/";
/// Remote used when none is configured for the branch.
pub const DEFAULT_REMOTE: &str = "origin";

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),
    #[error("would need a merge; sync manually")]
    NeedsMerge,
    #[error("push is not available in this build")]
    PushUnavailable,
    #[error("the working tree has uncommitted changes")]
    Dirty,
    #[error("nothing to commit")]
    NothingToCommit,
    #[error("branch already exists: {0}")]
    BranchExists(String),
    #[error("no such branch: {0}")]
    BranchNotFound(String),
    #[error("cannot delete the current branch: {0}")]
    CurrentBranch(String),
    #[error("tag already exists: {0}")]
    TagExists(String),
    #[error("no such remote: {0}")]
    RemoteNotFound(String),
    #[error("path is outside the repository: {0}")]
    OutsideRepo(PathBuf),
    #[error("the branch has no commits yet")]
    Unborn,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

impl From<gix::Error> for GitError {
    fn from(e: gix::Error) -> Self {
        GitError::Other(e.to_string())
    }
}

pub(crate) fn other(e: impl std::fmt::Display) -> GitError {
    GitError::Other(e.to_string())
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
    /// For a checkpoint in [`Repo::history`]: the autocommits it squashed,
    /// newest first. Empty otherwise.
    pub autocommits: Vec<CommitInfo>,
}

/// Whether a commit message marks an autocommit.
pub fn is_autocommit_message(message: &str) -> bool {
    message.starts_with(AUTOCOMMIT_PREFIX)
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

#[cfg(feature = "push")]
pub use push::Git2Push;

/// The push backend compiled into this build.
pub fn push_backend() -> Box<dyn PushBackend> {
    #[cfg(feature = "push")]
    {
        Box::new(Git2Push::default())
    }
    #[cfg(not(feature = "push"))]
    {
        Box::new(NoPush)
    }
}

pub fn push_available() -> bool {
    cfg!(feature = "push")
}
