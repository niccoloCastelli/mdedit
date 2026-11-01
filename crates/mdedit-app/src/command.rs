//! UI command registry (ARCHITECTURE 5).
//!
//! Every action the shell offers is a command with a stable string id, a
//! title, an optional icon and a default shortcut. The menu bar, the command
//! palette and the keymap all read [`all`], so a command is defined once.
//! Document commands (formatting) come from `mdedit_core::command`; this
//! module adds the UI-only ones (files, tabs, panels, modes) and maps both
//! into one list.

use mdedit_core::command::{self as core_cmd, DocCommand};

/// What running a command does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    App(AppCommand),
    Doc(DocCommand),
}

/// Commands handled by the shell itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppCommand {
    NewFile,
    Open,
    Save,
    SaveAs,
    SaveAll,
    CloseTab,
    Quit,
    Undo,
    Redo,
    Find,
    FindInFiles,
    ModeEdit,
    ModeSplit,
    ModeView,
    CycleMode,
    ToggleSidebar,
    ToggleComments,
    ShowFiles,
    ShowOutline,
    ShowSearch,
    ShowGit,
    Palette,
    QuickOpen,
    NextTab,
    PrevTab,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleFullWidth,
    ToggleLineNumbers,
    AppearanceSystem,
    AppearanceLight,
    AppearanceDark,
    OpenSettings,
    OpenKeymap,
    About,
}

/// Static description of a command for menus, palette and keymap.
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// Stable id used in `keymap.toml`, e.g. `"file.save"`.
    pub id: &'static str,
    pub title: &'static str,
    /// Lucide icon name.
    pub icon: Option<&'static str>,
    /// Default shortcut in keymap syntax (`Ctrl` is Cmd on macOS).
    pub shortcut: Option<&'static str>,
    pub command: Command,
}

macro_rules! app_commands {
    ($($variant:ident, $id:literal, $title:literal, $icon:expr, $shortcut:expr;)*) => {
        const APP: &[(AppCommand, &str, &str, Option<&str>, Option<&str>)] = &[
            $((AppCommand::$variant, $id, $title, $icon, $shortcut)),*
        ];
    };
}

// Ctrl+B is bold (DESIGN_SYSTEM 8.2), so the sidebar toggle moves to
// Ctrl+Shift+B. Ctrl+Alt+M toggles the comments panel until adding a
// comment (same shortcut) exists; that command will open the panel too.
app_commands! {
    NewFile, "file.new", "New File", Some("file-plus"), Some("Ctrl+N");
    Open, "file.open", "Open File…", Some("folder-open"), Some("Ctrl+O");
    Save, "file.save", "Save", Some("save"), Some("Ctrl+S");
    SaveAs, "file.save_as", "Save As…", None, Some("Ctrl+Shift+S");
    SaveAll, "file.save_all", "Save All", None, None;
    CloseTab, "file.close_tab", "Close Tab", Some("x"), Some("Ctrl+W");
    Quit, "file.quit", "Quit", None, Some("Ctrl+Q");
    Undo, "edit.undo", "Undo", Some("undo"), Some("Ctrl+Z");
    Redo, "edit.redo", "Redo", Some("redo"), Some("Ctrl+Shift+Z");
    Find, "edit.find", "Find", Some("search"), Some("Ctrl+F");
    FindInFiles, "edit.find_in_files", "Find in Files", Some("search"), Some("Ctrl+Shift+F");
    ModeEdit, "view.mode_edit", "Edit Mode", Some("code"), Some("Ctrl+1");
    ModeSplit, "view.mode_split", "Split Mode", Some("columns-2"), Some("Ctrl+2");
    ModeView, "view.mode_view", "View Mode", Some("eye"), Some("Ctrl+3");
    CycleMode, "view.cycle_mode", "Cycle Mode", None, Some("Ctrl+E");
    ToggleSidebar, "view.toggle_sidebar", "Toggle Sidebar", Some("panel-left"), Some("Ctrl+Shift+B");
    ToggleComments, "view.toggle_comments", "Toggle Comments Panel", Some("message-square"), Some("Ctrl+Alt+M");
    ShowFiles, "view.show_files", "Show Files", Some("files"), None;
    ShowOutline, "view.show_outline", "Show Outline", Some("list-tree"), Some("Ctrl+Shift+O");
    ShowSearch, "view.show_search", "Show Search", Some("search"), None;
    ShowGit, "view.show_git", "Show Git", Some("git-branch"), Some("Ctrl+Shift+G");
    Palette, "view.palette", "Command Palette", Some("command"), Some("Ctrl+Shift+P");
    QuickOpen, "view.quick_open", "Go to File…", Some("file-search"), Some("Ctrl+P");
    NextTab, "view.next_tab", "Next Tab", None, Some("Ctrl+Tab");
    PrevTab, "view.prev_tab", "Previous Tab", None, Some("Ctrl+Shift+Tab");
    ZoomIn, "view.zoom_in", "Zoom In", Some("zoom-in"), Some("Ctrl+=");
    ZoomOut, "view.zoom_out", "Zoom Out", Some("zoom-out"), Some("Ctrl+-");
    ZoomReset, "view.zoom_reset", "Reset Zoom", None, Some("Ctrl+0");
    ToggleFullWidth, "view.toggle_full_width", "Toggle Full Width", None, None;
    ToggleLineNumbers, "view.toggle_line_numbers", "Toggle Line Numbers", None, None;
    AppearanceSystem, "view.appearance_system", "Appearance: System", None, None;
    AppearanceLight, "view.appearance_light", "Appearance: Light", Some("sun"), None;
    AppearanceDark, "view.appearance_dark", "Appearance: Dark", Some("moon"), None;
    OpenSettings, "app.open_settings", "Open Settings File", Some("settings"), Some("Ctrl+,");
    OpenKeymap, "app.open_keymap", "Open Keymap File", Some("keyboard"), None;
    About, "help.about", "About mdedit", Some("info"), None;
}

/// Document commands and their ids, matching DESIGN_SYSTEM 8.2. Metadata
/// from `mdedit_core::command::registry()` wins when it has the same id;
/// these are the fallback until the commands workstream fills it in.
fn doc_commands() -> Vec<(&'static str, &'static str, Option<&'static str>, Option<&'static str>, DocCommand)> {
    use DocCommand::*;
    vec![
        ("format.bold", "Bold", Some("bold"), Some("Ctrl+B"), ToggleBold),
        ("format.italic", "Italic", Some("italic"), Some("Ctrl+I"), ToggleItalic),
        ("format.strike", "Strikethrough", Some("strikethrough"), Some("Ctrl+Shift+X"), ToggleStrike),
        ("format.code", "Inline Code", Some("code"), Some("Ctrl+`"), ToggleInlineCode),
        ("format.link", "Link", Some("link"), Some("Ctrl+K"), Link { url: None }),
        ("format.paragraph", "Paragraph", Some("pilcrow"), Some("Ctrl+Alt+0"), SetHeading(0)),
        ("format.h1", "Heading 1", Some("heading-1"), Some("Ctrl+Alt+1"), SetHeading(1)),
        ("format.h2", "Heading 2", Some("heading-2"), Some("Ctrl+Alt+2"), SetHeading(2)),
        ("format.h3", "Heading 3", Some("heading-3"), Some("Ctrl+Alt+3"), SetHeading(3)),
        ("format.h4", "Heading 4", Some("heading-4"), Some("Ctrl+Alt+4"), SetHeading(4)),
        ("format.h5", "Heading 5", Some("heading-5"), Some("Ctrl+Alt+5"), SetHeading(5)),
        ("format.h6", "Heading 6", Some("heading-6"), Some("Ctrl+Alt+6"), SetHeading(6)),
        ("format.bullet_list", "Bullet List", Some("list"), Some("Ctrl+Shift+8"), BulletList),
        ("format.numbered_list", "Numbered List", Some("list-ordered"), Some("Ctrl+Shift+7"), NumberedList),
        ("format.task_list", "Task List", Some("list-checks"), Some("Ctrl+Shift+9"), TaskList),
        ("format.quote", "Quote", Some("quote"), Some("Ctrl+Shift+."), Quote),
        ("insert.code_block", "Code Block", Some("square-code"), Some("Ctrl+Alt+C"), CodeBlock),
        ("insert.table", "Table", Some("table"), Some("Ctrl+Alt+T"), InsertTable { rows: 3, cols: 3 }),
        ("insert.hr", "Horizontal Rule", Some("minus"), None, HorizontalRule),
    ]
}

/// Every command, UI commands first. Built once.
pub fn all() -> &'static [CommandEntry] {
    use std::sync::OnceLock;
    static ALL: OnceLock<Vec<CommandEntry>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut out: Vec<CommandEntry> = APP
            .iter()
            .map(|&(cmd, id, title, icon, shortcut)| CommandEntry {
                id,
                title,
                icon,
                shortcut,
                command: Command::App(cmd),
            })
            .collect();
        let core = core_cmd::registry();
        for (id, title, icon, shortcut, command) in doc_commands() {
            let meta = core.iter().find(|m| m.id.0 == id);
            out.push(CommandEntry {
                id,
                title: meta.map_or(title, |m| m.title),
                icon: meta.map_or(icon, |m| m.icon),
                shortcut: meta.map_or(shortcut, |m| m.shortcut),
                command: Command::Doc(command),
            });
        }
        out
    })
}

pub fn find(id: &str) -> Option<&'static CommandEntry> {
    all().iter().find(|c| c.id == id)
}

pub fn entry_for(cmd: AppCommand) -> &'static CommandEntry {
    all().iter().find(|c| c.command == Command::App(cmd)).expect("every AppCommand is registered")
}

/// The menu bar layout (DESIGN_SYSTEM 6). `None` is a separator.
pub fn menus() -> &'static [(&'static str, &'static [Option<&'static str>])] {
    &[
        (
            "File",
            &[
                Some("file.new"),
                Some("file.open"),
                Some("view.quick_open"),
                None,
                Some("file.save"),
                Some("file.save_as"),
                Some("file.save_all"),
                None,
                Some("file.close_tab"),
                Some("file.quit"),
            ],
        ),
        (
            "Edit",
            &[
                Some("edit.undo"),
                Some("edit.redo"),
                None,
                Some("edit.find"),
                Some("edit.find_in_files"),
                None,
                Some("view.palette"),
            ],
        ),
        (
            "Format",
            &[
                Some("format.bold"),
                Some("format.italic"),
                Some("format.strike"),
                Some("format.code"),
                Some("format.link"),
                None,
                Some("format.paragraph"),
                Some("format.h1"),
                Some("format.h2"),
                Some("format.h3"),
                None,
                Some("format.bullet_list"),
                Some("format.numbered_list"),
                Some("format.task_list"),
                Some("format.quote"),
            ],
        ),
        ("Insert", &[Some("insert.code_block"), Some("insert.table"), Some("insert.hr")]),
        (
            "View",
            &[
                Some("view.mode_edit"),
                Some("view.mode_split"),
                Some("view.mode_view"),
                Some("view.cycle_mode"),
                None,
                Some("view.toggle_sidebar"),
                Some("view.show_outline"),
                Some("view.toggle_comments"),
                None,
                Some("view.zoom_in"),
                Some("view.zoom_out"),
                Some("view.zoom_reset"),
                Some("view.toggle_full_width"),
                Some("view.toggle_line_numbers"),
                None,
                Some("view.appearance_system"),
                Some("view.appearance_light"),
                Some("view.appearance_dark"),
            ],
        ),
        ("Project", &[Some("view.show_files"), Some("view.show_git")]),
        ("Help", &[Some("app.open_settings"), Some("app.open_keymap"), None, Some("help.about")]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique() {
        let mut seen = HashSet::new();
        for c in all() {
            assert!(seen.insert(c.id), "duplicate id {}", c.id);
        }
    }

    #[test]
    fn every_menu_item_exists() {
        for (_, items) in menus() {
            for id in items.iter().flatten() {
                assert!(find(id).is_some(), "menu references unknown command {id}");
            }
        }
    }

    #[test]
    fn every_default_shortcut_parses() {
        for c in all() {
            if let Some(s) = c.shortcut {
                assert!(crate::keymap::KeyChord::parse(s).is_some(), "{} has bad shortcut {s}", c.id);
            }
        }
    }
}
