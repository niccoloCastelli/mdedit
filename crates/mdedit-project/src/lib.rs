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
pub mod tree;

use std::path::{Path, PathBuf};

pub use config::ProjectConfig;

pub const PROJECT_DIR: &str = ".mdedit";
pub const PROJECT_FILE: &str = "project.toml";

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid project.toml: {0}")]
    Config(#[from] toml::de::Error),
    #[error("not a project (no {PROJECT_DIR}/): {0}")]
    NotAProject(PathBuf),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub config: ProjectConfig,
}

impl Project {
    /// Walk up from `path` to the nearest directory containing `.mdedit/`.
    pub fn find_root(path: &Path) -> Option<PathBuf> {
        path.ancestors().find(|p| p.join(PROJECT_DIR).is_dir()).map(Path::to_path_buf)
    }

    pub fn open(root: &Path) -> Result<Self, ProjectError> {
        let dir = root.join(PROJECT_DIR);
        if !dir.is_dir() {
            return Err(ProjectError::NotAProject(root.to_path_buf()));
        }
        let file = dir.join(PROJECT_FILE);
        let config =
            if file.exists() { toml::from_str(&std::fs::read_to_string(file)?)? } else { ProjectConfig::default() };
        Ok(Self { root: root.to_path_buf(), config })
    }

    /// Turn a directory into a project (creates `.mdedit/project.toml`).
    pub fn init(root: &Path, name: &str) -> Result<Self, ProjectError> {
        let _ = (root, name);
        Err(ProjectError::Unimplemented("Project::init"))
    }
}
