mod common;

use std::sync::mpsc;
use std::time::Duration;

use common::{me, repo_with_checkpoint, write};
use mdedit_git::{AutocommitEvent, AutocommitWorker};

#[test]
fn debounces_saves_into_one_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let repo = repo_with_checkpoint(&root);
    let (tx, rx) = mpsc::channel();
    let worker = AutocommitWorker::spawn(root.clone(), me(), Duration::from_millis(300), move |e| {
        tx.send(e).unwrap();
    });
    assert_eq!(worker.due_in(), None);

    // Saves within the debounce window restart the timer.
    for i in 0..3 {
        write(&root, &format!("n{i}.md"), "x");
        worker.saved();
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(rx.try_recv().is_err(), "no commit while saves keep coming");
    assert!(worker.due_in().is_some());
    match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        AutocommitEvent::Committed(info) => assert_eq!(info.summary, "auto: n0.md, n1.md, n2.md"),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(worker.due_in(), None);
    assert_eq!(repo.status().unwrap().pending_autocommits, 1);
}

#[test]
fn flush_and_drop_commit_pending_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let repo = repo_with_checkpoint(&root);
    let (tx, rx) = mpsc::channel();
    let worker = AutocommitWorker::spawn(root.clone(), me(), Duration::from_secs(3600), move |e| {
        tx.send(e).unwrap();
    });

    // Flush with nothing pending does nothing.
    worker.flush();
    assert!(rx.try_recv().is_err());

    write(&root, "a.md", "flushed");
    worker.saved();
    worker.flush();
    assert!(matches!(rx.try_recv().unwrap(), AutocommitEvent::Committed(_)));

    // A save that changed nothing reports so.
    worker.saved();
    worker.flush();
    assert_eq!(rx.try_recv().unwrap(), AutocommitEvent::NothingToCommit);

    // Quitting flushes.
    write(&root, "a.md", "on quit");
    worker.saved();
    drop(worker);
    assert!(matches!(rx.try_recv().unwrap(), AutocommitEvent::Committed(_)));
    assert_eq!(repo.status().unwrap().pending_autocommits, 2);
    assert!(repo.status().unwrap().changed_files.is_empty());
}

#[test]
fn set_debounce_shortens_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    repo_with_checkpoint(&root);
    let (tx, rx) = mpsc::channel();
    let worker = AutocommitWorker::spawn(root.clone(), me(), Duration::from_secs(3600), move |e| {
        tx.send(e).unwrap();
    });
    write(&root, "a.md", "changed");
    worker.saved();
    worker.set_debounce(Duration::from_millis(50));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), AutocommitEvent::Committed(_)));
}
