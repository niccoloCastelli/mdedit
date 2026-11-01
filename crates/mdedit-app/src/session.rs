//! Session restore (SPEC 5): open tabs, cursor positions, scroll and panel
//! layout, saved to `session.toml` in the state dir on quit and whenever the
//! set of tabs changes. Untitled buffers are not restored.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::{Mode, write_atomic};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<TabSession>,
    pub active: usize,
    pub sidebar_visible: bool,
    pub sidebar_tab: String,
    pub sidebar_width: f32,
    pub comments_visible: bool,
    /// Last directory used in the open/save dialogs.
    pub last_dir: Option<PathBuf>,
    /// Most recent first, for Ctrl+P.
    pub recent: Vec<PathBuf>,
    /// Folder open as a project, reopened at start.
    pub project: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TabSession {
    pub path: PathBuf,
    pub mode: Mode,
    pub anchor: usize,
    pub head: usize,
    pub scroll_y: f32,
    /// View mode without editing.
    pub view_only: bool,
}

pub const MAX_RECENT: usize = 50;

impl Session {
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str(&text).ok()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }

    pub fn push_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/session.toml");
        let mut s = Session {
            tabs: vec![TabSession {
                path: "/a.md".into(),
                mode: Mode::Split,
                anchor: 3,
                head: 9,
                scroll_y: 120.5,
                view_only: true,
            }],
            active: 0,
            sidebar_visible: true,
            sidebar_tab: "outline".into(),
            sidebar_width: 260.0,
            ..Default::default()
        };
        s.push_recent(Path::new("/b.md"));
        s.push_recent(Path::new("/a.md"));
        s.push_recent(Path::new("/b.md"));
        s.save(&path).unwrap();
        let back = Session::load(&path).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.recent, vec![PathBuf::from("/b.md"), PathBuf::from("/a.md")]);
    }

    #[test]
    fn missing_or_broken_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Session::load(&dir.path().join("nope.toml")).is_none());
        std::fs::write(dir.path().join("bad.toml"), "tabs = 3").unwrap();
        assert!(Session::load(&dir.path().join("bad.toml")).is_none());
    }
}
