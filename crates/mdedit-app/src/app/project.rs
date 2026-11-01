//! Project mode and git in the shell (SPEC 6; ARCHITECTURE 6, 7, 9).
//!
//! A folder opened as a project gets a file tree in the sidebar, autosave
//! on idle (DESIGN_SYSTEM 8.6), a debounced autocommit worker, checkpoints,
//! branches, sync and per-file history. Everything that touches the disk or
//! the repository runs off the UI thread; results come back as [`Msg`].
//! The rest of the shell only calls the hooks at the bottom of this file.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::futures::channel::mpsc;
use mdedit_git::{Author, AutocommitEvent, AutocommitWorker, CommitInfo, GitError, PullOutcome, Repo, RepoStatus};
use mdedit_project::{FileTree, Project, ProjectEvent, ProjectWatcher};

use super::*;

/// Autosave after this long without an edit (DESIGN_SYSTEM 8.6).
const AUTOSAVE_IDLE: Duration = Duration::from_secs(1);
const HISTORY_LIMIT: usize = 200;

pub struct ProjectState {
    pub project: Project,
    pub tree: FileTree,
    /// Selected tree row (target of rename, delete, and where new files go).
    pub selected: Option<PathBuf>,
    /// `None` when the folder is not a git repository.
    pub git: Option<GitView>,
    /// A git operation in flight, for the status bar ("syncing…").
    pub busy: Option<&'static str>,
    /// Last sync problem, shown in the Git panel until the next sync.
    pub notice: Option<String>,
    /// The last autocommit attempt committed (status bar "committed").
    pub committed: bool,
    pub history: Option<HistoryView>,
    author: Author,
    worker: Option<Arc<AutocommitWorker>>,
    _watcher: Option<ProjectWatcher>,
    /// Per tab: buffer version and when it was first seen (autosave idle).
    autosave_seen: HashMap<TabId, (u64, Instant)>,
    /// Branch to switch to once pending saves are done.
    pending_switch: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GitView {
    pub status: RepoStatus,
    pub branches: Vec<String>,
    pub history: Vec<CommitInfo>,
    /// Checkpoints whose autocommits are unfolded in the panel.
    pub expanded: HashSet<String>,
}

/// The per-file history and diff overlay.
#[derive(Debug, Clone)]
pub struct HistoryView {
    pub path: PathBuf,
    pub commits: Option<Vec<CommitInfo>>,
    pub selected: Option<usize>,
    pub diff: Option<Vec<DiffLine>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    Added,
    Removed,
    /// A run of unchanged lines left out; the text says how many.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// What opening a folder found, built off the UI thread.
pub struct Opened {
    project: Project,
    tree: FileTree,
    repo: Option<Author>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Opened(Result<Handoff<Opened>, String>),
    Click(PathBuf),
    Fs(Vec<ProjectEvent>),
    Autocommit(AutocommitEvent),
    Status(Result<GitView, String>),
    /// A git operation finished: its label and a message for the user.
    Done(&'static str, Result<String, String>),
    Prompt(PromptMsg),
    NewFile,
    NewFolder,
    Rename,
    Delete,
    InitGit,
    Checkpoint,
    Sync,
    NewBranch,
    Switch(String),
    RenameBranch(String),
    DeleteBranch(String),
    ToggleCommit(String),
    Tick,
    FileHistory,
    HistoryLoaded(PathBuf, Result<Vec<CommitInfo>, String>),
    PickVersion(usize),
    DiffReady(PathBuf, String, Result<Vec<DiffLine>, String>),
    Restore,
    CloseHistory,
    /// Images picked with Insert → Image, for this tab.
    InsertImages(TabId, Vec<PathBuf>),
    /// Ctrl+V: the clipboard's image as PNG, if it had one.
    PastedImage(TabId, Result<Option<Vec<u8>>, String>),
    /// A file dropped on the window.
    Dropped(PathBuf),
}

#[derive(Debug, Clone)]
pub enum PromptMsg {
    Input(String),
    Input2(String),
    Submit,
    Cancel,
}

/// A small dialog with up to two text fields.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub title: String,
    pub body: String,
    pub input: Option<String>,
    /// Second field: (label, value), e.g. the checkpoint's version tag.
    pub input2: Option<(&'static str, String)>,
    pub confirm: &'static str,
    pub danger: bool,
}

#[derive(Debug, Clone)]
pub enum PromptKind {
    MakeProject(PathBuf),
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Checkpoint { then_sync: bool },
    NewBranch,
    RenameBranch(String),
    DeleteBranch(String),
}

pub const PROMPT_INPUT: &str = "project-prompt";

impl App {
    pub(super) fn on_project(&mut self, msg: Msg) -> Task<Message> {
        match msg {
            Msg::Opened(Ok(opened)) => {
                if let Some(o) = opened.take() {
                    return self.project_opened(o);
                }
            }
            Msg::Opened(Err(e)) => self.toast_error(format!("Could not open the folder: {e}")),
            Msg::Click(path) => return self.tree_click(path),
            Msg::Fs(events) => {
                if let Some(p) = &mut self.project {
                    for e in &events {
                        let _ = p.tree.apply(e);
                    }
                }
            }
            Msg::Autocommit(event) => {
                let Some(p) = &mut self.project else { return Task::none() };
                match event {
                    AutocommitEvent::Committed(_) => p.committed = true,
                    AutocommitEvent::NothingToCommit => {}
                    AutocommitEvent::Failed(e) => {
                        p.committed = false;
                        self.toast_error(format!("Autocommit failed: {e}"));
                    }
                }
                return self.refresh_git();
            }
            Msg::Status(Ok(mut view)) => {
                if let Some(p) = &mut self.project {
                    if let Some(old) = &p.git {
                        view.expanded = old.expanded.clone();
                    }
                    p.git = Some(view);
                }
            }
            Msg::Status(Err(e)) => eprintln!("mdedit: git status: {e}"),
            Msg::Done(label, result) => return self.git_done(label, result),
            Msg::Prompt(m) => return self.on_prompt(m),
            Msg::NewFile => {
                self.ask_new(true);
                return focus_prompt();
            }
            Msg::NewFolder => {
                self.ask_new(false);
                return focus_prompt();
            }
            Msg::Rename => {
                self.ask_rename();
                return focus_prompt();
            }
            Msg::Delete => self.ask_delete(),
            Msg::InitGit => return self.init_git(),
            Msg::Checkpoint => return self.ask_checkpoint(false),
            Msg::Sync => return self.sync(),
            Msg::NewBranch => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::NewBranch,
                    title: "New branch".into(),
                    body: "Creates the branch from the current one and switches to it.".into(),
                    input: Some(String::new()),
                    input2: None,
                    confirm: "Create",
                    danger: false,
                });
                return focus_prompt();
            }
            Msg::Switch(name) => return self.switch_branch(name),
            Msg::RenameBranch(name) => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::RenameBranch(name.clone()),
                    title: format!("Rename branch {name}"),
                    body: String::new(),
                    input: Some(name),
                    input2: None,
                    confirm: "Rename",
                    danger: false,
                });
                return focus_prompt();
            }
            Msg::DeleteBranch(name) => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::DeleteBranch(name.clone()),
                    title: format!("Delete branch {name}?"),
                    body: "Its checkpoints are lost unless another branch contains them.".into(),
                    input: None,
                    input2: None,
                    confirm: "Delete",
                    danger: true,
                });
            }
            Msg::ToggleCommit(id) => {
                if let Some(g) = self.project.as_mut().and_then(|p| p.git.as_mut())
                    && !g.expanded.remove(&id)
                {
                    g.expanded.insert(id);
                }
            }
            Msg::Tick => return self.autosave_tick(),
            Msg::FileHistory => return self.open_file_history(),
            Msg::HistoryLoaded(path, result) => {
                let Some(h) = self.project.as_mut().and_then(|p| p.history.as_mut()) else { return Task::none() };
                if h.path != path {
                    return Task::none();
                }
                match result {
                    Ok(commits) => {
                        let first = !commits.is_empty();
                        h.commits = Some(commits);
                        if first {
                            return self.pick_version(0);
                        }
                    }
                    Err(e) => {
                        h.commits = Some(Vec::new());
                        self.toast_error(format!("Could not read the history: {e}"));
                    }
                }
            }
            Msg::PickVersion(i) => return self.pick_version(i),
            Msg::DiffReady(path, id, result) => {
                let Some(h) = self.project.as_mut().and_then(|p| p.history.as_mut()) else { return Task::none() };
                let current = h.selected.and_then(|i| h.commits.as_ref()?.get(i)).map(|c| c.id.clone());
                if h.path != path || current.as_deref() != Some(id.as_str()) {
                    return Task::none();
                }
                match result {
                    Ok(lines) => h.diff = Some(lines),
                    Err(e) => {
                        h.diff = Some(Vec::new());
                        self.toast_error(format!("Could not compare versions: {e}"));
                    }
                }
            }
            Msg::Restore => return self.restore_version(),
            Msg::InsertImages(id, paths) => return self.insert_images(id, paths),
            Msg::PastedImage(id, r) => return self.on_pasted_image(id, r),
            Msg::Dropped(path) => return self.on_dropped(path),
            Msg::CloseHistory => {
                if let Some(p) = &mut self.project {
                    p.history = None;
                }
            }
        }
        Task::none()
    }

    // ---------------------------------------------------------------- open

    /// Open `dir` as a project, or offer to make it one (SPEC 6).
    pub(super) fn open_folder(&mut self, dir: PathBuf) -> Task<Message> {
        if !Project::is_project(&dir) {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.prompt = Some(Prompt {
                kind: PromptKind::MakeProject(dir),
                title: format!("Make {name} a project?"),
                body: "This adds a .mdedit folder with the project settings and, if there is none yet, a git \
                       repository so every save is versioned."
                    .into(),
                input: None,
                input2: None,
                confirm: "Make project",
                danger: false,
            });
            return Task::none();
        }
        load_project(dir, false)
    }

    fn project_opened(&mut self, o: Opened) -> Task<Message> {
        self.close_project();
        let root = o.project.root.clone();
        let (tx, rx) = mpsc::unbounded();
        let watcher = match o.project.watch(move |events| {
            let _ = tx.unbounded_send(events);
        }) {
            Ok(w) => Some(w),
            Err(e) => {
                self.toast_error(format!("Not watching the folder for changes: {e}"));
                None
            }
        };
        let mut tasks = vec![Task::run(rx, |e| Message::Project(Msg::Fs(e)))];
        let has_git = o.repo.is_some();
        let author = o.repo.unwrap_or_else(|| Author { name: "mdedit".into(), email: "mdedit@localhost".into() });
        let git = &o.project.config.git;
        let worker = (has_git && git.autocommit).then(|| {
            let (tx, rx) = mpsc::unbounded();
            tasks.push(Task::run(rx, |e| Message::Project(Msg::Autocommit(e))));
            Arc::new(AutocommitWorker::spawn(
                root.clone(),
                author.clone(),
                Duration::from_secs(git.autocommit_debounce_secs),
                move |e| {
                    let _ = tx.unbounded_send(e);
                },
            ))
        });
        let pull = has_git && git.pull_on_open;
        self.project = Some(ProjectState {
            project: o.project,
            tree: o.tree,
            selected: None,
            git: has_git.then(GitView::default),
            busy: None,
            notice: None,
            committed: false,
            history: None,
            author,
            worker,
            _watcher: watcher,
            autosave_seen: HashMap::new(),
            pending_switch: None,
        });
        self.session.last_dir = Some(root);
        // Show the tree, unless the sidebar is already on the Git panel
        // (a restored session).
        if !(self.sidebar.visible && self.sidebar.tab == SidebarTab::Git) {
            self.sidebar.tab = SidebarTab::Files;
        }
        self.sidebar.visible = true;
        self.session_dirty = true;
        tasks.push(self.refresh_git());
        if pull {
            tasks.push(self.git_op("pull", |repo, _| match repo.pull_from(&repo.remote_for_branch()?) {
                Ok(PullOutcome::FastForwarded) => Ok("Pulled the latest changes.".into()),
                Ok(_) | Err(GitError::RemoteNotFound(_)) => Ok(String::new()),
                Err(GitError::Dirty) => Err(GitError::Other(
                    "There are uncommitted changes, so nothing was pulled. Sync manually when you are ready.".into(),
                )),
                Err(e) => Err(e),
            }));
        }
        Task::batch(tasks)
    }

    /// Flush the pending autocommit and forget the project.
    pub(super) fn close_project(&mut self) {
        if let Some(p) = self.project.take()
            && let Some(w) = &p.worker
        {
            w.flush();
        }
        self.session_dirty = true;
    }

    pub fn project_root(&self) -> Option<&Path> {
        self.project.as_ref().map(|p| p.project.root.as_path())
    }

    fn in_project(&self, path: &Path) -> bool {
        self.project_root().is_some_and(|r| path.starts_with(r))
    }

    // ---------------------------------------------------------------- tree

    fn tree_click(&mut self, path: PathBuf) -> Task<Message> {
        let Some(p) = &mut self.project else { return Task::none() };
        p.selected = Some(path.clone());
        if path.is_dir() {
            if let Err(e) = p.tree.toggle(&path) {
                self.toast_error(format!("Could not list {}: {e}", path.display()));
            }
            return Task::none();
        }
        if let Some(i) = self.tabs.iter().position(|t| t.doc.path.as_deref() == Some(&path)) {
            self.active = i;
            return self.focus_document();
        }
        Task::done(Message::OpenPicked(vec![path]))
    }

    /// Directory new files go to: the selected folder, the selected file's
    /// folder, or the root.
    fn target_dir(&self) -> Option<PathBuf> {
        let p = self.project.as_ref()?;
        Some(match &p.selected {
            Some(s) if s.is_dir() => s.clone(),
            Some(s) => s.parent().map(Path::to_path_buf).unwrap_or_else(|| p.project.root.clone()),
            None => p.project.root.clone(),
        })
    }

    fn ask_new(&mut self, file: bool) {
        let Some(dir) = self.target_dir() else { return };
        let rel = self.rel(&dir);
        let where_ = if rel.is_empty() { "the project root".to_string() } else { rel };
        self.prompt = Some(Prompt {
            kind: if file { PromptKind::NewFile(dir) } else { PromptKind::NewFolder(dir) },
            title: if file { "New file".into() } else { "New folder".into() },
            body: format!("In {where_}."),
            input: Some(if file { "untitled.md".into() } else { String::new() }),
            input2: None,
            confirm: "Create",
            danger: false,
        });
    }

    fn ask_rename(&mut self) {
        let Some(path) = self.project.as_ref().and_then(|p| p.selected.clone()) else {
            self.toast("Select a file or folder in the tree first.");
            return;
        };
        self.prompt = Some(Prompt {
            kind: PromptKind::Rename(path.clone()),
            title: "Rename or move".into(),
            body: "Path relative to the project. Markdown files take their images and comments along, and links \
                   to them are updated."
                .into(),
            input: Some(self.rel(&path)),
            input2: None,
            confirm: "Rename",
            danger: false,
        });
    }

    fn ask_delete(&mut self) {
        let Some(path) = self.project.as_ref().and_then(|p| p.selected.clone()) else {
            self.toast("Select a file or folder in the tree first.");
            return;
        };
        let rel = self.rel(&path);
        let body = if path.is_dir() {
            "The folder and everything in it are deleted. Committed versions stay in the git history."
        } else {
            "The file and its comments are deleted. Committed versions stay in the git history."
        };
        self.prompt = Some(Prompt {
            kind: PromptKind::Delete(path),
            title: format!("Delete {rel}?"),
            body: body.into(),
            input: None,
            input2: None,
            confirm: "Delete",
            danger: true,
        });
    }

    /// `path` relative to the project root, `/`-separated.
    fn rel(&self, path: &Path) -> String {
        let Some(root) = self.project_root() else { return path.display().to_string() };
        let rel = path.strip_prefix(root).unwrap_or(path);
        rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
    }

    fn on_prompt(&mut self, msg: PromptMsg) -> Task<Message> {
        let Some(prompt) = &mut self.prompt else { return Task::none() };
        match msg {
            PromptMsg::Input(v) => prompt.input = Some(v),
            PromptMsg::Input2(v) => {
                if let Some((_, value)) = &mut prompt.input2 {
                    *value = v;
                }
            }
            PromptMsg::Cancel => self.prompt = None,
            PromptMsg::Submit => {
                let prompt = self.prompt.take().expect("checked above");
                let value = prompt.input.as_deref().unwrap_or_default().trim().to_string();
                let value2 = prompt.input2.as_ref().map(|(_, v)| v.trim().to_string()).unwrap_or_default();
                return match self.submit_prompt(prompt.kind.clone(), value, value2) {
                    Ok(task) => task,
                    Err(e) => {
                        // Keep the dialog open so the input can be fixed.
                        self.toast_error(e);
                        self.prompt = Some(prompt);
                        Task::none()
                    }
                };
            }
        }
        Task::none()
    }

    fn submit_prompt(&mut self, kind: PromptKind, value: String, value2: String) -> Result<Task<Message>, String> {
        let need = |v: &str, what: &str| if v.is_empty() { Err(format!("Enter a {what}.")) } else { Ok(()) };
        match kind {
            PromptKind::MakeProject(dir) => Ok(load_project(dir, true)),
            PromptKind::NewFile(dir) => {
                need(&value, "file name")?;
                let mut name = value;
                if Path::new(&name).extension().is_none() {
                    name.push_str(".md");
                }
                let path = self.checked_path(&dir, &name)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
                self.after_fs_change(&path);
                Ok(Task::done(Message::OpenPicked(vec![path])))
            }
            PromptKind::NewFolder(dir) => {
                need(&value, "folder name")?;
                let path = self.checked_path(&dir, &value)?;
                if path.exists() {
                    return Err(format!("{value} already exists."));
                }
                std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
                self.after_fs_change(&path);
                Ok(Task::none())
            }
            PromptKind::Rename(from) => {
                need(&value, "path")?;
                let root = self.project_root().expect("prompt needs a project").to_path_buf();
                let to = self.checked_path(&root, &value)?;
                if to == from {
                    return Ok(Task::none());
                }
                self.move_path(&from, &to)?;
                Ok(Task::none())
            }
            PromptKind::Delete(path) => {
                self.delete_path(&path)?;
                Ok(Task::none())
            }
            PromptKind::Checkpoint { then_sync } => {
                need(&value, "checkpoint name")?;
                let tag = (!value2.is_empty()).then_some(value2);
                let label = if tag.is_some() { "Checkpoint and version tag saved." } else { "Checkpoint saved." };
                let op = if then_sync { "sync" } else { "checkpoint" };
                Ok(self.git_op(op, move |repo, me| {
                    repo.checkpoint(&value, tag.as_deref(), me)?;
                    if then_sync { sync_steps(repo) } else { Ok(label.into()) }
                }))
            }
            PromptKind::NewBranch => {
                need(&value, "branch name")?;
                if value.contains(char::is_whitespace) {
                    return Err("Branch names cannot contain spaces.".into());
                }
                let name = value.clone();
                Ok(self
                    .git_op("branch", move |repo, _| {
                        repo.create_branch(&name)?;
                        Ok(String::new())
                    })
                    .chain(Task::done(Message::Project(Msg::Switch(value)))))
            }
            PromptKind::RenameBranch(old) => {
                need(&value, "branch name")?;
                Ok(self.git_op("branch", move |repo, _| {
                    repo.rename_branch(&old, &value)?;
                    Ok(format!("Renamed {old} to {value}."))
                }))
            }
            PromptKind::DeleteBranch(name) => Ok(self.git_op("branch", move |repo, _| {
                repo.delete_branch(&name)?;
                Ok(format!("Deleted branch {name}."))
            })),
        }
    }

    /// `dir/name` if it stays inside the project.
    fn checked_path(&self, dir: &Path, name: &str) -> Result<PathBuf, String> {
        let root = self.project_root().ok_or("No project is open.")?;
        let mut out = dir.to_path_buf();
        for part in name.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => return Err("Paths must stay inside the project.".into()),
                p => out.push(p),
            }
        }
        if !out.starts_with(root) || out == root {
            return Err("Paths must stay inside the project.".into());
        }
        Ok(out)
    }

    fn after_fs_change(&mut self, path: &Path) {
        if let Some(p) = &mut self.project {
            if let Some(parent) = path.parent() {
                let _ = p.tree.refresh(parent);
                if parent != p.project.root && !p.tree.is_expanded(parent) {
                    let _ = p.tree.expand(parent);
                }
            }
            p.selected = Some(path.to_path_buf());
        }
    }

    fn move_path(&mut self, from: &Path, to: &Path) -> Result<(), String> {
        let open_dirty =
            self.tabs.iter().any(|t| t.doc.path.as_deref().is_some_and(|p| p.starts_with(from)) && t.is_dirty());
        if open_dirty {
            return Err("Save the open file first.".into());
        }
        if to.exists() {
            return Err(format!("{} already exists.", self.rel(to)));
        }
        let p = self.project.as_ref().ok_or("No project is open.")?;
        if from.is_file() && is_md(from) {
            p.project.move_markdown(from, to).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::rename(from, to).map_err(|e| e.to_string())?;
        }
        for tab in &mut self.tabs {
            if let Some(old) = tab.doc.path.clone()
                && let Ok(rest) = old.strip_prefix(from)
            {
                let new = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
                tab.disk = DiskStamp::read(&new);
                tab.doc.path = Some(new);
            }
        }
        if let Some(p) = &mut self.project {
            for dir in [from.parent(), to.parent()].into_iter().flatten() {
                let _ = p.tree.refresh(dir);
            }
        }
        self.after_fs_change(to);
        if let Some(w) = self.project.as_ref().and_then(|p| p.worker.as_ref()) {
            w.saved();
        }
        self.session_dirty = true;
        Ok(())
    }

    fn delete_path(&mut self, path: &Path) -> Result<(), String> {
        if path.is_dir() {
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
        } else {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
            let sidecar = mdedit_sidecar::sidecar_path(path);
            if sidecar.exists() {
                let _ = std::fs::remove_file(sidecar);
            }
        }
        while let Some(i) = self.tabs.iter().position(|t| t.doc.path.as_deref().is_some_and(|p| p.starts_with(path))) {
            self.close_tab_now(i);
        }
        if let Some(p) = &mut self.project {
            if let Some(parent) = path.parent() {
                let _ = p.tree.refresh(parent);
            }
            p.selected = None;
        }
        if let Some(w) = self.project.as_ref().and_then(|p| p.worker.as_ref()) {
            w.saved();
        }
        Ok(())
    }

    // ---------------------------------------------------------------- saving

    /// Save project files that have been idle for a second.
    fn autosave_tick(&mut self) -> Task<Message> {
        let Some(root) = self.project_root().map(Path::to_path_buf) else { return Task::none() };
        let now = Instant::now();
        let mut due = Vec::new();
        let mut seen = std::mem::take(&mut self.project.as_mut().expect("checked").autosave_seen);
        seen.retain(|id, _| self.tabs.iter().any(|t| t.id == *id && t.is_dirty()));
        for tab in &self.tabs {
            let in_project = tab.doc.path.as_deref().is_some_and(|p| p.starts_with(&root));
            if !in_project || !tab.is_dirty() || tab.saving || tab.read_only() || tab.external.is_some() {
                continue;
            }
            let v = tab.doc.buffer().version();
            match seen.get(&tab.id) {
                Some((sv, since)) if *sv == v => {
                    if now.duration_since(*since) >= AUTOSAVE_IDLE {
                        due.push(tab.id);
                    }
                }
                _ => {
                    seen.insert(tab.id, (v, now));
                }
            }
        }
        self.project.as_mut().expect("checked").autosave_seen = seen;
        Task::batch(due.into_iter().map(|id| self.start_save(id, None)).collect::<Vec<_>>())
    }

    /// Hook: a save finished.
    pub(super) fn project_saved(&mut self, path: &Path) -> Task<Message> {
        if !self.in_project(path) {
            return Task::none();
        }
        let Some(p) = &mut self.project else { return Task::none() };
        p.committed = false;
        if let Some(w) = &p.worker {
            w.saved();
        }
        if p.pending_switch.is_some() && !self.project_tabs_unsaved() {
            let name = self.project.as_mut().and_then(|p| p.pending_switch.take()).expect("checked");
            return self.switch_branch(name);
        }
        self.refresh_git()
    }

    fn project_tabs_unsaved(&self) -> bool {
        self.tabs.iter().any(|t| {
            t.doc.path.as_deref().is_some_and(|p| self.in_project(p)) && (t.is_dirty() || t.saving) && !t.read_only()
        })
    }

    // ---------------------------------------------------------------- git

    fn refresh_git(&self) -> Task<Message> {
        let Some(p) = &self.project else { return Task::none() };
        if p.git.is_none() {
            return Task::none();
        }
        let root = p.project.root.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || -> Result<GitView, String> {
                    let repo = Repo::open(&root).map_err(|e| e.to_string())?;
                    Ok(GitView {
                        status: repo.status().map_err(|e| e.to_string())?,
                        branches: repo.branches().map_err(|e| e.to_string())?,
                        history: repo.history(HISTORY_LIMIT).map_err(|e| e.to_string())?,
                        expanded: HashSet::new(),
                    })
                })
                .await
                .map_err(|e| e.to_string())?
            },
            |r| Message::Project(Msg::Status(r)),
        )
    }

    /// Run `op` on a blocking thread after flushing the pending autocommit.
    fn git_op(
        &mut self,
        label: &'static str,
        op: impl FnOnce(&Repo, &Author) -> mdedit_git::Result<String> + Send + 'static,
    ) -> Task<Message> {
        let Some(p) = &mut self.project else { return Task::none() };
        p.busy = Some(label);
        let root = p.project.root.clone();
        let author = p.author.clone();
        let worker = p.worker.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if let Some(w) = worker {
                        w.flush();
                    }
                    let repo = Repo::open(&root).map_err(|e| e.to_string())?;
                    op(&repo, &author).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |r| Message::Project(Msg::Done(label, r)),
        )
    }

    fn git_done(&mut self, label: &'static str, result: Result<String, String>) -> Task<Message> {
        let Some(p) = &mut self.project else { return Task::none() };
        p.busy = None;
        if matches!(label, "sync" | "pull") {
            p.notice = result.as_ref().err().cloned();
        }
        if matches!(label, "switch" | "pull" | "sync" | "restore") {
            let _ = p.tree.apply(&ProjectEvent::Rescan);
        }
        match result {
            Ok(msg) if !msg.is_empty() => self.toast(msg),
            Ok(_) => {}
            Err(e) => self.toast_error(e),
        }
        if label == "init" {
            return self.reopen_project();
        }
        let mut tasks = vec![self.refresh_git()];
        // Branch switches and pulls change files under open tabs.
        if matches!(label, "switch" | "pull" | "sync" | "restore") {
            let paths: Vec<PathBuf> = self.tabs.iter().filter_map(|t| t.doc.path.clone()).collect();
            tasks.extend(paths.into_iter().map(|p| Task::done(Message::FileChanged(p))));
        }
        if label == "restore"
            && let Some(h) = self.project.as_ref().and_then(|p| p.history.as_ref())
        {
            let path = h.path.clone();
            tasks.push(self.load_file_history(path));
        }
        Task::batch(tasks)
    }

    fn reopen_project(&mut self) -> Task<Message> {
        let Some(root) = self.project_root().map(Path::to_path_buf) else { return Task::none() };
        load_project(root, false)
    }

    fn init_git(&mut self) -> Task<Message> {
        let Some(p) = &self.project else { return Task::none() };
        let branch = p.project.config.project.default_branch.clone();
        let root = p.project.root.clone();
        self.project.as_mut().expect("checked").busy = Some("init");
        Task::perform(
            async move { tokio::task::spawn_blocking(move || init_repo(&root, &branch)).await.map_err(|e| e.to_string())? },
            |r| Message::Project(Msg::Done("init", r)),
        )
    }

    fn ask_checkpoint(&mut self, then_sync: bool) -> Task<Message> {
        let Some(p) = &self.project else {
            self.toast("Checkpoints need a project: open a folder first.");
            return Task::none();
        };
        if p.git.is_none() {
            self.toast("This project has no git repository yet (Git panel → Initialize).");
            return Task::none();
        }
        let default = if then_sync { format!("Sync {}", now_label()) } else { String::new() };
        self.prompt = Some(Prompt {
            kind: PromptKind::Checkpoint { then_sync },
            title: if then_sync { "Checkpoint before syncing".into() } else { "New checkpoint".into() },
            body: if then_sync {
                "Autocommits stay local, so they are squashed into a checkpoint before syncing.".into()
            } else {
                "Squashes the autocommits since the last checkpoint into one named commit.".into()
            },
            input: Some(default),
            input2: Some(("Version tag (optional, e.g. v1.2)", String::new())),
            confirm: "Save checkpoint",
            danger: false,
        });
        focus_prompt()
    }

    /// Checkpoint if needed, pull (fast-forward only), push if available.
    fn sync(&mut self) -> Task<Message> {
        let Some(p) = &self.project else {
            self.toast("Sync needs a project: open a folder first.");
            return Task::none();
        };
        let Some(g) = &p.git else {
            self.toast("This project has no git repository yet (Git panel → Initialize).");
            return Task::none();
        };
        if p.busy.is_some() {
            return Task::none();
        }
        if self.project_tabs_unsaved() {
            let ids: Vec<TabId> = self
                .tabs
                .iter()
                .filter(|t| t.doc.path.as_deref().is_some_and(|p| self.in_project(p)) && t.is_dirty())
                .map(|t| t.id)
                .collect();
            let saves: Vec<_> = ids.into_iter().map(|id| self.start_save(id, None)).collect();
            self.toast("Saving first; press Sync again in a moment.");
            return Task::batch(saves);
        }
        if g.status.pending_autocommits > 0 || !g.status.changed_files.is_empty() {
            return self.ask_checkpoint(true);
        }
        self.git_op("sync", |repo, _| sync_steps(repo))
    }

    fn switch_branch(&mut self, name: String) -> Task<Message> {
        if self.project_tabs_unsaved() {
            let ids: Vec<TabId> = self
                .tabs
                .iter()
                .filter(|t| t.doc.path.as_deref().is_some_and(|p| self.in_project(p)) && t.is_dirty() && !t.saving)
                .map(|t| t.id)
                .collect();
            if let Some(p) = &mut self.project {
                p.pending_switch = Some(name);
            }
            return Task::batch(ids.into_iter().map(|id| self.start_save(id, None)).collect::<Vec<_>>());
        }
        self.git_op("switch", move |repo, me| {
            match repo.autocommit_all(me) {
                Ok(_) | Err(GitError::NothingToCommit) => {}
                Err(e) => return Err(e),
            }
            repo.switch_branch(&name)?;
            Ok(format!("Switched to {name}."))
        })
    }

    // ---------------------------------------------------------------- history

    fn open_file_history(&mut self) -> Task<Message> {
        let Some(path) = self.active_tab().and_then(|t| t.doc.path.clone()) else {
            self.toast("Open a saved project file to see its history.");
            return Task::none();
        };
        let Some(p) = &mut self.project else {
            self.toast("History needs a project: open a folder first.");
            return Task::none();
        };
        if p.git.is_none() || !path.starts_with(&p.project.root) {
            self.toast("This file is not in the project's git repository.");
            return Task::none();
        }
        p.history = Some(HistoryView { path: path.clone(), commits: None, selected: None, diff: None });
        self.load_file_history(path)
    }

    fn load_file_history(&self, path: PathBuf) -> Task<Message> {
        let Some(p) = &self.project else { return Task::none() };
        let root = p.project.root.clone();
        let worker = p.worker.clone();
        let for_path = path.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    if let Some(w) = worker {
                        w.flush();
                    }
                    let repo = Repo::open(&root).map_err(|e| e.to_string())?;
                    repo.file_history(&path, HISTORY_LIMIT).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |r| Message::Project(Msg::HistoryLoaded(for_path.clone(), r)),
        )
    }

    fn pick_version(&mut self, i: usize) -> Task<Message> {
        let current = self.history_current_text();
        let Some(p) = &mut self.project else { return Task::none() };
        let root = p.project.root.clone();
        let Some(h) = &mut p.history else { return Task::none() };
        let Some(commit) = h.commits.as_ref().and_then(|c| c.get(i)) else { return Task::none() };
        h.selected = Some(i);
        h.diff = None;
        let (path, id) = (h.path.clone(), commit.id.clone());
        let (for_path, for_id) = (path.clone(), id.clone());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let repo = Repo::open(&root).map_err(|e| e.to_string())?;
                    let old = repo.file_at(&id, &path).map_err(|e| e.to_string())?.unwrap_or_default();
                    Ok(diff_lines(&String::from_utf8_lossy(&old), &current))
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |r| Message::Project(Msg::DiffReady(for_path.clone(), for_id.clone(), r)),
        )
    }

    /// The file as the user sees it now: the open buffer, else the disk.
    fn history_current_text(&self) -> String {
        let Some(path) = self.project.as_ref().and_then(|p| p.history.as_ref()).map(|h| h.path.clone()) else {
            return String::new();
        };
        match self.tabs.iter().find(|t| t.doc.path.as_deref() == Some(&path)) {
            Some(t) => {
                let buf = t.doc.buffer();
                buf.slice(0..buf.len_bytes()).into_owned()
            }
            None => std::fs::read_to_string(&path).unwrap_or_default(),
        }
    }

    /// Write the selected version back as a new commit (never rewrites).
    fn restore_version(&mut self) -> Task<Message> {
        let Some(h) = self.project.as_ref().and_then(|p| p.history.as_ref()) else { return Task::none() };
        let Some(commit) = h.selected.and_then(|i| h.commits.as_ref()?.get(i)).cloned() else {
            return Task::none();
        };
        let path = h.path.clone();
        if self.tabs.iter().any(|t| t.doc.path.as_deref() == Some(&path) && (t.is_dirty() || t.saving)) {
            self.toast("Wait for the file to save, then restore.");
            return Task::none();
        }
        let when = time_label(commit.time_unix);
        self.git_op("restore", move |repo, me| {
            match repo.file_at(&commit.id, &path)? {
                Some(bytes) => std::fs::write(&path, bytes)?,
                None => std::fs::remove_file(&path)?,
            }
            match repo.autocommit(std::slice::from_ref(&path), me) {
                Ok(_) | Err(GitError::NothingToCommit) => {}
                Err(e) => return Err(e),
            }
            Ok(format!("Restored the version from {when}."))
        })
    }

    // ---------------------------------------------------------------- hooks

    /// Hook: project commands (menus, palette, keymap).
    pub(super) fn project_command(&mut self, cmd: AppCommand) -> Task<Message> {
        use AppCommand::*;
        let needs_project = !matches!(cmd, OpenFolder);
        if needs_project && self.project.is_none() {
            self.toast("Open a folder as a project first (File → Open Folder…).");
            return Task::none();
        }
        match cmd {
            OpenFolder => {
                let dir = self.session.last_dir.clone();
                Task::perform(
                    async move {
                        let mut d = rfd::AsyncFileDialog::new();
                        if let Some(dir) = dir {
                            d = d.set_directory(dir);
                        }
                        d.pick_folder().await.map(|h| h.path().to_path_buf())
                    },
                    |dir| match dir {
                        Some(dir) => Message::OpenFolder(dir),
                        None => Message::Tick,
                    },
                )
            }
            CloseFolder => {
                self.close_project();
                Task::none()
            }
            ProjectNewFile => self.on_project(Msg::NewFile),
            ProjectNewFolder => self.on_project(Msg::NewFolder),
            Checkpoint => self.ask_checkpoint(false),
            Sync => self.sync(),
            NewBranch => self.on_project(Msg::NewBranch),
            FileHistory => self.open_file_history(),
            _ => Task::none(),
        }
    }

    /// Hook: whether a project command can run (menus grey out the rest).
    pub fn project_command_enabled(&self, id: &str) -> Option<bool> {
        match id {
            "file.open_folder" => Some(true),
            "file.close_folder" | "project.new_file" | "project.new_folder" => Some(self.project.is_some()),
            id if id.starts_with("git.") => Some(self.project.as_ref().is_some_and(|p| p.git.is_some())),
            _ => None,
        }
    }

    /// Hook: subscriptions while a project is open.
    pub(super) fn project_subscriptions(&self) -> Option<Subscription<Message>> {
        let p = self.project.as_ref()?;
        let editing =
            self.tabs.iter().any(|t| t.is_dirty() && t.doc.path.as_deref().is_some_and(|x| self.in_project(x)));
        let counting = p.worker.as_ref().is_some_and(|w| w.due_in().is_some());
        let every = if editing {
            Duration::from_millis(250)
        } else if counting {
            Duration::from_secs(1)
        } else {
            return None;
        };
        Some(iced::time::every(every).map(|_| Message::Project(Msg::Tick)))
    }

    /// Hook: on quit, commit what is pending.
    pub(super) fn project_flush(&self) {
        if let Some(w) = self.project.as_ref().and_then(|p| p.worker.as_ref()) {
            w.flush();
        }
    }

    /// Seconds until the pending autocommit, for the status bar.
    pub fn autocommit_due(&self) -> Option<u64> {
        let w = self.project.as_ref()?.worker.as_ref()?;
        w.due_in().map(|d| d.as_secs_f32().ceil() as u64)
    }
}

/// Pull (fast-forward only), then push when this build can.
fn sync_steps(repo: &Repo) -> mdedit_git::Result<String> {
    let remote = repo.remote_for_branch()?;
    let pulled = match repo.pull_from(&remote) {
        Ok(o) => o,
        Err(GitError::RemoteNotFound(r)) => {
            return Err(GitError::Other(format!(
                "No remote named {r}. Add one with `git remote add {r} <url>` in the project folder."
            )));
        }
        Err(GitError::NeedsMerge) => {
            return Err(GitError::Other(
                "The remote and this copy both have new checkpoints. Merging is not supported yet; \
                 sync with git outside mdedit."
                    .into(),
            ));
        }
        Err(e) => return Err(e),
    };
    if !mdedit_git::push_available() {
        return Ok(match pulled {
            PullOutcome::FastForwarded => "Pulled. Pushing is not available in this build.".into(),
            _ => "Up to date. Pushing is not available in this build.".into(),
        });
    }
    repo.push(mdedit_git::push_backend().as_ref())?;
    Ok(match pulled {
        PullOutcome::FastForwarded => "Pulled and pushed.".into(),
        _ => "Synced.".into(),
    })
}

/// Focus the dialog's first field with its text selected.
fn focus_prompt() -> Task<Message> {
    Task::batch([iced::widget::operation::focus(PROMPT_INPUT), iced::widget::operation::select_all(PROMPT_INPUT)])
}

/// Open (or with `create`, first make) a project off the UI thread.
fn load_project(dir: PathBuf, create: bool) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || -> Result<Handoff<Opened>, String> {
                let project = if create {
                    let p = Project::init(&dir, "").map_err(|e| e.to_string())?;
                    if !dir.join(".git").exists() {
                        init_repo(&dir, &p.config.project.default_branch)?;
                    }
                    p
                } else {
                    Project::open(&dir).map_err(|e| e.to_string())?
                };
                let tree = project.file_tree().map_err(|e| e.to_string())?;
                let repo = Repo::open(&project.root).ok().map(|r| {
                    let fallback = r.default_author();
                    let git = &project.config.git;
                    Author {
                        name: git.author_name.clone().filter(|s| !s.is_empty()).unwrap_or(fallback.name),
                        email: git.author_email.clone().filter(|s| !s.is_empty()).unwrap_or(fallback.email),
                    }
                });
                Ok(Handoff::new(Opened { project, tree, repo }))
            })
            .await
            .map_err(|e| e.to_string())?
        },
        |r| Message::Project(Msg::Opened(r)),
    )
}

/// `git init` plus a first commit of what is already there.
fn init_repo(root: &Path, branch: &str) -> Result<String, String> {
    let repo = Repo::init(root, branch).map_err(|e| e.to_string())?;
    match repo.autocommit_all(&repo.default_author()) {
        Ok(_) | Err(GitError::NothingToCommit) => Ok("Versioning is on: saves are committed automatically.".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn is_md(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown" | "mkd"))
}

/// Lines from `old` to `new`, with long unchanged runs folded.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    const CONTEXT: usize = 3;
    let diff = similar::TextDiff::from_lines(old, new);
    let mut all = Vec::new();
    for change in diff.iter_all_changes() {
        let kind = match change.tag() {
            similar::ChangeTag::Equal => DiffKind::Same,
            similar::ChangeTag::Insert => DiffKind::Added,
            similar::ChangeTag::Delete => DiffKind::Removed,
        };
        all.push(DiffLine { kind, text: change.value().trim_end_matches(['\n', '\r']).to_string() });
    }
    let changed: Vec<usize> =
        all.iter().enumerate().filter(|(_, l)| l.kind != DiffKind::Same).map(|(i, _)| i).collect();
    let near = |i: usize| changed.iter().any(|&c| c.abs_diff(i) <= CONTEXT);
    let mut out = Vec::new();
    let mut skipped = 0;
    for (i, line) in all.into_iter().enumerate() {
        if line.kind == DiffKind::Same && !near(i) {
            skipped += 1;
            continue;
        }
        if skipped > 0 {
            out.push(DiffLine { kind: DiffKind::Skipped, text: format!("{skipped} unchanged lines") });
            skipped = 0;
        }
        out.push(line);
    }
    if skipped > 0 {
        out.push(DiffLine { kind: DiffKind::Skipped, text: format!("{skipped} unchanged lines") });
    }
    out
}

fn now_label() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M").to_string()
}

/// A commit time for lists: "2026-11-01 15:30".
pub fn time_label(unix: i64) -> String {
    jiff::Timestamp::from_second(unix)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_folds_unchanged_runs() {
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 10\n", "line ten\n");
        let d = diff_lines(&old, &new);
        let kinds: Vec<DiffKind> = d.iter().map(|l| l.kind).collect();
        assert_eq!(kinds.first(), Some(&DiffKind::Skipped));
        assert_eq!(d.iter().filter(|l| l.kind == DiffKind::Removed).count(), 1);
        assert_eq!(d.iter().find(|l| l.kind == DiffKind::Added).unwrap().text, "line ten");
        assert_eq!(d[0].text, "6 unchanged lines");
        assert_eq!(kinds.last(), Some(&DiffKind::Skipped));
    }

    #[test]
    fn identical_is_one_fold() {
        let d = diff_lines("a\nb\n", "a\nb\n");
        assert_eq!(d, vec![DiffLine { kind: DiffKind::Skipped, text: "2 unchanged lines".into() }]);
    }
}
