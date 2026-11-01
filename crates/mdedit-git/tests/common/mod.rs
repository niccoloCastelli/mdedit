#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use mdedit_git::{Author, Repo};

pub fn me() -> Author {
    Author { name: "Test".into(), email: "test@example.com".into() }
}

pub fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

/// Run the git CLI to cross-check what gix wrote.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A repo with one checkpoint ("base") holding `a.md`.
pub fn repo_with_checkpoint(root: &Path) -> Repo {
    let repo = Repo::init(root, "main").unwrap();
    write(root, "a.md", "# A\n");
    repo.autocommit_all(&me()).unwrap();
    repo.checkpoint("base", None, &me()).unwrap();
    repo
}
