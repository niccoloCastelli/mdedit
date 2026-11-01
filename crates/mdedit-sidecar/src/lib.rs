//! Sidecar files and comments (SPEC 7).
//!
//! `name.md` gets a hidden `.name.md.toml` next to it, created only when
//! needed and deleted when empty. Works for loose files too, which is why it
//! is its own crate rather than part of `mdedit-project`.
//!
//! - [`Sidecar::load`] / [`Sidecar::save`]: atomic writes, the file is
//!   deleted when nothing is left in it, unknown top-level keys survive a
//!   load/save round trip (future extensions, SPEC 7).
//! - [`make_anchor`] / [`reanchor`]: comments are stored as quote + context
//!   so they survive edits made outside the app.
//! - [`LiveAnchors`]: comment ranges while the document is open, moved by
//!   the edit log.

mod anchor;
mod live;

use std::fs;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use anchor::{CONTEXT_CHARS, make_anchor, reanchor};
pub use live::LiveAnchors;

pub const SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    pub version: u32,
    #[serde(default, rename = "comment", skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    /// Top-level keys this version does not know (written by a newer app or
    /// by hand). Kept as they are so saving never drops them.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for Sidecar {
    fn default() -> Self {
        Self { version: SIDECAR_VERSION, comments: Vec::new(), extra: toml::Table::new() }
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
    /// Up to 32 chars before the quote, not reaching back past the start of
    /// its line.
    #[serde(default)]
    pub prefix: String,
    /// Up to 32 chars after the quote, ending at the first line break
    /// (included).
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

impl Resolution {
    pub fn range(&self) -> Option<Range<usize>> {
        match self {
            Resolution::Attached(r) => Some(r.clone()),
            Resolution::Detached => None,
        }
    }

    pub fn is_detached(&self) -> bool {
        matches!(self, Resolution::Detached)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid sidecar: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("cannot write sidecar: {0}")]
    Serialize(#[from] toml::ser::Error),
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
        let text = match fs::read_to_string(sidecar_path(md_file)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        Self::from_toml(&text)
    }

    /// Parse sidecar text. Versions newer than [`SIDECAR_VERSION`] are
    /// refused so an old app never rewrites data it does not understand.
    pub fn from_toml(text: &str) -> Result<Self, SidecarError> {
        let sidecar: Self = toml::from_str(text)?;
        if sidecar.version > SIDECAR_VERSION {
            return Err(SidecarError::Version(sidecar.version));
        }
        Ok(sidecar)
    }

    pub fn to_toml(&self) -> Result<String, SidecarError> {
        Ok(toml::to_string(self)?)
    }

    /// No comments and no extension data: nothing worth a file.
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty() && self.extra.is_empty()
    }

    /// Save atomically (temp file in the same directory, fsync, rename), or
    /// delete the file when there is nothing to keep.
    pub fn save(&self, md_file: &Path) -> Result<(), SidecarError> {
        let path = sidecar_path(md_file);
        if self.is_empty() {
            return match fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            };
        }
        let text = self.to_toml()?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = path.with_file_name(format!("{name}.tmp-{}", std::process::id()));
        let write = || -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &path)
        };
        if let Err(e) = write() {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
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

    const SPEC_EXAMPLE: &str = r#"version = 1

[[comment]]
id = "c_01JC8Z3Q"
author = "Niccolo"
created = 2026-11-01T15:30:00Z
resolved = false
body = "Is this figure still right after the October change?"

  [comment.anchor]
  quote = "3–5 business days"         # exact text the comment is attached to
  prefix = "In the recipient's currency, "   # up to 32 chars before
  suffix = "\n"                       # up to 32 chars after
  hint = 4812                         # last known byte offset, speeds lookup

  [[comment.reply]]
  author = "Niccolo"
  created = 2026-11-01T15:42:00Z
  body = "Yes, checked with ops."
"#;

    #[test]
    fn sidecar_path_is_hidden_toml() {
        assert_eq!(sidecar_path(Path::new("a/b/intro.md")), PathBuf::from("a/b/.intro.md.toml"));
    }

    #[test]
    fn spec_example_round_trips() {
        let s = Sidecar::from_toml(SPEC_EXAMPLE).unwrap();
        assert_eq!(s.comments.len(), 1);
        let c = &s.comments[0];
        assert_eq!(c.id, "c_01JC8Z3Q");
        assert_eq!(c.created, "2026-11-01T15:30:00Z".parse::<jiff::Timestamp>().unwrap());
        assert_eq!(c.anchor.quote, "3–5 business days");
        assert_eq!(c.anchor.prefix, "In the recipient's currency, ");
        assert_eq!(c.anchor.suffix, "\n");
        assert_eq!(c.anchor.hint, 4812);
        assert_eq!(c.replies.len(), 1);
        assert_eq!(c.replies[0].body, "Yes, checked with ops.");
        assert!(s.extra.is_empty());

        let text = s.to_toml().unwrap();
        assert!(text.contains("created = 2026-11-01T15:30:00Z"), "native TOML datetime:\n{text}");
        assert_eq!(Sidecar::from_toml(&text).unwrap(), s);
    }

    #[test]
    fn save_load_through_disk_and_delete_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let md = dir.path().join("intro.md");
        assert_eq!(Sidecar::load(&md).unwrap(), Sidecar::default());

        let mut s = Sidecar::from_toml(SPEC_EXAMPLE).unwrap();
        s.save(&md).unwrap();
        let path = dir.path().join(".intro.md.toml");
        assert!(path.exists());
        assert_eq!(Sidecar::load(&md).unwrap(), s);
        // No temp files left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

        s.comments.clear();
        s.save(&md).unwrap();
        assert!(!path.exists());
        // Saving an empty sidecar with no file is fine too.
        s.save(&md).unwrap();
    }

    #[test]
    fn unknown_top_level_tables_survive() {
        let text = format!(
            "{SPEC_EXAMPLE}\n[export]\ntemplate = \"guide\"\nmargins = [20, 25]\n\n[[block]]\nid = \"b1\"\nat = 2026-11-01T10:00:00Z\n"
        );
        let s = Sidecar::from_toml(&text).unwrap();
        assert_eq!(s.comments.len(), 1);
        assert_eq!(s.extra["export"]["template"].as_str(), Some("guide"));
        assert!(s.extra["block"].is_array());

        let again = Sidecar::from_toml(&s.to_toml().unwrap()).unwrap();
        assert_eq!(again, s);

        // Extension data alone keeps the file alive.
        let mut only_extra = s.clone();
        only_extra.comments.clear();
        assert!(!only_extra.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let md = dir.path().join("a.md");
        only_extra.save(&md).unwrap();
        assert_eq!(Sidecar::load(&md).unwrap(), only_extra);
    }

    #[test]
    fn newer_version_is_refused() {
        assert!(matches!(Sidecar::from_toml("version = 2\n"), Err(SidecarError::Version(2))));
        assert!(matches!(Sidecar::from_toml("version = \"x\"\n"), Err(SidecarError::Parse(_))));
    }
}
