//! Push through libgit2 (ARCHITECTURE 7), only with the `push` feature.
//!
//! Only checkpoints are ever sent (SPEC 6.3): the branch is pushed as
//! `<last checkpoint>:refs/heads/<branch>`, so autocommits on top stay local,
//! and so do the `refs/mdedit/autocommits/*` fold refs. Tags are pushed when
//! they point at a commit that is being pushed.

use std::path::{Path, PathBuf};

use git2::{Cred, CredentialType, ErrorCode, Oid, PushOptions, RemoteCallbacks};

use crate::{GitError, PushBackend, Result, is_autocommit_message, other};

/// SSH key files tried after the SSH agent, in `~/.ssh`.
const SSH_KEYS: &[&str] = &["id_ed25519", "id_ecdsa", "id_rsa"];

#[derive(Debug, Clone, Default)]
pub struct Git2Push {
    /// Username and token for HTTPS (from the OS keychain), tried before
    /// the system's git credential helper.
    pub https_token: Option<(String, String)>,
}

impl PushBackend for Git2Push {
    fn push(&self, repo: &Path, remote: &str, branch: &str) -> Result<()> {
        let r = git2::Repository::open(repo).map_err(|_| GitError::NotARepo(repo.to_path_buf()))?;
        let tip =
            r.refname_to_id(&format!("refs/heads/{branch}")).map_err(|_| GitError::BranchNotFound(branch.into()))?;
        let checkpoint = last_checkpoint(&r, tip)?.ok_or(GitError::NothingToCommit)?;

        let mut specs = vec![format!("{checkpoint}:refs/heads/{branch}")];
        for name in r.tag_names(None).map_err(other)?.iter().flatten().flatten() {
            let Ok(target) = r.revparse_single(&format!("refs/tags/{name}")).and_then(|o| o.peel_to_commit()) else {
                continue;
            };
            let target = target.id();
            if target == checkpoint || r.graph_descendant_of(checkpoint, target).map_err(other)? {
                specs.push(format!("refs/tags/{name}:refs/tags/{name}"));
            }
        }

        let mut remote_obj = r.find_remote(remote).map_err(|_| GitError::RemoteNotFound(remote.into()))?;
        let mut rejected: Vec<String> = Vec::new();
        {
            let mut callbacks = RemoteCallbacks::new();
            callbacks.credentials(credentials(self.https_token.clone()));
            callbacks.push_update_reference(|name, status| {
                if let Some(msg) = status {
                    rejected.push(format!("{name}: {msg}"));
                }
                Ok(())
            });
            let mut opts = PushOptions::new();
            opts.remote_callbacks(callbacks);
            remote_obj.push(&specs, Some(&mut opts)).map_err(|e| match e.code() {
                ErrorCode::NotFastForward => GitError::NeedsMerge,
                _ => other(e.message()),
            })?;
        }
        if !rejected.is_empty() {
            let all = rejected.join("; ");
            return Err(if all.contains("fast-forward") || all.contains("fetch first") {
                GitError::NeedsMerge
            } else {
                other(all)
            });
        }
        r.reference(&format!("refs/remotes/{remote}/{branch}"), checkpoint, true, "push").map_err(other)?;
        Ok(())
    }
}

/// First commit on the first-parent chain from `tip` that is not an autocommit.
fn last_checkpoint(r: &git2::Repository, tip: Oid) -> Result<Option<Oid>> {
    let mut commit = r.find_commit(tip).map_err(other)?;
    loop {
        if !is_autocommit_message(&String::from_utf8_lossy(commit.message_bytes())) {
            return Ok(Some(commit.id()));
        }
        match commit.parent(0) {
            Ok(parent) => commit = parent,
            Err(_) => return Ok(None),
        }
    }
}

/// SSH agent, then key files in `~/.ssh`; for HTTPS the app's token, then
/// the system credential helper. Each is tried once.
fn credentials(
    token: Option<(String, String)>,
) -> impl FnMut(&str, Option<&str>, CredentialType) -> std::result::Result<Cred, git2::Error> {
    let mut tried_agent = false;
    let mut keys: Vec<PathBuf> = home()
        .map(|h| SSH_KEYS.iter().map(|k| h.join(".ssh").join(k)).filter(|p| p.exists()).collect())
        .unwrap_or_default();
    keys.reverse();
    let mut token = token;
    let mut tried_helper = false;
    let mut tried_default = false;
    move |url, username, allowed| {
        if allowed.contains(CredentialType::USERNAME) {
            return Cred::username(username.unwrap_or("git"));
        }
        if allowed.contains(CredentialType::SSH_KEY) {
            let user = username.unwrap_or("git");
            if !tried_agent {
                tried_agent = true;
                return Cred::ssh_key_from_agent(user);
            }
            if let Some(key) = keys.pop() {
                return Cred::ssh_key(user, None, &key, None);
            }
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
            if let Some((user, secret)) = token.take() {
                return Cred::userpass_plaintext(&user, &secret);
            }
            if !tried_helper {
                tried_helper = true;
                if let Ok(config) = git2::Config::open_default() {
                    return Cred::credential_helper(&config, url, username);
                }
            }
        }
        if allowed.contains(CredentialType::DEFAULT) && !tried_default {
            tried_default = true;
            return Cred::default();
        }
        Err(git2::Error::from_str("no usable credentials (SSH agent, ~/.ssh keys, token or credential helper)"))
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}
