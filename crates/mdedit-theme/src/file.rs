//! Theme files (DESIGN_SYSTEM 2.5) and theme selection (SPEC 9).
//!
//! A theme is a TOML file whose id is its file stem. Every value is a hex
//! colour (`#RRGGBB` or `#RRGGBBAA`) or the name of another token, resolved
//! after inheritance, so a theme that overrides `text-muted` also changes
//! every token that references it. `extends = "<id>"` inherits from another
//! theme; a user theme without `extends` inherits from the built-in of its
//! `kind`, so it only needs the tokens it changes.
//!
//! ```toml
//! name = "Ember Sepia"
//! kind = "light"
//! extends = "ember-light"
//!
//! [color]
//! bg = "#F8F1E3"
//! bg-subtle = "#EFE6D3"
//!
//! [font]
//! prose = "Source Serif 4"
//!
//! [syntax]                 # the `syntax-*` tokens
//! url = "text-secondary"
//!
//! [code]                   # syntect scope selector = colour or style
//! "comment" = { color = "text-muted", italic = true }
//! "string" = "#4D6B2F"
//! ```
//!
//! See `docs/example-theme.toml` for a complete commented file.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{CodeStyle, ColorToken, Fonts, Rgba, Theme, ThemeKind};

pub(crate) struct Builtin {
    id: &'static str,
    src: &'static str,
}

pub(crate) const EMBER_LIGHT: Builtin =
    Builtin { id: "ember-light", src: include_str!("../../../assets/themes/ember-light.toml") };
pub(crate) const EMBER_DARK: Builtin =
    Builtin { id: "ember-dark", src: include_str!("../../../assets/themes/ember-dark.toml") };
const BUILTINS: &[Builtin] = &[EMBER_LIGHT, EMBER_DARK];

/// A built-in theme, parsed once; the files are checked by the tests, so
/// this cannot fail.
pub(crate) fn builtin(b: Builtin) -> Theme {
    static RESOLVED: std::sync::LazyLock<Vec<Theme>> = std::sync::LazyLock::new(|| {
        let set = ThemeSet::builtin();
        BUILTINS.iter().map(|b| set.get(b.id).expect("built-in theme resolves")).collect()
    });
    let index = BUILTINS.iter().position(|other| other.id == b.id).expect("listed in BUILTINS");
    RESOLVED[index].clone()
}

/// The user theme directory: `~/.config/mdedit/themes` on Linux, the
/// platform config dir elsewhere. `None` if no home directory is known.
pub fn user_theme_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "mdedit").map(|d| d.config_dir().join("themes"))
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("theme `{id}`: {source}")]
    Parse { id: String, source: Box<toml::de::Error> },
    #[error("unknown theme `{0}`")]
    UnknownTheme(String),
    #[error("theme `{0}` is built in and cannot be replaced; give the file another name")]
    ReservedId(String),
    #[error("theme `{0}`: `extends` forms a cycle")]
    ExtendsCycle(String),
    #[error("theme `{theme}`: unknown colour token `{token}`")]
    UnknownToken { theme: String, token: String },
    #[error("theme `{theme}`: `{key}` = `{value}` is neither a #hex colour nor a token name")]
    BadValue { theme: String, key: String, value: String },
    #[error("theme `{theme}`: token `{token}` has no value")]
    MissingToken { theme: String, token: String },
    #[error("theme `{theme}`: token references around `{token}` form a cycle")]
    ReferenceCycle { theme: String, token: String },
}

/// `appearance` in settings: follow the OS, or force light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    /// The kind to show. `os` is the OS preference if known (iced reports
    /// it); without one, `System` means light.
    pub fn resolve(self, os: Option<ThemeKind>) -> ThemeKind {
        match self {
            Appearance::System => os.unwrap_or(ThemeKind::Light),
            Appearance::Light => ThemeKind::Light,
            Appearance::Dark => ThemeKind::Dark,
        }
    }
}

/// The theme part of `settings.toml` (SPEC 9): one light and one dark theme,
/// and which of them to show. Embed it in the app's settings struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ThemeSettings {
    pub appearance: Appearance,
    pub light_theme: String,
    pub dark_theme: String,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self { appearance: Appearance::System, light_theme: EMBER_LIGHT.id.into(), dark_theme: EMBER_DARK.id.into() }
    }
}

/// A theme as listed in the settings dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    pub kind: ThemeKind,
    pub builtin: bool,
    /// The file it came from, for user themes.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: Option<String>,
    kind: Option<ThemeKind>,
    extends: Option<String>,
    shadow: Option<String>,
    #[serde(default)]
    color: BTreeMap<String, String>,
    #[serde(default)]
    font: FontSection,
    #[serde(default)]
    syntax: BTreeMap<String, String>,
    #[serde(default)]
    code: BTreeMap<String, CodeValue>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FontSection {
    ui: Option<String>,
    prose: Option<String>,
    mono: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum CodeValue {
    Color(String),
    Style(CodeTable),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeTable {
    color: String,
    #[serde(default)]
    bold: bool,
    #[serde(default)]
    italic: bool,
}

struct Entry {
    file: ThemeFile,
    builtin: bool,
    path: Option<PathBuf>,
}

/// All known themes: the built-ins plus any user theme files.
pub struct ThemeSet {
    themes: BTreeMap<String, Entry>,
}

impl Default for ThemeSet {
    fn default() -> Self {
        Self::builtin()
    }
}

impl ThemeSet {
    /// Only the built-in themes.
    pub fn builtin() -> Self {
        let mut set = Self { themes: BTreeMap::new() };
        for b in BUILTINS {
            set.insert(b.id, b.src, None, true).expect("built-in theme parses");
        }
        set
    }

    /// Built-ins plus the themes in [`user_theme_dir`]. Files that fail to
    /// load are skipped and returned as errors for the app to report.
    pub fn load_user() -> (Self, Vec<ThemeError>) {
        let mut set = Self::builtin();
        let errors = match user_theme_dir() {
            Some(dir) => set.load_dir(&dir),
            None => Vec::new(),
        };
        (set, errors)
    }

    /// Adds every `*.toml` in `dir` (a missing dir is not an error). Bad
    /// files are skipped and returned; themes that parse but do not resolve
    /// (unknown parent, missing token) are reported by [`ThemeSet::get`].
    pub fn load_dir(&mut self, dir: &Path) -> Vec<ThemeError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(source) => return vec![ThemeError::Io { path: dir.into(), source }],
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "toml") && p.is_file())
            .collect();
        paths.sort();
        let mut errors = Vec::new();
        for path in paths {
            if let Err(e) = self.load_file(&path) {
                errors.push(e);
            }
        }
        errors
    }

    /// Adds one theme file; its id is the file stem.
    pub fn load_file(&mut self, path: &Path) -> Result<String, ThemeError> {
        let src = std::fs::read_to_string(path).map_err(|source| ThemeError::Io { path: path.into(), source })?;
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.insert(&id, &src, Some(path.into()), false)?;
        Ok(id)
    }

    /// Adds a theme from TOML source under `id`, replacing an earlier user
    /// theme with the same id.
    pub fn add_toml(&mut self, id: &str, src: &str) -> Result<(), ThemeError> {
        self.insert(id, src, None, false)
    }

    fn insert(&mut self, id: &str, src: &str, path: Option<PathBuf>, builtin: bool) -> Result<(), ThemeError> {
        if !builtin && self.themes.get(id).is_some_and(|e| e.builtin) {
            return Err(ThemeError::ReservedId(id.into()));
        }
        let file: ThemeFile =
            toml::from_str(src).map_err(|e| ThemeError::Parse { id: id.into(), source: Box::new(e) })?;
        let unknown = |token: String| ThemeError::UnknownToken { theme: id.into(), token };
        for key in file.color.keys() {
            ColorToken::from_name(key).ok_or_else(|| unknown(key.clone()))?;
        }
        for key in file.syntax.keys() {
            ColorToken::from_name(&format!("syntax-{key}")).ok_or_else(|| unknown(format!("syntax-{key}")))?;
        }
        self.themes.insert(id.into(), Entry { file, builtin, path });
        Ok(())
    }

    /// Theme ids, sorted.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.themes.keys().map(String::as_str)
    }

    /// Every theme that resolves, for the settings dialog.
    pub fn list(&self) -> Vec<ThemeInfo> {
        self.themes
            .iter()
            .filter_map(|(id, entry)| {
                let theme = self.get(id).ok()?;
                Some(ThemeInfo {
                    id: id.clone(),
                    name: theme.name,
                    kind: theme.kind,
                    builtin: entry.builtin,
                    path: entry.path.clone(),
                })
            })
            .collect()
    }

    /// The theme to show for these settings. Falls back to the built-in of
    /// the wanted kind if the selected theme is missing or broken; call
    /// [`ThemeSet::get`] to find out why.
    pub fn select(&self, settings: &ThemeSettings, os: Option<ThemeKind>) -> Theme {
        let kind = settings.appearance.resolve(os);
        let id = match kind {
            ThemeKind::Light => &settings.light_theme,
            ThemeKind::Dark => &settings.dark_theme,
        };
        self.get(id).unwrap_or_else(|_| Theme::default_for(kind))
    }

    /// Resolves a theme: applies its `extends` chain, then token references.
    pub fn get(&self, id: &str) -> Result<Theme, ThemeError> {
        let chain = self.chain(id)?;
        let leaf = chain[0];

        let mut raw: HashMap<ColorToken, String> = HashMap::new();
        let mut kind = ThemeKind::Light;
        let mut fonts = Fonts::default();
        let mut shadow = None;
        let mut code: BTreeMap<String, CodeValue> = BTreeMap::new();
        for file in chain.iter().rev() {
            for (key, value) in &file.color {
                raw.insert(ColorToken::from_name(key).expect("checked on insert"), value.clone());
            }
            for (key, value) in &file.syntax {
                let token = ColorToken::from_name(&format!("syntax-{key}")).expect("checked on insert");
                raw.insert(token, value.clone());
            }
            kind = file.kind.unwrap_or(kind);
            shadow = file.shadow.clone().or(shadow);
            let f = &file.font;
            fonts.ui = f.ui.clone().unwrap_or(fonts.ui);
            fonts.prose = f.prose.clone().unwrap_or(fonts.prose);
            fonts.mono = f.mono.clone().unwrap_or(fonts.mono);
            code.extend(file.code.iter().map(|(k, v)| (k.clone(), v.clone())));
        }

        let resolver = Resolver { theme: id, raw: &raw };
        let mut colors = Vec::with_capacity(ColorToken::ALL.len());
        for &token in ColorToken::ALL {
            colors.push(resolver.token(token, 0)?);
        }
        let shadow = match shadow {
            Some(value) => resolver.value("shadow", &value, 0)?,
            None => Rgba { r: 0, g: 0, b: 0, a: 26 },
        };
        let code = code
            .into_iter()
            .map(|(scope, value)| {
                let (color, bold, italic) = match value {
                    CodeValue::Color(c) => (c, false, false),
                    CodeValue::Style(t) => (t.color, t.bold, t.italic),
                };
                let color = resolver.value(&scope, &color, 0)?;
                Ok(CodeStyle { scope, color, bold, italic })
            })
            .collect::<Result<_, ThemeError>>()?;

        Ok(Theme { name: leaf.name.clone().unwrap_or_else(|| id.into()), kind, colors, fonts, shadow, code })
    }

    /// The theme and its ancestors, leaf first.
    fn chain(&self, id: &str) -> Result<Vec<&ThemeFile>, ThemeError> {
        let mut chain = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut current = id;
        loop {
            let entry = self.themes.get(current).ok_or_else(|| ThemeError::UnknownTheme(current.into()))?;
            seen.push(current);
            chain.push(&entry.file);
            let parent = match (&entry.file.extends, entry.builtin) {
                (Some(parent), _) => parent.as_str(),
                (None, true) => return Ok(chain),
                (None, false) => match entry.file.kind {
                    Some(ThemeKind::Dark) => EMBER_DARK.id,
                    _ => EMBER_LIGHT.id,
                },
            };
            if seen.contains(&parent) {
                return Err(ThemeError::ExtendsCycle(id.into()));
            }
            current = parent;
        }
    }
}

struct Resolver<'a> {
    theme: &'a str,
    raw: &'a HashMap<ColorToken, String>,
}

impl Resolver<'_> {
    const MAX_DEPTH: usize = 16;

    fn token(&self, token: ColorToken, depth: usize) -> Result<Rgba, ThemeError> {
        if depth > Self::MAX_DEPTH {
            return Err(ThemeError::ReferenceCycle { theme: self.theme.into(), token: token.name().into() });
        }
        let value = self
            .raw
            .get(&token)
            .ok_or_else(|| ThemeError::MissingToken { theme: self.theme.into(), token: token.name().into() })?;
        self.value(token.name(), value, depth)
    }

    fn value(&self, key: &str, value: &str, depth: usize) -> Result<Rgba, ThemeError> {
        let bad = || ThemeError::BadValue { theme: self.theme.into(), key: key.into(), value: value.into() };
        if value.starts_with('#') {
            Rgba::parse(value).ok_or_else(bad)
        } else {
            let token = ColorToken::from_name(value).ok_or_else(bad)?;
            self.token(token, depth + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_with(id: &str, src: &str) -> ThemeSet {
        let mut set = ThemeSet::builtin();
        set.add_toml(id, src).unwrap();
        set
    }

    #[test]
    fn extends_overrides_only_some_tokens() {
        let set = set_with(
            "sepia",
            r##"
            name = "Sepia"
            extends = "ember-light"
            [color]
            bg = "#F8F1E3"
            text-muted = "#6A655E"
            [font]
            prose = "Source Serif 4"
            "##,
        );
        let sepia = set.get("sepia").unwrap();
        let light = Theme::ember_light();
        assert_eq!(sepia.name, "Sepia");
        assert_eq!(sepia.kind, ThemeKind::Light);
        assert_eq!(sepia.color(ColorToken::Bg), Rgba::hex(0xF8F1E3));
        assert_eq!(sepia.color(ColorToken::Accent), light.color(ColorToken::Accent));
        // syntax-marker = "text-muted" in the parent follows the override.
        assert_eq!(sepia.color(ColorToken::SyntaxMarker), Rgba::hex(0x6A655E));
        assert_eq!(sepia.fonts.prose, "Source Serif 4");
        assert_eq!(sepia.fonts.mono, "JetBrains Mono");
        assert_eq!(sepia.code.len(), light.code.len());
    }

    #[test]
    fn implicit_base_follows_kind_and_chains_work() {
        let mut set = ThemeSet::builtin();
        set.add_toml("night", "kind = \"dark\"\n[color]\naccent = \"#4FA3FF\"").unwrap();
        set.add_toml("night-plus", "extends = \"night\"\n[syntax]\nlink = \"accent\"\n").unwrap();
        let night = set.get("night").unwrap();
        assert_eq!(night.kind, ThemeKind::Dark);
        assert_eq!(night.name, "night");
        assert_eq!(night.color(ColorToken::Bg), Theme::ember_dark().color(ColorToken::Bg));
        let plus = set.get("night-plus").unwrap();
        assert_eq!(plus.kind, ThemeKind::Dark);
        assert_eq!(plus.color(ColorToken::SyntaxLink), Rgba::hex(0x4FA3FF));
    }

    #[test]
    fn code_rules_accept_colours_tokens_and_tables() {
        let set = set_with(
            "c",
            r##"
            [code]
            "string" = "#123456"
            "comment" = { color = "accent", bold = true }
            "##,
        );
        let theme = set.get("c").unwrap();
        let rule = |scope: &str| theme.code.iter().find(|c| c.scope == scope).unwrap().clone();
        assert_eq!(rule("string").color, Rgba::hex(0x123456));
        let comment = rule("comment");
        assert_eq!(comment.color, theme.color(ColorToken::Accent));
        assert!(comment.bold && !comment.italic);
        // Inherited rules are kept.
        assert!(theme.code.iter().any(|c| c.scope == "keyword"));
    }

    #[test]
    fn errors_are_reported() {
        let mut set = ThemeSet::builtin();
        assert!(matches!(set.add_toml("a", "[color]\nnope = \"#000000\""), Err(ThemeError::UnknownToken { .. })));
        assert!(matches!(set.add_toml("a", "[syntax]\nbg = \"#000000\""), Err(ThemeError::UnknownToken { .. })));
        assert!(matches!(set.add_toml("a", "colour = 1"), Err(ThemeError::Parse { .. })));
        assert!(matches!(set.add_toml("ember-light", ""), Err(ThemeError::ReservedId(_))));

        set.add_toml("bad-hex", "[color]\nbg = \"#12\"").unwrap();
        assert!(matches!(set.get("bad-hex"), Err(ThemeError::BadValue { .. })));
        set.add_toml("orphan", "extends = \"missing\"").unwrap();
        assert!(matches!(set.get("orphan"), Err(ThemeError::UnknownTheme(_))));
        set.add_toml("x", "extends = \"y\"").unwrap();
        set.add_toml("y", "extends = \"x\"").unwrap();
        assert!(matches!(set.get("x"), Err(ThemeError::ExtendsCycle(_))));
        set.add_toml("loop", "[color]\ntext = \"text-muted\"\ntext-muted = \"text\"").unwrap();
        assert!(matches!(set.get("loop"), Err(ThemeError::ReferenceCycle { .. })));
        assert!(matches!(set.get("nothing"), Err(ThemeError::UnknownTheme(_))));
    }

    #[test]
    fn appearance_and_selection() {
        assert_eq!(Appearance::System.resolve(Some(ThemeKind::Dark)), ThemeKind::Dark);
        assert_eq!(Appearance::System.resolve(None), ThemeKind::Light);
        assert_eq!(Appearance::Light.resolve(Some(ThemeKind::Dark)), ThemeKind::Light);
        assert_eq!(Appearance::Dark.resolve(None), ThemeKind::Dark);

        let set = set_with("sepia", "name = \"Sepia\"\n[color]\nbg = \"#F8F1E3\"");
        let mut settings = ThemeSettings::default();
        assert_eq!(set.select(&settings, Some(ThemeKind::Dark)).name, "Ember Dark");
        settings.light_theme = "sepia".into();
        assert_eq!(set.select(&settings, None).name, "Sepia");
        settings.light_theme = "gone".into();
        assert_eq!(set.select(&settings, None).name, "Ember Light");
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let settings: ThemeSettings = toml::from_str("appearance = \"dark\"\ndark-theme = \"night\"").unwrap();
        assert_eq!(settings.appearance, Appearance::Dark);
        assert_eq!(settings.dark_theme, "night");
        assert_eq!(settings.light_theme, "ember-light");
        let back: ThemeSettings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn load_dir_reads_user_themes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sepia.toml"), "name = \"Sepia\"\n[color]\nbg = \"#F8F1E3\"").unwrap();
        std::fs::write(dir.path().join("broken.toml"), "name = ").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let mut set = ThemeSet::builtin();
        let errors = set.load_dir(dir.path());
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], ThemeError::Parse { ref id, .. } if id == "broken"));
        let list = set.list();
        let sepia = list.iter().find(|t| t.id == "sepia").unwrap();
        assert!(!sepia.builtin && sepia.path.is_some());
        assert_eq!(list.iter().filter(|t| t.builtin).count(), 2);
        assert!(set.load_dir(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn example_theme_in_docs_loads_and_passes_contrast() {
        let set = set_with("example", include_str!("../../../docs/example-theme.toml"));
        let theme = set.get("example").unwrap();
        assert!(theme.contrast_failures().is_empty(), "{:#?}", theme.contrast_failures());
    }

    #[test]
    fn user_theme_dir_is_named_after_the_app() {
        if let Some(dir) = user_theme_dir() {
            assert!(dir.ends_with("themes"));
            assert!(dir.to_string_lossy().contains("mdedit"));
        }
    }
}
