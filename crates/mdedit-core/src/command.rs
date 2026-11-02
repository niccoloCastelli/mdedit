//! Document commands (formatting & smart-editing workstream).
//!
//! ARCHITECTURE 5: every action is a command defined once and read by the
//! menu bar, floating toolbar, slash menu, palette and keymap. Document
//! commands live here so the MCP server can reuse them; UI-only commands
//! (toggle sidebar, switch mode) live in `mdedit-app`.
//!
//! Commands never mutate the document: [`run`] returns the edits and the
//! selection afterwards, and the caller applies them through
//! `Document::apply` under one `TxnId` (so a command undoes as one step).
//!
//! Besides the registry commands, the editor hooks [`smart_input`] (typing:
//! Enter, Tab, pairing characters), [`smart_paste`] (a URL onto a selection)
//! and [`align_table_at`] (auto-align when the cursor leaves a table) give
//! the Edit-mode behaviour of DESIGN_SYSTEM 8.3 and 8.4.

mod block;
mod inline;
mod smart;
mod table;
mod text;

pub use table::{display_width, parse_delimited};

use std::ops::Range;

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

impl CommandMeta {
    /// The command this entry runs (with default arguments).
    pub fn command(&self) -> DocCommand {
        DocCommand::from_id(self.id.0).expect("every registry id maps to a command")
    }
}

/// Column alignment in a GFM table delimiter row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ColumnAlign {
    /// `---`
    #[default]
    None,
    /// `:--`
    Left,
    /// `:-:`
    Center,
    /// `--:`
    Right,
}

/// Row and column operations on the table at the cursor (DESIGN_SYSTEM 8.4).
/// Every operation also re-aligns the table. Outside a table they do nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableOp {
    /// Align the source (pad cells so pipes line up).
    Format,
    /// Select the next cell; past the last cell, add a row.
    NextCell,
    PrevCell,
    /// Above the header this inserts the first body row.
    InsertRowAbove,
    InsertRowBelow,
    InsertColumnLeft,
    InsertColumnRight,
    /// The header row cannot be deleted.
    DeleteRow,
    /// The last column cannot be deleted.
    DeleteColumn,
    MoveRowUp,
    MoveRowDown,
    MoveColumnLeft,
    MoveColumnRight,
    Align(ColumnAlign),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocCommand {
    ToggleBold,
    ToggleItalic,
    ToggleStrike,
    ToggleInlineCode,
    /// Wrap the selection or word in a link, or remove the link at the
    /// cursor. Without a URL the cursor is left where the URL goes.
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
    /// A starter ```` ```mermaid ```` block, caret on its last line.
    Mermaid,
    /// `rows` counts the header row. Inside a table this aligns it instead
    /// (Ctrl+Alt+T, DESIGN_SYSTEM 8.4).
    InsertTable {
        rows: u8,
        cols: u8,
    },
    HorizontalRule,
    /// `![selection](url)`.
    Image {
        url: Option<String>,
    },
    /// Indent list items, or move to the next table cell.
    Indent,
    /// Outdent list items, or move to the previous table cell.
    Outdent,
    Table(TableOp),
    /// Ctrl+Shift+V: CSV/TSV text (the app reads the clipboard) as a table.
    PasteTable {
        text: String,
    },
}

macro_rules! doc_commands {
    ($($id:literal, $title:literal, $icon:expr, $shortcut:expr, $slash:expr, $cmd:expr;)*) => {
        const REGISTRY: &[CommandMeta] = &[
            $(CommandMeta { id: CommandId($id), title: $title, icon: $icon, shortcut: $shortcut, slash: $slash }),*
        ];

        impl DocCommand {
            /// The command with registry id `id` (default arguments), e.g.
            /// `"format.bold"`.
            pub fn from_id(id: &str) -> Option<DocCommand> {
                use DocCommand::*;
                use TableOp::*;
                match id {
                    $($id => Some($cmd),)*
                    _ => None,
                }
            }
        }
    };
}

// DESIGN_SYSTEM 8.2. Ctrl+Shift+B is the app's sidebar toggle. Indent and
// outdent have no default shortcut: the editor routes Tab through
// `smart_input` and Shift+Tab to `DocCommand::Outdent` itself.
doc_commands! {
    "format.bold", "Bold", Some("bold"), Some("Ctrl+B"), None, ToggleBold;
    "format.italic", "Italic", Some("italic"), Some("Ctrl+I"), None, ToggleItalic;
    "format.strike", "Strikethrough", Some("strikethrough"), Some("Ctrl+Shift+X"), None, ToggleStrike;
    "format.code", "Inline Code", Some("code"), Some("Ctrl+`"), None, ToggleInlineCode;
    "format.link", "Link", Some("link"), Some("Ctrl+K"), Some("link"), Link { url: None };
    "format.paragraph", "Paragraph", Some("pilcrow"), Some("Ctrl+Alt+0"), None, SetHeading(0);
    "format.h1", "Heading 1", Some("heading-1"), Some("Ctrl+Alt+1"), Some("h1"), SetHeading(1);
    "format.h2", "Heading 2", Some("heading-2"), Some("Ctrl+Alt+2"), Some("h2"), SetHeading(2);
    "format.h3", "Heading 3", Some("heading-3"), Some("Ctrl+Alt+3"), Some("h3"), SetHeading(3);
    "format.h4", "Heading 4", Some("heading-4"), Some("Ctrl+Alt+4"), Some("h4"), SetHeading(4);
    "format.h5", "Heading 5", Some("heading-5"), Some("Ctrl+Alt+5"), Some("h5"), SetHeading(5);
    "format.h6", "Heading 6", Some("heading-6"), Some("Ctrl+Alt+6"), Some("h6"), SetHeading(6);
    "format.bullet_list", "Bullet List", Some("list"), Some("Ctrl+Shift+8"), Some("list"), BulletList;
    "format.numbered_list", "Numbered List", Some("list-ordered"), Some("Ctrl+Shift+7"), Some("numbered"), NumberedList;
    "format.task_list", "Task List", Some("list-todo"), Some("Ctrl+Shift+9"), Some("todo"), TaskList;
    "format.quote", "Quote", Some("quote"), Some("Ctrl+Shift+."), Some("quote"), Quote;
    "insert.code_block", "Code Block", Some("square-code"), Some("Ctrl+Alt+C"), Some("code"), CodeBlock;
    "insert.mermaid", "Mermaid Diagram", Some("square-code"), None, Some("mermaid"), Mermaid;
    "insert.table", "Table", Some("table"), Some("Ctrl+Alt+T"), Some("table"), InsertTable { rows: 3, cols: 3 };
    "insert.image", "Image", Some("image"), Some("Ctrl+Alt+I"), Some("image"), Image { url: None };
    "insert.hr", "Horizontal Rule", Some("minus"), None, Some("hr"), HorizontalRule;
    "edit.indent", "Indent", Some("indent-increase"), None, None, Indent;
    "edit.outdent", "Outdent", Some("indent-decrease"), None, None, Outdent;
    "edit.paste_table", "Paste as Table", Some("table"), Some("Ctrl+Shift+V"), None, PasteTable { text: String::new() };
    "table.format", "Table: Align Columns", Some("table"), None, None, Table(Format);
    "table.insert_row_above", "Table: Insert Row Above", None, None, None, Table(InsertRowAbove);
    "table.insert_row_below", "Table: Insert Row Below", None, None, None, Table(InsertRowBelow);
    "table.insert_column_left", "Table: Insert Column Left", None, None, None, Table(InsertColumnLeft);
    "table.insert_column_right", "Table: Insert Column Right", None, None, None, Table(InsertColumnRight);
    "table.delete_row", "Table: Delete Row", Some("trash"), None, None, Table(DeleteRow);
    "table.delete_column", "Table: Delete Column", Some("trash"), None, None, Table(DeleteColumn);
    "table.move_row_up", "Table: Move Row Up", Some("chevron-up"), None, None, Table(MoveRowUp);
    "table.move_row_down", "Table: Move Row Down", Some("chevron-down"), None, None, Table(MoveRowDown);
    "table.move_column_left", "Table: Move Column Left", Some("chevron-left"), None, None, Table(MoveColumnLeft);
    "table.move_column_right", "Table: Move Column Right", Some("chevron-right"), None, None, Table(MoveColumnRight);
    "table.align_none", "Table: Default Alignment", None, None, None, Table(Align(ColumnAlign::None));
    "table.align_left", "Table: Align Column Left", Some("align-left"), None, None, Table(Align(ColumnAlign::Left));
    "table.align_center", "Table: Align Column Center", Some("align-center"), None, None, Table(Align(ColumnAlign::Center));
    "table.align_right", "Table: Align Column Right", Some("align-right"), None, None, Table(Align(ColumnAlign::Right));
}

impl DocCommand {
    /// The registry id of this command, when it is the registry's form of
    /// it (`Link { url: Some(..) }` has none).
    pub fn id(&self) -> Option<CommandId> {
        REGISTRY.iter().find(|m| DocCommand::from_id(m.id.0).as_ref() == Some(self)).map(|m| m.id)
    }
}

/// The edits a command makes, plus the selection afterwards. The caller
/// applies the edits through `Document::apply` under one `TxnId`.
///
/// Edits are ordered last offset first and do not overlap, so applying them
/// in order (each to the result of the previous one) is the same as
/// applying them all to the original text. Their `txn` is a placeholder and
/// their origin is `EditOrigin::Command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub edits: Vec<Edit>,
    pub selection: Selection,
}

impl CommandResult {
    fn unchanged(selection: Selection) -> Self {
        Self { edits: Vec::new(), selection }
    }
}

/// Run a document command against `doc` with `selection`.
pub fn run(doc: &Document, selection: Selection, command: &DocCommand) -> CommandResult {
    use DocCommand::*;
    use inline::Inline;
    let sel = clamp(doc, selection);
    match command {
        ToggleBold => inline::toggle(doc, sel, Inline::Bold),
        ToggleItalic => inline::toggle(doc, sel, Inline::Italic),
        ToggleStrike => inline::toggle(doc, sel, Inline::Strike),
        ToggleInlineCode => inline::toggle(doc, sel, Inline::Code),
        Link { url } => inline::link(doc, sel, url.as_deref()),
        Image { url } => inline::image(doc, sel, url.as_deref()),
        SetHeading(level) => block::set_heading(doc, sel, *level),
        BulletList => block::toggle_list(doc, sel, block::List::Bullet),
        NumberedList => block::toggle_list(doc, sel, block::List::Numbered),
        TaskList => block::toggle_list(doc, sel, block::List::Task),
        Quote => block::toggle_quote(doc, sel),
        CodeBlock => block::toggle_code_block(doc, sel),
        Mermaid => block::mermaid(doc, sel),
        HorizontalRule => block::horizontal_rule(doc, sel),
        InsertTable { rows, cols } => match table::align_at(doc, sel.head, sel) {
            Some(r) => r,
            None => table::insert_table(doc, sel, *rows, *cols),
        },
        Indent => smart::indent(doc, sel).unwrap_or(CommandResult::unchanged(sel)),
        Outdent => smart::outdent(doc, sel).unwrap_or(CommandResult::unchanged(sel)),
        Table(op) => table::run_op(doc, sel, *op).unwrap_or(CommandResult::unchanged(sel)),
        PasteTable { text } => table::paste_table(doc, sel, text),
    }
}

fn clamp(doc: &Document, sel: Selection) -> Selection {
    let len = doc.buffer().len_bytes();
    Selection { anchor: sel.anchor.min(len), head: sel.head.min(len) }
}

/// Smart handling of text the user types over `selection` (DESIGN_SYSTEM
/// 8.3): `"\n"` continues or ends a list item or quote, `"\t"` indents a
/// list item or moves to the next table cell, and a pairing character
/// (`*`, `**`, `_`, `` ` ``, `~`, `[`) typed over a selection wraps it.
///
/// `None` means the plain edit is right; apply it as usual. On `Some`, apply
/// the result's edits (instead of the plain edit) and its selection.
pub fn smart_input(doc: &Document, selection: Selection, text: &str) -> Option<CommandResult> {
    if doc.is_large() {
        return None;
    }
    let sel = clamp(doc, selection);
    match text {
        "\n" | "\r\n" => smart::enter(doc, sel),
        "\t" => smart::indent(doc, sel),
        _ => smart::wrap(sel, text),
    }
}

/// Pasting `text` over `selection`: a URL pasted onto a single-line
/// selection makes a link. `None` means paste as usual.
pub fn smart_paste(doc: &Document, selection: Selection, text: &str) -> Option<CommandResult> {
    inline::paste_url(doc, clamp(doc, selection), text)
}

/// The byte range of the GFM table containing byte `at`, if any. The app
/// remembers it while the cursor is inside and calls [`align_table_at`]
/// when the cursor leaves.
pub fn table_range_at(doc: &Document, at: usize) -> Option<Range<usize>> {
    table::table_at(doc.buffer(), at).map(|t| t.range)
}

/// Auto-align the table containing byte `at` (DESIGN_SYSTEM 8.4: on leaving
/// the table). `selection` is mapped through the edits. `None` when there is
/// no table at `at`; no edits when it is already aligned.
pub fn align_table_at(doc: &Document, at: usize, selection: Selection) -> Option<CommandResult> {
    table::align_at(doc, at, clamp(doc, selection))
}

/// Metadata for every document command, in menu order.
pub fn registry() -> &'static [CommandMeta] {
    REGISTRY
}
