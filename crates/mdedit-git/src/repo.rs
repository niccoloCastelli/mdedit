//! [`Repo`]: the project repository on top of `gix`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use gix::ObjectId;
use gix::bstr::{BString, ByteSlice};
use gix::object::tree::EntryKind;
use gix::progress::Discard;
use gix::refs::transaction::{PreviousValue, RefEdit};
use gix::refs::{FullName, Target};
use gix::remote::Direction;

use crate::worktree;
use crate::{
    AUTOCOMMIT_PREFIX, AUTOCOMMIT_REF_PREFIX, Author, CommitInfo, DEFAULT_REMOTE, GitError, PushBackend, RepoStatus,
    Result, is_autocommit_message, other,
};

/// What a pull did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullOutcome {
    /// Local and remote are the same, or the remote has no such branch.
    UpToDate,
    /// The branch moved forward to the remote's commit.
    FastForwarded,
    /// The remote is behind (local checkpoints or autocommits on top).
    Ahead,
}

/// How many file names an autocommit message lists before "and N more".
const MESSAGE_FILES: usize = 5;

/// A project's repository.
pub struct Repo {
    root: PathBuf,
    /// Reopened when this crate changes the config (e.g. `add_remote`).
    repo: RwLock<gix::ThreadSafeRepository>,
}

impl Repo {
    pub fn open(root: &Path) -> Result<Self> {
        let repo = gix::open(root).map_err(|_| GitError::NotARepo(root.to_path_buf()))?;
        if repo.workdir().is_none() {
            return Err(GitError::NotARepo(root.to_path_buf()));
        }
        Ok(Self { root: root.to_path_buf(), repo: RwLock::new(repo.into_sync()) })
    }

    /// Create a repository in `root` (or open the one already there) with
    /// HEAD on `default_branch`, and ignore `.mdedit/cache/`.
    pub fn init(root: &Path, default_branch: &str) -> Result<Self> {
        if root.join(".git").exists() {
            return Self::open(root);
        }
        std::fs::create_dir_all(root)?;
        let repo = gix::init(root)?;
        let branch = branch_ref(default_branch)?;
        let me = reflog_author(&repo);
        edit_refs(&repo, [RefEdit::update(head_name(), Target::Symbolic(branch), PreviousValue::Any, "init")], &me)?;
        ensure_ignored(root, ".mdedit/cache/")?;
        Ok(Self { root: root.to_path_buf(), repo: RwLock::new(repo.into_sync()) })
    }

    /// Clone `url` (HTTPS, SSH or a local path) into `dest` and check out the
    /// remote's default branch.
    pub fn clone(url: &str, dest: &Path) -> Result<Self> {
        let interrupt = AtomicBool::new(false);
        let mut fetch = gix::prepare_clone(url, dest)?;
        let (mut checkout, _) = fetch.fetch_then_checkout(Discard, &interrupt)?;
        let (repo, _) = checkout.main_worktree(Discard, &interrupt)?;
        Ok(Self { root: dest.to_path_buf(), repo: RwLock::new(repo.into_sync()) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn local(&self) -> gix::Repository {
        self.repo.read().unwrap_or_else(|e| e.into_inner()).to_thread_local()
    }

    /// Name of the current branch (`HEAD` when detached).
    pub fn current_branch(&self) -> Result<String> {
        Ok(current_branch(&self.local())?.unwrap_or_else(|| "HEAD".into()))
    }

    pub fn status(&self) -> Result<RepoStatus> {
        let repo = self.local();
        let branch = current_branch(&repo)?;
        let head = head_id(&repo)?;
        let pending_autocommits = match head {
            Some(id) => checkpoint_chain(&repo, id)?.0.len(),
            None => 0,
        };
        let (mut ahead, mut behind) = (0, 0);
        if let (Some(branch), Some(local)) = (&branch, head)
            && let Some(upstream) = upstream_id(&repo, branch)?
        {
            ahead = count_only_in(&repo, local, upstream)?;
            behind = count_only_in(&repo, upstream, local)?;
        }
        Ok(RepoStatus {
            branch: branch.unwrap_or_else(|| "HEAD".into()),
            changed_files: changed_files(&repo)?.iter().map(|p| rela_to_path(p.as_ref())).collect(),
            pending_autocommits,
            ahead,
            behind,
        })
    }

    /// Commit `files` as `auto: <files>`. Paths may be absolute (inside the
    /// repo) or relative to the root; deleted files are removed from the
    /// commit. `NothingToCommit` if none of them changed.
    pub fn autocommit(&self, files: &[PathBuf], author: &Author) -> Result<CommitInfo> {
        let mut rels = files.iter().map(|f| self.rela_path(f)).collect::<Result<Vec<_>>>()?;
        rels.sort();
        rels.dedup();
        commit_files(&self.local(), &self.root, &rels, author)
    }

    /// Autocommit every changed or untracked (not ignored) file.
    pub fn autocommit_all(&self, author: &Author) -> Result<CommitInfo> {
        let repo = self.local();
        let rels = changed_files(&repo)?;
        commit_files(&repo, &self.root, &rels, author)
    }

    /// Squash autocommits since the last checkpoint into one commit, with an
    /// optional annotated tag. Uncommitted changes are included. With no
    /// autocommits, only the tag is added to HEAD (or `NothingToCommit`).
    pub fn checkpoint(&self, message: &str, tag: Option<&str>, author: &Author) -> Result<CommitInfo> {
        let repo = self.local();
        if let Some(tag) = tag
            && repo.try_find_reference(format!("refs/tags/{tag}").as_str())?.is_some()
        {
            return Err(GitError::TagExists(tag.into()));
        }
        match self.autocommit_all(author) {
            Ok(_) | Err(GitError::NothingToCommit) => {}
            Err(e) => return Err(e),
        }
        let repo = self.local();
        let head = head_id(&repo)?.ok_or(GitError::Unborn)?;
        let (autos, base) = checkpoint_chain(&repo, head)?;
        if autos.is_empty() {
            let Some(tag) = tag else { return Err(GitError::NothingToCommit) };
            create_tag(&repo, tag, head, message, author)?;
            return commit_info(&repo, head, Some(tag.into()));
        }
        let tree = repo.find_commit(head)?.tree_id()?.detach();
        let sig = signature(author);
        let mut time = gix::date::parse::TimeBuf::default();
        let sig = sig.to_ref(&mut time);
        let new = repo.new_commit_as(sig, sig, message, tree, base)?.id;
        let fold_ref: FullName = format!("{AUTOCOMMIT_REF_PREFIX}{new}").try_into().map_err(other)?;
        edit_refs(
            &repo,
            [
                RefEdit::update(head_name(), new, PreviousValue::MustExistAndMatch(head.into()), "checkpoint")
                    .with_deref(true),
                RefEdit::update(fold_ref, head, PreviousValue::MustNotExist, "checkpoint"),
            ],
            author,
        )?;
        if let Some(tag) = tag {
            create_tag(&repo, tag, new, message, author)?;
        }
        commit_info(&repo, new, tag.map(Into::into))
    }

    /// Add an annotated tag on the current commit.
    pub fn tag(&self, name: &str, message: &str, author: &Author) -> Result<()> {
        let repo = self.local();
        let head = head_id(&repo)?.ok_or(GitError::Unborn)?;
        create_tag(&repo, name, head, message, author)
    }

    pub fn tags(&self) -> Result<Vec<String>> {
        let repo = self.local();
        let mut tags = tag_map(&repo)?.into_values().collect::<Vec<_>>();
        tags.sort();
        Ok(tags)
    }

    /// The last checkpoint on the current branch (what push would send).
    pub fn last_checkpoint(&self) -> Result<Option<String>> {
        let repo = self.local();
        let Some(head) = head_id(&repo)? else { return Ok(None) };
        Ok(checkpoint_chain(&repo, head)?.1.map(|id| id.to_string()))
    }

    pub fn branches(&self) -> Result<Vec<String>> {
        let repo = self.local();
        let mut names = Vec::new();
        for r in repo.references()?.local_branches()? {
            let r = r.map_err(other)?;
            names.push(r.name().shorten().to_string());
        }
        names.sort();
        Ok(names)
    }

    /// Create `name` at the current commit (does not switch to it).
    pub fn create_branch(&self, name: &str) -> Result<()> {
        let repo = self.local();
        let full = branch_ref(name)?;
        if repo.try_find_reference(full.as_ref())?.is_some() {
            return Err(GitError::BranchExists(name.into()));
        }
        let head = head_id(&repo)?.ok_or(GitError::Unborn)?;
        let me = reflog_author(&repo);
        edit_refs(&repo, [RefEdit::update(full, head, PreviousValue::MustNotExist, "branch: Created from HEAD")], &me)?;
        Ok(())
    }

    /// Check out `name`. Fails with `Dirty` if tracked files have uncommitted
    /// changes (the app autocommits first) or an untracked file would be
    /// overwritten.
    pub fn switch_branch(&self, name: &str) -> Result<()> {
        let repo = self.local();
        let full = branch_ref(name)?;
        let target = repo
            .try_find_reference(full.as_ref())?
            .ok_or_else(|| GitError::BranchNotFound(name.into()))?
            .into_fully_peeled_id()?
            .detach();
        let current = current_branch(&repo)?;
        if current.as_deref() == Some(name) {
            return Ok(());
        }
        if repo.is_dirty()? {
            return Err(GitError::Dirty);
        }
        let from = repo.head_tree_id_or_empty()?.detach();
        let to = repo.find_commit(target)?.tree_id()?.detach();
        worktree::checkout(&repo, &self.root, from, to)?;
        let me = reflog_author(&repo);
        let msg = format!("checkout: moving from {} to {name}", current.unwrap_or_else(|| "HEAD".into()));
        edit_refs(&repo, [RefEdit::update(head_name(), Target::Symbolic(full), PreviousValue::Any, msg)], &me)?;
        Ok(())
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        let repo = self.local();
        let old_ref = branch_ref(old)?;
        let new_ref = branch_ref(new)?;
        let id = repo
            .try_find_reference(old_ref.as_ref())?
            .ok_or_else(|| GitError::BranchNotFound(old.into()))?
            .into_fully_peeled_id()?
            .detach();
        if repo.try_find_reference(new_ref.as_ref())?.is_some() {
            return Err(GitError::BranchExists(new.into()));
        }
        let me = reflog_author(&repo);
        let msg = format!("branch: renamed {old} to {new}");
        edit_refs(&repo, [RefEdit::update(new_ref.clone(), id, PreviousValue::MustNotExist, msg.clone())], &me)?;
        if current_branch(&repo)?.as_deref() == Some(old) {
            edit_refs(&repo, [RefEdit::update(head_name(), Target::Symbolic(new_ref), PreviousValue::Any, msg)], &me)?;
        }
        edit_refs(&repo, [RefEdit::delete(old_ref, PreviousValue::MustExistAndMatch(id.into()))], &me)?;
        Ok(())
    }

    /// Delete a branch other than the current one (the UI confirms first).
    pub fn delete_branch(&self, name: &str) -> Result<()> {
        let repo = self.local();
        if current_branch(&repo)?.as_deref() == Some(name) {
            return Err(GitError::CurrentBranch(name.into()));
        }
        let full = branch_ref(name)?;
        if repo.try_find_reference(full.as_ref())?.is_none() {
            return Err(GitError::BranchNotFound(name.into()));
        }
        let me = reflog_author(&repo);
        edit_refs(&repo, [RefEdit::delete(full, PreviousValue::MustExist)], &me)?;
        Ok(())
    }

    /// Up to `limit` entries of the current branch, newest first, following
    /// first parents: pending autocommits, then checkpoints with the
    /// autocommits they squashed folded into [`CommitInfo::autocommits`].
    pub fn history(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let repo = self.local();
        let Some(mut id) = head_id(&repo)? else { return Ok(Vec::new()) };
        let tags = tag_map(&repo)?;
        let mut out = Vec::new();
        while out.len() < limit {
            let commit = repo.find_commit(id)?;
            let parent = commit.parent_ids().next().map(|p| p.detach());
            let mut info = commit_info(&repo, id, tags.get(&id).cloned())?;
            if info.is_checkpoint {
                info.autocommits = folded_autocommits(&repo, id, parent, &tags)?;
            }
            out.push(info);
            match parent {
                Some(p) => id = p,
                None => break,
            }
        }
        Ok(out)
    }

    /// Register a remote (fetch refspec `+refs/heads/*:refs/remotes/<name>/*`).
    pub fn add_remote(&self, name: &str, url: &str) -> Result<()> {
        let repo = self.local();
        let spec = format!("+refs/heads/*:refs/remotes/{name}/*");
        let mut remote = repo.remote_at(url)?.with_refspecs(Some(spec.as_str()), Direction::Fetch)?;
        let path = repo.git_dir().join("config");
        let mut config =
            gix::config::File::from_path_no_includes(path.clone(), gix::config::Source::Local).map_err(other)?;
        remote.save_as_to(name, &mut config)?;
        std::fs::write(&path, config.to_bstring())?;
        let reopened = gix::open(&self.root)?.into_sync();
        *self.repo.write().unwrap_or_else(|e| e.into_inner()) = reopened;
        Ok(())
    }

    /// The remote configured for the current branch, else `origin`.
    pub fn remote_for_branch(&self) -> Result<String> {
        let repo = self.local();
        Ok(match current_branch(&repo)? {
            Some(b) => branch_remote(&repo, &b),
            None => DEFAULT_REMOTE.into(),
        })
    }

    /// Fetch `remote` (updates `refs/remotes/<remote>/*` and tags).
    pub fn fetch(&self, remote: &str) -> Result<()> {
        let repo = self.local();
        let r = repo.find_remote(remote).map_err(|_| GitError::RemoteNotFound(remote.into()))?;
        let interrupt = AtomicBool::new(false);
        r.connect(Direction::Fetch)?.prepare_fetch(Discard, Default::default())?.receive(Discard, &interrupt)?;
        Ok(())
    }

    /// Fetch and fast-forward; `NeedsMerge` if it can't.
    pub fn pull(&self) -> Result<()> {
        self.pull_from(&self.remote_for_branch()?).map(|_| ())
    }

    /// Fetch `remote` and fast-forward the current branch to its
    /// counterpart. `NeedsMerge` if both sides moved, `Dirty` if a
    /// fast-forward is needed but tracked files have uncommitted changes.
    pub fn pull_from(&self, remote: &str) -> Result<PullOutcome> {
        self.fetch(remote)?;
        let repo = self.local();
        let branch = current_branch(&repo)?.ok_or_else(|| other("HEAD is detached"))?;
        let up_ref: FullName = format!("refs/remotes/{remote}/{branch}").try_into().map_err(other)?;
        let Some(up) = repo.try_find_reference(up_ref.as_ref())? else { return Ok(PullOutcome::UpToDate) };
        let up = up.into_fully_peeled_id()?.detach();
        let local = head_id(&repo)?;
        if local == Some(up) {
            return Ok(PullOutcome::UpToDate);
        }
        if let Some(local) = local {
            let base = repo.merge_base(local, up).map_err(other)?.map(|id| id.detach());
            if base == Some(up) {
                return Ok(PullOutcome::Ahead);
            }
            if base != Some(local) {
                return Err(GitError::NeedsMerge);
            }
        }
        if repo.is_dirty()? {
            return Err(GitError::Dirty);
        }
        let from = repo.head_tree_id_or_empty()?.detach();
        let to = repo.find_commit(up)?.tree_id()?.detach();
        worktree::checkout(&repo, &self.root, from, to)?;
        let expected = match local {
            Some(id) => PreviousValue::MustExistAndMatch(id.into()),
            None => PreviousValue::Any,
        };
        let me = reflog_author(&repo);
        let msg = format!("pull: fast-forward from {remote}");
        edit_refs(&repo, [RefEdit::update(head_name(), up, expected, msg).with_deref(true)], &me)?;
        Ok(PullOutcome::FastForwarded)
    }

    /// Push the current branch up to its last checkpoint through `backend`.
    pub fn push(&self, backend: &dyn PushBackend) -> Result<()> {
        let branch = current_branch(&self.local())?.ok_or_else(|| other("HEAD is detached"))?;
        backend.push(&self.root, &self.remote_for_branch()?, &branch)
    }

    /// `path` relative to the root, `/`-separated, without `..`.
    fn rela_path(&self, path: &Path) -> Result<BString> {
        let outside = || GitError::OutsideRepo(path.to_path_buf());
        let rel = if path.is_absolute() {
            match path.strip_prefix(&self.root) {
                Ok(rel) => rel.to_path_buf(),
                Err(_) => {
                    // Try again with symlinks resolved (the file itself may be deleted).
                    let root = self.root.canonicalize()?;
                    let parent = path.parent().ok_or_else(outside)?.canonicalize().map_err(|_| outside())?;
                    let full = parent.join(path.file_name().ok_or_else(outside)?);
                    full.strip_prefix(&root).map_err(|_| outside())?.to_path_buf()
                }
            }
        } else {
            path.to_path_buf()
        };
        let mut parts = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_str().ok_or_else(|| other("path is not UTF-8"))?.to_owned()),
                Component::CurDir => {}
                _ => return Err(GitError::OutsideRepo(path.to_path_buf())),
            }
        }
        if parts.is_empty() || parts[0] == ".git" {
            return Err(GitError::OutsideRepo(path.to_path_buf()));
        }
        Ok(parts.join("/").into())
    }
}

fn head_name() -> FullName {
    "HEAD".try_into().expect("valid ref name")
}

fn branch_ref(name: &str) -> Result<FullName> {
    format!("refs/heads/{name}").try_into().map_err(|_| other(format!("invalid branch name: {name}")))
}

fn head_id(repo: &gix::Repository) -> Result<Option<ObjectId>> {
    Ok(repo.head()?.id().map(|id| id.detach()))
}

fn current_branch(repo: &gix::Repository) -> Result<Option<String>> {
    let head = repo.head()?;
    Ok(head.referent_name().map(|n| n.shorten().to_string()))
}

fn branch_remote(repo: &gix::Repository, branch: &str) -> String {
    repo.config_snapshot()
        .string(format!("branch.{branch}.remote").as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| DEFAULT_REMOTE.into())
}

fn upstream_id(repo: &gix::Repository, branch: &str) -> Result<Option<ObjectId>> {
    let remote = branch_remote(repo, branch);
    let name = format!("refs/remotes/{remote}/{branch}");
    match repo.try_find_reference(name.as_str())? {
        Some(r) => Ok(Some(r.into_fully_peeled_id()?.detach())),
        None => Ok(None),
    }
}

/// Commits reachable from `tip` but not from `hidden`.
fn count_only_in(repo: &gix::Repository, tip: ObjectId, hidden: ObjectId) -> Result<usize> {
    let mut n = 0;
    for info in repo.rev_walk([tip]).with_hidden([hidden]).all()? {
        info.map_err(other)?;
        n += 1;
    }
    Ok(n)
}

/// Walk first parents from `start`: the autocommits on top (newest first)
/// and the checkpoint below them, if any.
fn checkpoint_chain(repo: &gix::Repository, start: ObjectId) -> Result<(Vec<ObjectId>, Option<ObjectId>)> {
    let mut autos = Vec::new();
    let mut id = start;
    loop {
        let commit = repo.find_commit(id)?;
        if !is_autocommit_message(&commit.message_raw_sloppy().to_str_lossy()) {
            return Ok((autos, Some(id)));
        }
        autos.push(id);
        match commit.parent_ids().next() {
            Some(p) => id = p.detach(),
            None => return Ok((autos, None)),
        }
    }
}

/// The autocommits `checkpoint` squashed, from its fold ref down to (not
/// including) the checkpoint's parent.
fn folded_autocommits(
    repo: &gix::Repository,
    checkpoint: ObjectId,
    stop: Option<ObjectId>,
    tags: &HashMap<ObjectId, String>,
) -> Result<Vec<CommitInfo>> {
    let name = format!("{AUTOCOMMIT_REF_PREFIX}{checkpoint}");
    let Some(r) = repo.try_find_reference(name.as_str())? else { return Ok(Vec::new()) };
    let mut id = r.into_fully_peeled_id()?.detach();
    let mut out = Vec::new();
    while Some(id) != stop {
        let info = commit_info(repo, id, tags.get(&id).cloned())?;
        if info.is_checkpoint {
            break;
        }
        out.push(info);
        match repo.find_commit(id)?.parent_ids().next() {
            Some(p) => id = p.detach(),
            None => break,
        }
    }
    Ok(out)
}

fn commit_info(repo: &gix::Repository, id: ObjectId, tag: Option<String>) -> Result<CommitInfo> {
    let commit = repo.find_commit(id)?;
    let message = commit.message_raw_sloppy().to_str_lossy().into_owned();
    let summary = message.lines().next().unwrap_or_default().trim().to_owned();
    Ok(CommitInfo {
        id: id.to_string(),
        is_checkpoint: !is_autocommit_message(&message),
        summary,
        time_unix: commit.time()?.seconds,
        tag,
        autocommits: Vec::new(),
    })
}

/// Commit id → tag name (one per commit, the first in name order).
fn tag_map(repo: &gix::Repository) -> Result<HashMap<ObjectId, String>> {
    let mut map = HashMap::new();
    let mut tags = Vec::new();
    for r in repo.references()?.tags()? {
        let r = r.map_err(other)?;
        let name = r.name().shorten().to_string();
        if let Ok(id) = r.into_fully_peeled_id() {
            tags.push((name, id.detach()));
        }
    }
    tags.sort();
    for (name, id) in tags {
        map.entry(id).or_insert(name);
    }
    Ok(map)
}

fn create_tag(repo: &gix::Repository, name: &str, target: ObjectId, message: &str, author: &Author) -> Result<()> {
    if repo.try_find_reference(format!("refs/tags/{name}").as_str())?.is_some() {
        return Err(GitError::TagExists(name.into()));
    }
    let sig = signature(author);
    let mut time = gix::date::parse::TimeBuf::default();
    repo.tag(name, target, gix::objs::Kind::Commit, Some(sig.to_ref(&mut time)), message, PreviousValue::MustNotExist)?;
    Ok(())
}

/// Changed, deleted and untracked (not ignored) paths, repo-relative.
fn changed_files(repo: &gix::Repository) -> Result<Vec<BString>> {
    use gix::status::Item;
    let iter = repo
        .status(Discard)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .index_worktree_rewrites(None)
        .into_iter(Vec::<BString>::new())?;
    let mut out = Vec::new();
    for item in iter {
        let item = item?;
        if let Item::IndexWorktree(i) = &item
            && i.summary().is_none()
        {
            continue;
        }
        if let Item::TreeIndex(change) = &item
            && let gix::diff::index::ChangeRef::Rewrite { source_location, .. } = change
        {
            out.push(source_location.as_ref().to_owned());
        }
        out.push(item.location().to_owned());
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn rela_to_path(rela: &gix::bstr::BStr) -> PathBuf {
    rela.to_str_lossy().split('/').collect()
}

fn autocommit_message(files: &[BString]) -> String {
    let mut msg = String::from(AUTOCOMMIT_PREFIX);
    let shown = files.iter().take(MESSAGE_FILES).map(|f| f.to_str_lossy()).collect::<Vec<_>>().join(", ");
    msg.push_str(&shown);
    if files.len() > MESSAGE_FILES {
        msg.push_str(&format!(" and {} more", files.len() - MESSAGE_FILES));
    }
    msg
}

/// Build a commit from HEAD's tree plus `rels` as they are on disk (missing
/// files are removed), move HEAD's branch to it and refresh the index.
fn commit_files(repo: &gix::Repository, root: &Path, rels: &[BString], author: &Author) -> Result<CommitInfo> {
    let head = head_id(repo)?;
    let base_tree = repo.head_tree_id_or_empty()?.detach();
    let mut editor = repo.edit_tree(base_tree)?;
    let mut fresh = HashMap::new();
    let mut changed = Vec::new();
    for rel in rels {
        let fs = worktree::fs_path(root, rel.as_ref())?;
        let existing = editor.get(rel).map(|e| (e.id().detach(), e.mode().kind()));
        match std::fs::symlink_metadata(&fs) {
            Ok(meta) if meta.is_file() || meta.file_type().is_symlink() => {
                let stat = worktree::stat_of(&fs)?;
                let (data, kind) = if meta.file_type().is_symlink() {
                    let target =
                        gix::path::os_string_into_bstring(std::fs::read_link(&fs)?.into_os_string()).map_err(other)?;
                    (Vec::from(target), EntryKind::Link)
                } else {
                    (std::fs::read(&fs)?, file_kind(&meta))
                };
                let id = repo.write_blob(&data)?.detach();
                if existing != Some((id, kind)) {
                    editor.upsert(rel, kind, id)?;
                    changed.push(rel.clone());
                }
                fresh.insert(rel.clone(), stat);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if existing.is_some() {
                    editor.remove(rel)?;
                    changed.push(rel.clone());
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    if changed.is_empty() {
        return Err(GitError::NothingToCommit);
    }
    let tree = editor.write()?.detach();
    let sig = signature(author);
    let mut time = gix::date::parse::TimeBuf::default();
    let sig = sig.to_ref(&mut time);
    let id = repo.commit_as(sig, sig, "HEAD", autocommit_message(&changed), tree, head)?.detach();
    worktree::write_index(repo, tree, &fresh)?;
    commit_info(repo, id, None)
}

#[cfg(unix)]
fn file_kind(meta: &std::fs::Metadata) -> EntryKind {
    use std::os::unix::fs::PermissionsExt;
    if meta.permissions().mode() & 0o111 != 0 { EntryKind::BlobExecutable } else { EntryKind::Blob }
}

#[cfg(not(unix))]
fn file_kind(_meta: &std::fs::Metadata) -> EntryKind {
    EntryKind::Blob
}

fn signature(author: &Author) -> gix::actor::Signature {
    gix::actor::Signature {
        name: author.name.as_str().into(),
        email: author.email.as_str().into(),
        time: gix::date::Time::now_local_or_utc(),
    }
}

/// Identity for reflog entries of operations that take no author.
fn reflog_author(repo: &gix::Repository) -> Author {
    match repo.committer() {
        Some(Ok(sig)) => Author { name: sig.name.to_string(), email: sig.email.to_string() },
        _ => Author { name: "mdedit".into(), email: "mdedit@localhost".into() },
    }
}

fn edit_refs(repo: &gix::Repository, edits: impl IntoIterator<Item = RefEdit>, author: &Author) -> Result<()> {
    let sig = signature(author);
    let mut time = gix::date::parse::TimeBuf::default();
    repo.edit_references_as(edits, Some(sig.to_ref(&mut time)))?;
    Ok(())
}

/// Append `pattern` to `.gitignore` unless a line already says it.
fn ensure_ignored(root: &Path, pattern: &str) -> Result<()> {
    let path = root.join(".gitignore");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(pattern);
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}
