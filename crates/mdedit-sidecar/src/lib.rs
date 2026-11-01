//! Sidecar files and comments (SPEC 7).
//!
//! `name.md` gets a hidden `.name.md.toml` next to it, created only when
//! needed and deleted when empty. Works for loose files too, which is why it
//! is its own crate rather than part of `mdedit-project`.
//!
//! Sidecar workstream: load/save (atomic, delete when empty), the
//! re-anchoring algorithm, [`LiveAnchors`] driven by the edit log, and the
//! fuzz test "random edits around anchors → move or detach, never lost".

use std::ops::Range;
use std::path::{Path, PathBuf};

use mdedit_core::{AppliedEdit, EditListener, TextBuffer};
use serde::{Deserialize, Serialize};

pub const SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    pub version: u32,
    #[serde(default, rename = "comment", skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
}

impl Default for Sidecar {
    fn default() -> Self {
        Self { version: SIDECAR_VERSION, comments: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: String,
    #[serde(with = "toml_timestamp")]
    pub created: jiff::Timestamp,
    #[serde(default)]
    pub resolved: bool,
    pub body: String,
    pub anchor: Anchor,
    #[serde(default, rename = "reply", skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub author: String,
    #[serde(with = "toml_timestamp")]
    pub created: jiff::Timestamp,
    pub body: String,
}

/// Where a comment is attached, stored as text so it survives edits made
/// outside the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub quote: String,
    /// Up to 32 chars before the quote.
    #[serde(default)]
    pub prefix: String,
    /// Up to 32 chars after the quote.
    #[serde(default)]
    pub suffix: String,
    /// Last known byte offset of the quote.
    #[serde(default)]
    pub hint: usize,
}

/// Outcome of re-anchoring one comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Attached(Range<usize>),
    /// Shown in the "Detached" group; never silently deleted.
    Detached,
}

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid sidecar: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported sidecar version {0}")]
    Version(u32),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}

/// `dir/name.md` → `dir/.name.md.toml`.
pub fn sidecar_path(md_file: &Path) -> PathBuf {
    let name = md_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    md_file.with_file_name(format!(".{name}.toml"))
}

impl Sidecar {
    /// Load the sidecar of `md_file`; a missing file is an empty sidecar.
    pub fn load(md_file: &Path) -> Result<Self, SidecarError> {
        let _ = md_file;
        Err(SidecarError::Unimplemented("Sidecar::load"))
    }

    /// Save atomically, or delete the file when there is nothing to keep.
    pub fn save(&self, md_file: &Path) -> Result<(), SidecarError> {
        let _ = md_file;
        Err(SidecarError::Unimplemented("Sidecar::save"))
    }
}

/// Find an anchor in the text (SPEC 7: exact with context near `hint`,
/// exact quote near `hint`, fuzzy, else detached). Placeholder.
pub fn reanchor(anchor: &Anchor, buf: &dyn TextBuffer) -> Resolution {
    let _ = (anchor, buf);
    Resolution::Detached
}

/// Build an anchor for a new comment on `range`.
pub fn make_anchor(buf: &dyn TextBuffer, range: Range<usize>) -> Anchor {
    Anchor {
        quote: buf.slice(range.clone()).into_owned(),
        prefix: String::new(),
        suffix: String::new(),
        hint: range.start,
    }
}

/// Comment ranges while the document is open, moved by the edit log.
#[derive(Debug, Default)]
pub struct LiveAnchors {
    /// Indexed like `Sidecar::comments`.
    pub ranges: Vec<Resolution>,
}

impl EditListener for LiveAnchors {
    fn on_edit(&mut self, applied: &AppliedEdit) {
        let _ = applied;
    }
}

/// TOML has a native datetime type (`created = 2026-11-01T15:30:00Z`, no
/// quotes); map it to `jiff::Timestamp`.
mod toml_timestamp {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(ts: &jiff::Timestamp, s: S) -> Result<S::Ok, S::Error> {
        let dt: toml::value::Datetime = ts.to_string().parse().map_err(serde::ser::Error::custom)?;
        serde::Serialize::serialize(&dt, s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<jiff::Timestamp, D::Error> {
        let dt = toml::value::Datetime::deserialize(d)?;
        dt.to_string().parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_is_hidden_toml() {
        assert_eq!(sidecar_path(Path::new("a/b/intro.md")), PathBuf::from("a/b/.intro.md.toml"));
    }

    #[test]
    fn parses_spec_example() {
        let s: Sidecar = toml::from_str(
            r#"
version = 1

[[comment]]
id = "c_01JC8Z3Q"
author = "Niccolo"
created = 2026-11-01T15:30:00Z
resolved = false
body = "Is this figure still right?"

  [comment.anchor]
  quote = "3–5 business days"
  prefix = "In the recipient's currency, "
  suffix = "\n"
  hint = 4812

  [[comment.reply]]
  author = "Niccolo"
  created = 2026-11-01T15:42:00Z
  body = "Yes, checked with ops."
"#,
        )
        .unwrap();
        assert_eq!(s.comments.len(), 1);
        assert_eq!(s.comments[0].anchor.hint, 4812);
        assert_eq!(s.comments[0].replies.len(), 1);
        let again: Sidecar = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
        assert_eq!(again, s);
    }
}
