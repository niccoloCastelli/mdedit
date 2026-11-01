//! Keymap table (DESIGN_SYSTEM 8.2): default shortcuts from the command
//! registry, overridden by `[keymap]` in settings.toml and by `keymap.toml`
//! in the config dir.
//!
//! ```toml
//! # keymap.toml
//! "file.save" = "Ctrl+S"
//! "view.toggle_sidebar" = ["Ctrl+B", "F9"]
//! "view.zoom_in" = ""            # unbind
//! ```
//!
//! `Ctrl` means Cmd on macOS and `Alt` means Option.

use std::collections::BTreeMap;
use std::fmt;

use iced::keyboard::{Key, Modifiers, key::Named};

use crate::command;

/// A key plus modifiers. `primary` is Ctrl (Cmd on macOS).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub primary: bool,
    pub alt: bool,
    pub shift: bool,
    /// Lowercase key name: a character (`"s"`, `"1"`, `"."`) or a named key
    /// (`"tab"`, `"enter"`, `"f9"`).
    pub key: String,
}

impl KeyChord {
    /// Parse `"Ctrl+Shift+P"`. Case-insensitive; `Cmd`/`Meta` are aliases of
    /// `Ctrl`, `Option` of `Alt`. `+` itself is written `Plus`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut chord = KeyChord { primary: false, alt: false, shift: false, key: String::new() };
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = parts.split_last()?;
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "meta" | "super" | "primary" => chord.primary = true,
                "alt" | "option" => chord.alt = true,
                "shift" => chord.shift = true,
                _ => return None,
            }
        }
        let key = key.to_lowercase();
        let key = match key.as_str() {
            "" => return None,
            "plus" => "+".to_string(),
            "esc" => "escape".to_string(),
            "return" => "enter".to_string(),
            "space" => " ".to_string(),
            _ => key,
        };
        if key.chars().count() > 1 && !NAMED.iter().any(|(n, _)| *n == key) {
            return None;
        }
        chord.key = key;
        Some(chord)
    }

    /// The chord for a key press, using the key without modifiers applied so
    /// `Ctrl+Shift+8` matches regardless of what Shift+8 types.
    pub fn from_event(key: &Key, modifiers: Modifiers) -> Option<Self> {
        let name = match key {
            Key::Character(c) => c.to_lowercase(),
            Key::Named(named) => NAMED.iter().find(|(_, n)| n == named)?.0.to_string(),
            Key::Unidentified => return None,
        };
        Some(KeyChord { primary: modifiers.command(), alt: modifiers.alt(), shift: modifiers.shift(), key: name })
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mac = cfg!(target_os = "macos");
        if self.primary {
            f.write_str(if mac { "Cmd+" } else { "Ctrl+" })?;
        }
        if self.alt {
            f.write_str(if mac { "Option+" } else { "Alt+" })?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        match self.key.as_str() {
            " " => f.write_str("Space"),
            "+" => f.write_str("Plus"),
            k if k.chars().count() == 1 => write!(f, "{}", k.to_uppercase()),
            k => {
                let mut c = k.chars();
                let first = c.next().map(|c| c.to_ascii_uppercase()).unwrap_or_default();
                write!(f, "{first}{}", c.as_str())
            }
        }
    }
}

const NAMED: &[(&str, Named)] = &[
    ("enter", Named::Enter),
    ("tab", Named::Tab),
    ("escape", Named::Escape),
    ("backspace", Named::Backspace),
    ("delete", Named::Delete),
    ("insert", Named::Insert),
    ("home", Named::Home),
    ("end", Named::End),
    ("pageup", Named::PageUp),
    ("pagedown", Named::PageDown),
    ("up", Named::ArrowUp),
    ("down", Named::ArrowDown),
    ("left", Named::ArrowLeft),
    ("right", Named::ArrowRight),
    ("f1", Named::F1),
    ("f2", Named::F2),
    ("f3", Named::F3),
    ("f4", Named::F4),
    ("f5", Named::F5),
    ("f6", Named::F6),
    ("f7", Named::F7),
    ("f8", Named::F8),
    ("f9", Named::F9),
    ("f10", Named::F10),
    ("f11", Named::F11),
    ("f12", Named::F12),
];

/// One override value: a single shortcut, several, or `""` to unbind.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum Binding {
    One(String),
    Many(Vec<String>),
}

impl Binding {
    fn chords(&self) -> Vec<&str> {
        match self {
            Binding::One(s) if s.trim().is_empty() => Vec::new(),
            Binding::One(s) => vec![s.as_str()],
            Binding::Many(v) => v.iter().map(String::as_str).filter(|s| !s.trim().is_empty()).collect(),
        }
    }
}

pub type Overrides = BTreeMap<String, Binding>;

/// Chord → command id, built from defaults plus overrides.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: Vec<(KeyChord, &'static str)>,
    /// Problems found while applying overrides, shown once as a toast.
    pub warnings: Vec<String>,
}

impl Keymap {
    /// Defaults with each override layer applied in order (later wins).
    pub fn new<'a>(layers: impl IntoIterator<Item = &'a Overrides>) -> Self {
        let mut map: BTreeMap<&'static str, Vec<KeyChord>> = BTreeMap::new();
        for c in command::all() {
            map.insert(c.id, c.shortcut.and_then(KeyChord::parse).into_iter().collect());
        }
        let mut warnings = Vec::new();
        for layer in layers {
            for (id, binding) in layer {
                let Some(entry) = command::find(id) else {
                    warnings.push(format!("keymap: unknown command \"{id}\""));
                    continue;
                };
                let mut chords = Vec::new();
                for s in binding.chords() {
                    match KeyChord::parse(s) {
                        Some(c) => chords.push(c),
                        None => warnings.push(format!("keymap: can't parse \"{s}\" for {id}")),
                    }
                }
                // An override takes the chord away from any other command.
                for other in map.values_mut() {
                    other.retain(|c| !chords.contains(c));
                }
                map.insert(entry.id, chords);
            }
        }
        let bindings = map.into_iter().flat_map(|(id, chords)| chords.into_iter().map(move |c| (c, id))).collect();
        Self { bindings, warnings }
    }

    pub fn lookup(&self, chord: &KeyChord) -> Option<&'static str> {
        self.bindings.iter().find(|(c, _)| c == chord).map(|(_, id)| *id)
    }

    /// The first chord bound to `id`, for menus and the palette.
    pub fn shortcut_for(&self, id: &str) -> Option<&KeyChord> {
        self.bindings.iter().find(|(_, i)| *i == id).map(|(c, _)| c)
    }
}

/// Parse a `keymap.toml` file.
pub fn parse_overrides(text: &str) -> Result<Overrides, toml::de::Error> {
    toml::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays() {
        let c = KeyChord::parse("ctrl+shift+p").unwrap();
        assert!(c.primary && c.shift && !c.alt);
        assert_eq!(c.key, "p");
        if !cfg!(target_os = "macos") {
            assert_eq!(c.to_string(), "Ctrl+Shift+P");
            assert_eq!(KeyChord::parse("Ctrl+Tab").unwrap().to_string(), "Ctrl+Tab");
        }
        assert_eq!(KeyChord::parse("Ctrl+Plus").unwrap().key, "+");
        assert!(KeyChord::parse("Hyper+X").is_none());
        assert!(KeyChord::parse("Ctrl+Nope").is_none());
        assert!(KeyChord::parse("").is_none());
    }

    #[test]
    fn matches_events() {
        let chord = KeyChord::from_event(&Key::Character("S".into()), Modifiers::CTRL).unwrap();
        let km = Keymap::new([]);
        if !cfg!(target_os = "macos") {
            assert_eq!(km.lookup(&chord), Some("file.save"));
        }
        let tab = KeyChord::from_event(&Key::Named(Named::Tab), Modifiers::CTRL | Modifiers::SHIFT).unwrap();
        assert_eq!(tab, KeyChord::parse("Ctrl+Shift+Tab").unwrap());
    }

    #[test]
    fn overrides_rebind_unbind_and_steal() {
        let o = parse_overrides(
            r#"
            "view.toggle_sidebar" = ["Ctrl+B", "F9"]
            "view.zoom_in" = ""
            "nope.nothing" = "Ctrl+J"
            "file.save" = "Ctrl+Shift+Nope"
            "#,
        )
        .unwrap();
        let km = Keymap::new([&o]);
        let b = KeyChord::parse("Ctrl+B").unwrap();
        assert_eq!(km.lookup(&b), Some("view.toggle_sidebar"));
        assert_eq!(km.lookup(&KeyChord::parse("F9").unwrap()), Some("view.toggle_sidebar"));
        assert_eq!(km.lookup(&KeyChord::parse("Ctrl+=").unwrap()), None);
        assert!(km.shortcut_for("format.bold").is_none(), "Ctrl+B was taken from bold");
        assert_eq!(km.warnings.len(), 2);
        assert!(km.shortcut_for("file.save").is_none());
    }

    #[test]
    fn defaults_have_no_duplicate_chords() {
        let km = Keymap::new([]);
        let mut seen = std::collections::HashMap::new();
        for (c, id) in &km.bindings {
            if let Some(prev) = seen.insert(c.clone(), *id) {
                panic!("{c} bound to both {prev} and {id}");
            }
        }
    }
}
