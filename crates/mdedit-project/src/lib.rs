//! Project mode (SPEC 6.1, 6.2, 6.4; ARCHITECTURE 6).
//!
//! A project is a directory with a `.mdedit/` folder. This crate reads and
//! writes `project.toml`, lists the file tree lazily (hiding sidecars),
//! imports assets into `assets/` with SHA-256 dedupe, moves files with their
//! assets and sidecars while rewriting links, and watches the tree with
//! `notify` (debounced). Git lives in `mdedit-git`, comments in
//! `mdedit-sidecar`.

pub mod assets;
pub mod config;
pub mod links;
pub mod moving;
pub mod tree;
pub mod watch;

mod fsutil;

use std::path::{Path, PathBuf};

pub use config::ProjectConfig;
pub use moving::MoveReport;
pub use tree::{FileTree, TreeEntry};
pub use watch::{ProjectEvent, ProjectWatcher};

pub const PROJECT_DIR: &str = ".mdedit";
pub const PROJECT_FILE: &str = "project.toml";

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid project.toml: {0}")]
    Config(#[from] toml::de::Error),
    #[error("cannot write project.toml: {0}")]
    ConfigWrite(#[from] toml::ser::Error),
    #[error("not a project (no {PROJECT_DIR}/): {0}")]
    NotAProject(PathBuf),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("file watcher: {0}")]
    Watch(#[from] notify::Error),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

#[derive(Debug, Clone)]
pub struct Project {
    /// Absolute (not canonicalised) project directory.
    pub root: PathBuf,
    pub config: ProjectConfig,
}

impl Project {
    /// Walk up from `path` to the nearest directory containing `.mdedit/`.
    pub fn find_root(path: &Path) -> Option<PathBuf> {
        path.ancestors().find(|p| p.join(PROJECT_DIR).is_dir()).map(Path::to_path_buf)
    }

    /// True if `dir` contains a `.mdedit/` folder.
    pub fn is_project(dir: &Path) -> bool {
        dir.join(PROJECT_DIR).is_dir()
    }

    pub fn open(root: &Path) -> Result<Self, ProjectError> {
        let root = fsutil::absolute(root)?;
        let dir = root.join(PROJECT_DIR);
        if !dir.is_dir() {
            return Err(ProjectError::NotAProject(root));
        }
        let file = dir.join(PROJECT_FILE);
        let config =
            if file.exists() { toml::from_str(&std::fs::read_to_string(file)?)? } else { ProjectConfig::default() };
        Ok(Self { root, config })
    }

    /// Turn a directory into a project (creates `.mdedit/project.toml` and
    /// `.mdedit/.gitignore` ignoring `cache/`, SPEC 6.3). An existing project
    /// is opened unchanged. An empty `name` uses the directory name.
    pub fn init(root: &Path, name: &str) -> Result<Self, ProjectError> {
        let root = fsutil::absolute(root)?;
        let dir = root.join(PROJECT_DIR);
        if dir.join(PROJECT_FILE).exists() {
            return Self::open(&root);
        }
        std::fs::create_dir_all(&dir)?;
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(ignore, "cache/\n")?;
        }
        let mut config = ProjectConfig::default();
        config.project.name = if name.is_empty() {
            root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            name.to_owned()
        };
        let project = Self { root, config };
        project.save_config()?;
        Ok(project)
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(PROJECT_DIR).join(PROJECT_FILE)
    }

    /// Write `project.toml` atomically.
    pub fn save_config(&self) -> Result<(), ProjectError> {
        let text = toml::to_string_pretty(&self.config)?;
        fsutil::write_atomic(&self.config_path(), text.as_bytes())?;
        Ok(())
    }

    /// Re-read `project.toml` (after an external change).
    pub fn reload_config(&mut self) -> Result<(), ProjectError> {
        self.config = Self::open(&self.root)?.config;
        Ok(())
    }

    pub fn assets_dir(&self) -> &str {
        &self.config.files.assets_dir
    }

    /// Import an image for `md_file` (SPEC 6.4); returns the relative link.
    pub fn import_asset(&self, md_file: &Path, source: assets::AssetSource<'_>) -> Result<PathBuf, ProjectError> {
        assets::import(md_file, self.assets_dir(), source)
    }

    /// Rename or move a markdown file with its assets and sidecar, rewriting
    /// links (SPEC 6.4). See [`moving::move_markdown`].
    pub fn move_markdown(&self, from: &Path, to: &Path) -> Result<MoveReport, ProjectError> {
        moving::move_markdown(&self.root, from, to, self.assets_dir())
    }

    /// The lazy sidebar tree, with the root listed.
    pub fn file_tree(&self) -> std::io::Result<FileTree> {
        FileTree::new(&self.root)
    }

    /// Start the debounced watcher on the project directory.
    pub fn watch(
        &self,
        on_events: impl FnMut(Vec<ProjectEvent>) + Send + 'static,
    ) -> Result<ProjectWatcher, ProjectError> {
        ProjectWatcher::start(&self.root, watch::DEFAULT_DEBOUNCE, on_events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_project_and_open_reads_it() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!Project::is_project(tmp.path()));
        assert!(matches!(Project::open(tmp.path()), Err(ProjectError::NotAProject(_))));

        let p = Project::init(tmp.path(), "My notes").unwrap();
        assert!(Project::is_project(tmp.path()));
        assert!(p.config_path().is_file());
        assert_eq!(std::fs::read_to_string(tmp.path().join(".mdedit/.gitignore")).unwrap(), "cache/\n");

        let opened = Project::open(tmp.path()).unwrap();
        assert_eq!(opened.config.project.name, "My notes");
        assert_eq!(opened.config, p.config);
    }

    #[test]
    fn init_on_existing_project_keeps_config() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Project::init(tmp.path(), "First").unwrap();
        p.config.git.autocommit_debounce_secs = 5;
        p.save_config().unwrap();
        let again = Project::init(tmp.path(), "Second").unwrap();
        assert_eq!(again.config.project.name, "First");
        assert_eq!(again.config.git.autocommit_debounce_secs, 5);
    }

    #[test]
    fn init_without_name_uses_directory_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("my-notes");
        std::fs::create_dir(&root).unwrap();
        assert_eq!(Project::init(&root, "").unwrap().config.project.name, "my-notes");
    }

    #[test]
    fn find_root_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        Project::init(tmp.path(), "x").unwrap();
        let sub = tmp.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(Project::find_root(&sub.join("doc.md")).unwrap(), tmp.path());
    }

    #[test]
    fn missing_project_toml_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(PROJECT_DIR)).unwrap();
        let p = Project::open(tmp.path()).unwrap();
        assert_eq!(p.config, ProjectConfig::default());
    }

    #[test]
    fn bad_project_toml_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(PROJECT_DIR)).unwrap();
        std::fs::write(tmp.path().join(".mdedit/project.toml"), "[project\nname=").unwrap();
        assert!(matches!(Project::open(tmp.path()), Err(ProjectError::Config(_))));
    }

    #[test]
    fn save_config_keeps_unknown_tables() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(PROJECT_DIR)).unwrap();
        std::fs::write(tmp.path().join(".mdedit/project.toml"), "[project]\nname = \"N\"\n\n[future]\nflag = true\n")
            .unwrap();
        let mut p = Project::open(tmp.path()).unwrap();
        p.config.files.assets_dir = "img".into();
        p.save_config().unwrap();
        let text = std::fs::read_to_string(p.config_path()).unwrap();
        assert!(text.contains("[future]") && text.contains("flag = true"), "{text}");
        let mut again = Project::open(tmp.path()).unwrap();
        assert_eq!(again.assets_dir(), "img");
        std::fs::write(p.config_path(), "[files]\nassets_dir = \"pics\"\n").unwrap();
        again.reload_config().unwrap();
        assert_eq!(again.assets_dir(), "pics");
    }
}
