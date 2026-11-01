//! Document commands (formatting & smart-editing workstream).
//!
//! ARCHITECTURE 5: every action is a command defined once and read by the
//! menu bar, floating toolbar, slash menu, palette and keymap. Document
//! commands live here so the MCP server can reuse them; UI-only commands
//! (toggle sidebar, switch mode) live in `mdedit-app`.

use crate::Document;
use crate::edit::Edit;
use crate::selection::Selection;

/// Stable string id, e.g. `"format.bold"`. Used in keymap.toml overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub &'static str);

/// Static description of a command for menus, palette and keymap.
#[derive(Debug, Clone, Copy)]
pub struct CommandMeta {
    pub id: CommandId,
    pub title: &'static str,
    /// Lucide icon name, e.g. `"bold"`.
    pub icon: Option<&'static str>,
    /// Default shortcut in keymap syntax, e.g. `"Ctrl+B"` (Cmd on macOS).
    pub shortcut: Option<&'static str>,
    /// Slash-menu keyword without the slash, e.g. `"h1"`.
    pub slash: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocCommand {
    ToggleBold,
    ToggleItalic,
    ToggleStrike,
    ToggleInlineCode,
    Link {
        url: Option<String>,
    },
    /// 0 = paragraph.
    SetHeading(u8),
    BulletList,
    NumberedList,
    TaskList,
    Quote,
    CodeBlock,
    InsertTable {
        rows: u8,
        cols: u8,
    },
    HorizontalRule,
}

/// The edits a command makes, plus the selection afterwards. The caller
/// applies the edits through `Document::apply` under one `TxnId`.
pub struct CommandResult {
    pub edits: Vec<Edit>,
    pub selection: Selection,
}

/// Placeholder: does nothing.
pub fn run(doc: &Document, selection: Selection, command: &DocCommand) -> CommandResult {
    let _ = (doc, command);
    CommandResult { edits: Vec::new(), selection }
}

/// Metadata for every document command (filled in by the workstream).
pub fn registry() -> &'static [CommandMeta] {
    &[]
}
