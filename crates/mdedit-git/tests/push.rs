mod common;

#[cfg(not(feature = "push"))]
#[test]
fn push_is_unavailable_without_the_feature() {
    use mdedit_git::{GitError, push_available, push_backend};
    assert!(!push_available());
    let dir = tempfile::tempdir().unwrap();
    let repo = common::repo_with_checkpoint(dir.path());
    assert!(matches!(repo.push(push_backend().as_ref()), Err(GitError::PushUnavailable)));
}

#[cfg(feature = "push")]
mod with_push {
    use std::path::Path;
    use std::process::Command;

    use super::common::{git, me, repo_with_checkpoint, write};
    use mdedit_git::{GitError, Repo, push_available, push_backend};

    fn bare_remote(dir: &Path) -> String {
        let out = Command::new("git").args(["init", "--bare", "-q", "-b", "main"]).arg(dir).status().unwrap();
        assert!(out.success());
        dir.to_str().unwrap().to_owned()
    }

    fn has_object(repo: &Path, id: &str) -> bool {
        Command::new("git").arg("-C").arg(repo).args(["cat-file", "-e", id]).status().unwrap().success()
    }

    #[test]
    fn push_never_sends_autocommits() {
        assert!(push_available());
        let remote_dir = tempfile::tempdir().unwrap();
        let url = bare_remote(remote_dir.path());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = repo_with_checkpoint(root);
        repo.add_remote("origin", &url).unwrap();

        // Squashed autocommits (kept locally under the fold ref) and a
        // pending autocommit on top.
        let squashed = repo.history(1).unwrap()[0].autocommits[0].id.clone();
        write(root, "a.md", "edited");
        repo.autocommit_all(&me()).unwrap();
        write(root, "b.md", "b");
        repo.checkpoint("Chapter one", Some("v1"), &me()).unwrap();
        let checkpoint = repo.last_checkpoint().unwrap().unwrap();
        write(root, "a.md", "work in progress");
        let pending = repo.autocommit_all(&me()).unwrap().id;
        repo.tag("wip", "on an autocommit", &me()).unwrap();

        repo.push(push_backend().as_ref()).unwrap();

        let remote = remote_dir.path();
        assert_eq!(git(remote, &["rev-parse", "refs/heads/main"]), checkpoint);
        assert!(!has_object(remote, &pending), "pending autocommit was pushed");
        assert!(!has_object(remote, &squashed), "squashed autocommit was pushed");
        assert_eq!(git(remote, &["for-each-ref", "--format=%(refname)"]), "refs/heads/main\nrefs/tags/v1");
        assert_eq!(git(remote, &["rev-list", "--count", "main"]), "2");

        // Local tracking ref follows; the autocommit is still ahead.
        assert_eq!(git(root, &["rev-parse", "refs/remotes/origin/main"]), checkpoint);
        let status = repo.status().unwrap();
        assert_eq!((status.ahead, status.behind, status.pending_autocommits), (1, 0, 1));

        // A clone sees checkpoints only, with nothing folded under them.
        let clone_dir = tempfile::tempdir().unwrap();
        let clone = Repo::clone(&url, clone_dir.path()).unwrap();
        let history = clone.history(10).unwrap();
        assert_eq!(history.iter().map(|c| c.summary.as_str()).collect::<Vec<_>>(), ["Chapter one", "base"]);
        assert!(history.iter().all(|c| c.is_checkpoint && c.autocommits.is_empty()));
        assert_eq!(std::fs::read_to_string(clone_dir.path().join("a.md")).unwrap(), "edited");
    }

    #[test]
    fn push_needs_a_checkpoint_and_refuses_non_fast_forward() {
        let remote_dir = tempfile::tempdir().unwrap();
        let url = bare_remote(remote_dir.path());

        // Only autocommits: nothing pushable.
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path(), "main").unwrap();
        repo.add_remote("origin", &url).unwrap();
        write(dir.path(), "a.md", "a");
        repo.autocommit_all(&me()).unwrap();
        assert!(matches!(repo.push(push_backend().as_ref()), Err(GitError::NothingToCommit)));
        repo.checkpoint("first", None, &me()).unwrap();
        repo.push(push_backend().as_ref()).unwrap();

        // Someone else pushes a checkpoint; ours would not fast-forward.
        let other_dir = tempfile::tempdir().unwrap();
        let other = Repo::clone(&url, other_dir.path()).unwrap();
        write(other_dir.path(), "a.md", "theirs");
        other.checkpoint("theirs", None, &me()).unwrap();
        other.push(push_backend().as_ref()).unwrap();

        write(dir.path(), "a.md", "ours");
        repo.checkpoint("ours", None, &me()).unwrap();
        assert!(matches!(repo.push(push_backend().as_ref()), Err(GitError::NeedsMerge)));
        assert!(matches!(repo.pull(), Err(GitError::NeedsMerge)));

        let missing = mdedit_git::Git2Push::default();
        assert!(matches!(
            mdedit_git::PushBackend::push(&missing, dir.path(), "nope", "main"),
            Err(GitError::RemoteNotFound(_))
        ));
    }
}
