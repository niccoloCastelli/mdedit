mod common;

use std::path::PathBuf;

use common::{git, me, read, repo_with_checkpoint, write};
use mdedit_git::{GitError, PullOutcome, Repo};

#[test]
fn open_rejects_plain_directory() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(Repo::open(dir.path()), Err(GitError::NotARepo(_))));
}

#[test]
fn init_sets_branch_and_ignores_cache() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::init(dir.path(), "notes").unwrap();
    let status = repo.status().unwrap();
    assert_eq!(status.branch, "notes");
    assert_eq!(status.changed_files, vec![PathBuf::from(".gitignore")]);
    assert_eq!(status.pending_autocommits, 0);
    assert!(read(dir.path(), ".gitignore").lines().any(|l| l == ".mdedit/cache/"));
    assert!(repo.history(10).unwrap().is_empty());
    assert_eq!(git(dir.path(), &["symbolic-ref", "HEAD"]), "refs/heads/notes");

    // Opening again and re-running init are both fine.
    Repo::open(dir.path()).unwrap();
    Repo::init(dir.path(), "notes").unwrap();
    assert_eq!(read(dir.path(), ".gitignore").matches(".mdedit/cache/").count(), 1);

    write(dir.path(), ".mdedit/cache/x.bin", "cache");
    assert_eq!(repo.status().unwrap().changed_files, vec![PathBuf::from(".gitignore")]);
}

#[test]
fn autocommit_commits_only_the_given_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = Repo::init(root, "main").unwrap();
    write(root, "a.md", "a");
    write(root, "sub/b.md", "b");

    let info = repo.autocommit(&[PathBuf::from("a.md")], &me()).unwrap();
    assert_eq!(info.summary, "auto: a.md");
    assert!(!info.is_checkpoint);
    assert_eq!(git(root, &["ls-tree", "-r", "--name-only", "HEAD"]), "a.md");
    // The index matches what was committed: git sees only the other files.
    assert_eq!(git(root, &["status", "--porcelain"]), "?? .gitignore\n?? sub/");
    assert_eq!(repo.status().unwrap().changed_files, vec![PathBuf::from(".gitignore"), PathBuf::from("sub/b.md")]);

    // Absolute paths work, and so does a list of several files.
    write(root, "a.md", "a2");
    let info = repo.autocommit(&[root.join("sub/b.md"), root.join("a.md")], &me()).unwrap();
    assert_eq!(info.summary, "auto: a.md, sub/b.md");
    assert_eq!(git(root, &["show", "HEAD:a.md"]), "a2");

    // Unchanged files: nothing to commit.
    assert!(matches!(repo.autocommit(&[PathBuf::from("a.md")], &me()), Err(GitError::NothingToCommit)));
    assert!(matches!(repo.autocommit(&[], &me()), Err(GitError::NothingToCommit)));

    // Deleting a file and autocommitting it removes it from the tree.
    std::fs::remove_file(root.join("a.md")).unwrap();
    let info = repo.autocommit(&[PathBuf::from("a.md")], &me()).unwrap();
    assert_eq!(info.summary, "auto: a.md");
    assert_eq!(git(root, &["ls-tree", "-r", "--name-only", "HEAD"]), "sub/b.md");

    // Paths outside the repository are refused.
    let elsewhere = tempfile::tempdir().unwrap();
    write(elsewhere.path(), "x.md", "x");
    assert!(matches!(repo.autocommit(&[elsewhere.path().join("x.md")], &me()), Err(GitError::OutsideRepo(_))));
    assert!(matches!(repo.autocommit(&[PathBuf::from("../x.md")], &me()), Err(GitError::OutsideRepo(_))));
    assert!(matches!(repo.autocommit(&[PathBuf::from(".git/config")], &me()), Err(GitError::OutsideRepo(_))));
}

#[test]
fn autocommit_all_leaves_a_clean_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = Repo::init(root, "main").unwrap();
    for i in 0..7 {
        write(root, &format!("f{i}.md"), "x");
    }
    let info = repo.autocommit_all(&me()).unwrap();
    assert_eq!(info.summary, "auto: .gitignore, f0.md, f1.md, f2.md, f3.md and 3 more");
    assert!(repo.status().unwrap().changed_files.is_empty());
    assert_eq!(git(root, &["status", "--porcelain"]), "");
    assert_eq!(repo.status().unwrap().pending_autocommits, 1);
    assert!(matches!(repo.autocommit_all(&me()), Err(GitError::NothingToCommit)));

    // Edits, deletions and new files in one go.
    write(root, "f0.md", "changed");
    std::fs::remove_file(root.join("f1.md")).unwrap();
    write(root, "new/g.md", "g");
    let status = repo.status().unwrap();
    assert_eq!(status.changed_files, vec![PathBuf::from("f0.md"), PathBuf::from("f1.md"), PathBuf::from("new/g.md")]);
    let info = repo.autocommit_all(&me()).unwrap();
    assert_eq!(info.summary, "auto: f0.md, f1.md, new/g.md");
    assert_eq!(git(root, &["status", "--porcelain"]), "");
    git(root, &["fsck", "--strict"]);
}

#[cfg(unix)]
#[test]
fn autocommit_keeps_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = Repo::init(root, "main").unwrap();
    write(root, "run.sh", "#!/bin/sh\n");
    std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.autocommit(&[PathBuf::from("run.sh")], &me()).unwrap();
    assert!(git(root, &["ls-tree", "HEAD", "run.sh"]).starts_with("100755"));
}

#[test]
fn checkpoint_squashes_autocommits_and_tags() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = Repo::init(root, "main").unwrap();
    for i in 0..3 {
        write(root, "a.md", &format!("v{i}"));
        repo.autocommit_all(&me()).unwrap();
    }
    assert_eq!(repo.status().unwrap().pending_autocommits, 3);
    assert_eq!(repo.last_checkpoint().unwrap(), None);

    let first = repo.checkpoint("First draft", Some("v1"), &me()).unwrap();
    assert!(first.is_checkpoint);
    assert_eq!(first.summary, "First draft");
    assert_eq!(first.tag.as_deref(), Some("v1"));
    assert_eq!(repo.status().unwrap().pending_autocommits, 0);
    assert_eq!(repo.last_checkpoint().unwrap().as_deref(), Some(first.id.as_str()));
    assert_eq!(git(root, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(root, &["cat-file", "-t", "v1"]), "tag");
    assert_eq!(git(root, &["rev-parse", "v1^{commit}"]), first.id);
    assert_eq!(git(root, &["show", "HEAD:a.md"]), "v2");
    assert_eq!(repo.tags().unwrap(), vec!["v1".to_string()]);

    write(root, "a.md", "v3");
    repo.autocommit_all(&me()).unwrap();
    write(root, "b.md", "b");
    repo.autocommit_all(&me()).unwrap();
    // Uncommitted changes go into the checkpoint too.
    write(root, "c.md", "c");
    let second = repo.checkpoint("Second", None, &me()).unwrap();
    assert_eq!(git(root, &["rev-parse", "HEAD^"]), first.id);
    assert_eq!(git(root, &["show", "HEAD:c.md"]), "c");
    assert_eq!(git(root, &["status", "--porcelain"]), "");

    let history = repo.history(10).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].id, second.id);
    assert_eq!(history[0].tag, None);
    let folded: Vec<_> = history[0].autocommits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(folded, ["auto: c.md", "auto: b.md", "auto: a.md"]);
    assert_eq!(history[1].id, first.id);
    assert_eq!(history[1].tag.as_deref(), Some("v1"));
    assert_eq!(history[1].autocommits.len(), 3);
    let folded: Vec<_> = history[1].autocommits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(folded, ["auto: a.md", "auto: a.md", "auto: .gitignore, a.md"]);
    assert_eq!(repo.history(1).unwrap().len(), 1);

    // Nothing new: no checkpoint, but a tag alone is fine.
    assert!(matches!(repo.checkpoint("Again", None, &me()), Err(GitError::NothingToCommit)));
    let tagged = repo.checkpoint("Release", Some("v2"), &me()).unwrap();
    assert_eq!(tagged.id, second.id);
    assert_eq!(tagged.tag.as_deref(), Some("v2"));
    assert!(matches!(repo.checkpoint("Dup", Some("v2"), &me()), Err(GitError::TagExists(_))));
    git(root, &["fsck", "--strict"]);
}

#[test]
fn history_lists_pending_autocommits_on_top() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = repo_with_checkpoint(root);
    write(root, "a.md", "1");
    repo.autocommit_all(&me()).unwrap();
    write(root, "b.md", "2");
    repo.autocommit_all(&me()).unwrap();
    let history = repo.history(10).unwrap();
    let summaries: Vec<_> = history.iter().map(|c| (c.summary.as_str(), c.is_checkpoint)).collect();
    assert_eq!(summaries, [("auto: b.md", false), ("auto: a.md", false), ("base", true)]);
    assert_eq!(history[2].autocommits.len(), 1);
}

#[test]
fn branches_create_switch_rename_delete() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let repo = repo_with_checkpoint(root);

    repo.create_branch("draft").unwrap();
    assert!(matches!(repo.create_branch("draft"), Err(GitError::BranchExists(_))));
    assert_eq!(repo.branches().unwrap(), ["draft", "main"]);

    repo.switch_branch("draft").unwrap();
    assert_eq!(repo.current_branch().unwrap(), "draft");
    write(root, "a.md", "draft text");
    write(root, "dir/new.md", "new");
    repo.checkpoint("On draft", None, &me()).unwrap();

    repo.switch_branch("main").unwrap();
    assert_eq!(read(root, "a.md"), "# A\n");
    assert!(!root.join("dir").exists(), "empty directories are removed");
    assert_eq!(git(root, &["status", "--porcelain"]), "");

    // Dirty tracked files block a switch; so does an untracked file in the way.
    write(root, "a.md", "unsaved");
    assert!(matches!(repo.switch_branch("draft"), Err(GitError::Dirty)));
    write(root, "a.md", "# A\n");
    write(root, "dir/new.md", "mine");
    assert!(matches!(repo.switch_branch("draft"), Err(GitError::Dirty)));
    assert_eq!(read(root, "dir/new.md"), "mine");
    // The same content in the way is fine.
    write(root, "dir/new.md", "new");
    repo.switch_branch("draft").unwrap();
    assert_eq!(read(root, "a.md"), "draft text");
    assert_eq!(git(root, &["status", "--porcelain"]), "");

    // Rename the current branch (HEAD follows) and another one.
    repo.rename_branch("draft", "chapter-2").unwrap();
    assert_eq!(repo.current_branch().unwrap(), "chapter-2");
    assert_eq!(repo.branches().unwrap(), ["chapter-2", "main"]);
    repo.rename_branch("main", "trunk").unwrap();
    assert_eq!(repo.branches().unwrap(), ["chapter-2", "trunk"]);
    assert!(matches!(repo.rename_branch("nope", "x"), Err(GitError::BranchNotFound(_))));
    assert!(matches!(repo.rename_branch("trunk", "chapter-2"), Err(GitError::BranchExists(_))));

    assert!(matches!(repo.delete_branch("chapter-2"), Err(GitError::CurrentBranch(_))));
    assert!(matches!(repo.delete_branch("nope"), Err(GitError::BranchNotFound(_))));
    assert!(matches!(repo.switch_branch("nope"), Err(GitError::BranchNotFound(_))));
    repo.delete_branch("trunk").unwrap();
    assert_eq!(repo.branches().unwrap(), ["chapter-2"]);
    assert!(repo.create_branch("bad..name").is_err());
    git(root, &["fsck", "--strict"]);
}

#[test]
fn clone_fetch_and_fast_forward_pull() {
    let up_dir = tempfile::tempdir().unwrap();
    let upstream = repo_with_checkpoint(up_dir.path());
    let dir = tempfile::tempdir().unwrap();
    let local_root = dir.path().join("clone");

    let local = Repo::clone(up_dir.path().to_str().unwrap(), &local_root).unwrap();
    assert_eq!(read(&local_root, "a.md"), "# A\n");
    assert_eq!(local.current_branch().unwrap(), "main");
    assert_eq!(local.remote_for_branch().unwrap(), "origin");
    assert_eq!(local.history(10).unwrap().len(), 1);
    assert_eq!(local.pull_from("origin").unwrap(), PullOutcome::UpToDate);

    // Upstream moves on: fetch shows it as behind, pull fast-forwards.
    write(up_dir.path(), "a.md", "# A\nmore\n");
    write(up_dir.path(), "b.md", "b");
    upstream.checkpoint("More", Some("v1"), &me()).unwrap();
    local.fetch("origin").unwrap();
    let status = local.status().unwrap();
    assert_eq!((status.ahead, status.behind), (0, 1));
    assert_eq!(local.pull_from("origin").unwrap(), PullOutcome::FastForwarded);
    assert_eq!(read(&local_root, "a.md"), "# A\nmore\n");
    assert_eq!(read(&local_root, "b.md"), "b");
    assert_eq!(git(&local_root, &["status", "--porcelain"]), "");
    assert_eq!(local.tags().unwrap(), ["v1"]);
    assert_eq!(local.status().unwrap().behind, 0);

    // Local autocommits only: local is ahead, nothing to pull.
    write(&local_root, "c.md", "c");
    local.autocommit_all(&me()).unwrap();
    assert_eq!(local.pull_from("origin").unwrap(), PullOutcome::Ahead);
    assert_eq!(local.status().unwrap().ahead, 1);

    // Both sides moved: needs a merge, nothing changes locally.
    write(up_dir.path(), "a.md", "upstream edit");
    upstream.checkpoint("Upstream", None, &me()).unwrap();
    let before = git(&local_root, &["rev-parse", "HEAD"]);
    assert!(matches!(local.pull_from("origin"), Err(GitError::NeedsMerge)));
    assert!(matches!(local.pull(), Err(GitError::NeedsMerge)));
    assert_eq!(git(&local_root, &["rev-parse", "HEAD"]), before);
    assert_eq!(read(&local_root, "a.md"), "# A\nmore\n");
    assert!(matches!(local.fetch("nope"), Err(GitError::RemoteNotFound(_))));
}

#[test]
fn pull_refuses_dirty_tree() {
    let up_dir = tempfile::tempdir().unwrap();
    let upstream = repo_with_checkpoint(up_dir.path());
    let dir = tempfile::tempdir().unwrap();
    let local = Repo::clone(up_dir.path().to_str().unwrap(), dir.path()).unwrap();
    write(up_dir.path(), "a.md", "new");
    upstream.checkpoint("Change", None, &me()).unwrap();
    write(dir.path(), "a.md", "local unsaved");
    assert!(matches!(local.pull_from("origin"), Err(GitError::Dirty)));
    assert_eq!(read(dir.path(), "a.md"), "local unsaved");
    write(dir.path(), "a.md", "# A\n");
    local.pull().unwrap();
    assert_eq!(read(dir.path(), "a.md"), "new");
}

#[test]
fn add_remote_and_fetch() {
    let up_dir = tempfile::tempdir().unwrap();
    let upstream = repo_with_checkpoint(up_dir.path());
    let dir = tempfile::tempdir().unwrap();
    let local = Repo::init(dir.path(), "main").unwrap();
    local.add_remote("origin", up_dir.path().to_str().unwrap()).unwrap();
    local.fetch("origin").unwrap();
    // Unborn local branch: pull checks out the remote's commit. The local
    // `.gitignore` from init is identical to upstream's, so it is not in the way.
    assert_eq!(local.pull_from("origin").unwrap(), PullOutcome::FastForwarded);
    assert_eq!(read(dir.path(), "a.md"), "# A\n");
    assert_eq!(local.last_checkpoint().unwrap(), upstream.last_checkpoint().unwrap());
}
