//! Working-tree and index plumbing that `gix` has no high-level call for:
//! moving an existing checkout from one tree to another (branch switch,
//! fast-forward) and rewriting the index after a commit made from files on
//! disk, keeping stat data so status stays fast.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use gix::ObjectId;
use gix::bstr::{BStr, BString};
use gix::index::entry::{Mode, Stat};

use crate::{GitError, Result, other};

type Flat = BTreeMap<BString, (ObjectId, Mode)>;

fn flatten(repo: &gix::Repository, tree: ObjectId) -> Result<Flat> {
    let index = repo.index_from_tree(&tree)?;
    Ok(index.entries().iter().map(|e| (e.path(&index).to_owned(), (e.id, e.mode))).collect())
}

pub(crate) fn fs_path(root: &Path, rela: &BStr) -> Result<PathBuf> {
    let rela = gix::path::from_bstr(rela).map_err(other)?;
    Ok(root.join(rela))
}

pub(crate) fn stat_of(path: &Path) -> Result<Stat> {
    let meta = gix::index::fs::Metadata::from_path_no_follow(path)?;
    Stat::from_fs(&meta).map_err(other)
}

/// Does the file at `path` hold exactly blob `id`?
fn holds_blob(repo: &gix::Repository, path: &Path, id: ObjectId) -> Result<bool> {
    let meta = std::fs::symlink_metadata(path)?;
    let data = if meta.file_type().is_symlink() {
        gix::path::os_string_into_bstring(std::fs::read_link(path)?.into_os_string()).map_err(other)?.into()
    } else if meta.is_file() {
        std::fs::read(path)?
    } else {
        return Ok(false);
    };
    let actual = gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Blob, &data).map_err(other)?;
    Ok(actual == id)
}

/// Move the working tree from `from` to `to`, then rewrite the index for
/// `to`. The caller has checked that tracked files are clean; untracked
/// files that would be overwritten make this fail with `Dirty` before
/// anything is touched.
pub(crate) fn checkout(repo: &gix::Repository, root: &Path, from: ObjectId, to: ObjectId) -> Result<()> {
    let old = flatten(repo, from)?;
    let new = flatten(repo, to)?;

    for (path, (id, _)) in &new {
        if old.contains_key(path) {
            continue;
        }
        let fs = fs_path(root, path.as_ref())?;
        if std::fs::symlink_metadata(&fs).is_ok() && !holds_blob(repo, &fs, *id)? {
            return Err(GitError::Dirty);
        }
    }

    for path in old.keys().filter(|p| !new.contains_key(*p)) {
        let fs = fs_path(root, path.as_ref())?;
        match std::fs::remove_file(&fs) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        prune_empty_dirs(root, &fs);
    }

    let mut fresh = HashMap::new();
    for (path, entry) in &new {
        if old.get(path) == Some(entry) {
            continue;
        }
        let (id, mode) = *entry;
        if mode == Mode::COMMIT {
            continue; // submodules are not checked out
        }
        let fs = fs_path(root, path.as_ref())?;
        write_entry(repo, &fs, id, mode)?;
        fresh.insert(path.clone(), stat_of(&fs)?);
    }
    write_index(repo, to, &fresh)
}

fn write_entry(repo: &gix::Repository, fs: &Path, id: ObjectId, mode: Mode) -> Result<()> {
    if let Some(parent) = fs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if fs.is_dir() && !fs.is_symlink() {
        return Err(GitError::Dirty);
    }
    let blob = repo.find_object(id)?.detach();
    if std::fs::symlink_metadata(fs).is_ok() {
        std::fs::remove_file(fs)?;
    }
    if mode == Mode::SYMLINK {
        #[cfg(unix)]
        {
            let target = gix::path::from_byte_slice(&blob.data).map_err(other)?;
            std::os::unix::fs::symlink(target, fs)?;
            return Ok(());
        }
    }
    std::fs::write(fs, &blob.data)?;
    #[cfg(unix)]
    if mode == Mode::FILE_EXECUTABLE {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(fs)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(fs, perms)?;
    }
    Ok(())
}

fn prune_empty_dirs(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// Replace the index with the entries of `tree`. Stat data comes from
/// `fresh` (files just written or committed, stat taken before reading
/// them), else from the old index when the entry is unchanged, else it is
/// left empty so status compares content.
pub(crate) fn write_index(repo: &gix::Repository, tree: ObjectId, fresh: &HashMap<BString, Stat>) -> Result<()> {
    let mut index = repo.index_from_tree(&tree)?;
    let old = repo.try_index()?;
    let stats: Vec<Option<Stat>> = index
        .entries()
        .iter()
        .map(|e| {
            let path = e.path(&index);
            if let Some(stat) = fresh.get(path) {
                return Some(*stat);
            }
            let old = old.as_ref()?;
            let prev = old.entry_by_path(path)?;
            (prev.id == e.id && prev.mode == e.mode).then_some(prev.stat)
        })
        .collect();
    for (entry, stat) in index.entries_mut().iter_mut().zip(stats) {
        if let Some(stat) = stat {
            entry.stat = stat;
        }
    }
    index.write(gix::index::write::Options::default()).map_err(other)?;
    Ok(())
}
