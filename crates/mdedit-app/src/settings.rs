//! App settings (SPEC 9) in `settings.toml` in the platform config dir
//! (`~/.config/mdedit/` on Linux). Every field has a default, so a partial
//! or missing file works; unknown keys are ignored. The file is re-read when
//! it is saved from inside the app, so changes apply live.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::keymap::Overrides;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    pub fonts: FontSettings,
    pub editor: EditorSettings,
    pub files: FileSettings,
    pub author: Author,
    /// Same format as `keymap.toml`; `keymap.toml` wins on conflicts.
    pub keymap: Overrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub mode: AppearanceMode,
    pub light_theme: String,
    pub dark_theme: String,
    /// Document zoom (Ctrl +/-), multiplies the type scale.
    pub zoom: f32,
    /// UI chrome scale, separate from the document zoom.
    pub ui_scale: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            mode: AppearanceMode::System,
            light_theme: "ember-light".into(),
            dark_theme: "ember-dark".into(),
            zoom: 1.0,
            ui_scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSettings {
    /// Empty means the theme's choice.
    pub ui: String,
    pub prose: String,
    pub mono: String,
    pub size: f32,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self { ui: String::new(), prose: String::new(), mono: String::new(), size: 16.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Edit,
    Split,
    View,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Edit => Mode::Split,
            Mode::Split => Mode::View,
            Mode::View => Mode::Edit,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Edit => "Edit",
            Mode::Split => "Split",
            Mode::View => "View",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    pub default_mode: Mode,
    pub line_numbers: bool,
    /// Document column max width in px (DESIGN_SYSTEM 3.3).
    pub measure: f32,
    pub full_width: bool,
    /// Whole buffer in monospace in Edit mode.
    pub monospace: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self { default_mode: Mode::Edit, line_numbers: false, measure: 720.0, full_width: false, monospace: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSettings {
    /// Save loose files automatically on idle. Project files always
    /// autosave (DESIGN_SYSTEM 8.6).
    pub autosave: bool,
    pub large_file_threshold_mb: u64,
    pub restore_session: bool,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self { autosave: false, large_file_threshold_mb: 20, restore_session: true }
    }
}

impl FileSettings {
    pub fn large_file_threshold(&self) -> u64 {
        self.large_file_threshold_mb.saturating_mul(1024 * 1024)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Author {
    pub name: String,
    pub email: String,
}

/// Where the app keeps its files.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl Paths {
    /// Platform dirs, or `$MDEDIT_CONFIG_DIR` for both (tests, portable use).
    pub fn detect() -> Self {
        if let Some(dir) = std::env::var_os("MDEDIT_CONFIG_DIR") {
            let dir = PathBuf::from(dir);
            return Self { config_dir: dir.clone(), state_dir: dir };
        }
        match directories::ProjectDirs::from("", "", "mdedit") {
            Some(d) => Self {
                config_dir: d.config_dir().to_path_buf(),
                state_dir: d.state_dir().unwrap_or_else(|| d.data_local_dir()).to_path_buf(),
            },
            None => {
                let dir = std::env::temp_dir().join("mdedit");
                Self { config_dir: dir.clone(), state_dir: dir }
            }
        }
    }

    pub fn settings(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }

    pub fn keymap(&self) -> PathBuf {
        self.config_dir.join("keymap.toml")
    }

    pub fn session(&self) -> PathBuf {
        self.state_dir.join("session.toml")
    }
}

impl Settings {
    /// Read settings; a missing file gives the defaults, a broken one gives
    /// the defaults plus the error to show.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(s) => (s, None),
                Err(e) => (Self::default(), Some(format!("settings.toml: {}", e.message()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (Self::default(), Some(format!("settings.toml: {e}"))),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }

    /// Settings with only a few fields changed by the UI (appearance, zoom).
    /// Writes them into the existing file so the user's comments and layout
    /// elsewhere survive as far as `toml` allows.
    pub fn store_ui_change(&self, path: &Path) -> std::io::Result<()> {
        let mut doc: toml::Table = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        let fresh: toml::Table = toml::Table::try_from(self).map_err(std::io::Error::other)?;
        for key in ["appearance", "editor"] {
            if let Some(v) = fresh.get(key) {
                doc.insert(key.into(), v.clone());
            }
        }
        let text = toml::to_string_pretty(&doc).map_err(std::io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }
}

/// Temp file in the same dir, fsync, rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Commented starter file written the first time the user opens settings.
pub const SETTINGS_TEMPLATE: &str = r#"# mdedit settings. Saving this file applies the changes.

[appearance]
mode = "system"            # system | light | dark
light_theme = "ember-light"
dark_theme = "ember-dark"
zoom = 1.0
ui_scale = 1.0

[fonts]
ui = ""                    # empty = the theme's font
prose = ""
mono = ""
size = 16.0

[editor]
default_mode = "edit"      # edit | split | view
line_numbers = false
measure = 720.0
full_width = false
monospace = false

[files]
autosave = false           # loose files; project files always autosave
large_file_threshold_mb = 20
restore_session = true

[author]
name = ""
email = ""

# Shortcut overrides, same format as keymap.toml.
[keymap]
"#;

pub const KEYMAP_TEMPLATE: &str = r#"# mdedit keymap overrides. Saving this file applies the changes.
# Command ids are listed in the command palette (Ctrl+Shift+P).
# Ctrl means Cmd on macOS. An empty string unbinds a command.
#
# "view.toggle_sidebar" = ["Ctrl+B", "F9"]
# "format.bold" = ""
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_uses_defaults() {
        let s: Settings = toml::from_str("[editor]\nline_numbers = true\n").unwrap();
        assert!(s.editor.line_numbers);
        assert_eq!(s.editor.measure, 720.0);
        assert_eq!(s.appearance.mode, AppearanceMode::System);
        assert_eq!(s.files.large_file_threshold(), 20 * 1024 * 1024);
    }

    #[test]
    fn templates_parse_to_defaults() {
        let s: Settings = toml::from_str(SETTINGS_TEMPLATE).unwrap();
        assert_eq!(s, Settings::default());
        assert!(crate::keymap::parse_overrides(KEYMAP_TEMPLATE).unwrap().is_empty());
    }

    #[test]
    fn round_trip_and_ui_change_keeps_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[author]\nname = \"N\"\n[keymap]\n\"file.save\" = \"F2\"\n").unwrap();
        let (mut s, err) = Settings::load(&path);
        assert!(err.is_none());
        s.appearance.mode = AppearanceMode::Dark;
        s.store_ui_change(&path).unwrap();
        let (back, _) = Settings::load(&path);
        assert_eq!(back.appearance.mode, AppearanceMode::Dark);
        assert_eq!(back.author.name, "N");
        assert_eq!(back.keymap.len(), 1);
    }

    #[test]
    fn broken_file_reports_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[editor\n").unwrap();
        let (s, err) = Settings::load(&path);
        assert_eq!(s, Settings::default());
        assert!(err.is_some());
    }
}
