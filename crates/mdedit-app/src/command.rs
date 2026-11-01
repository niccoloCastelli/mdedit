//! UI command registry (ARCHITECTURE 5).
//!
//! Every action the shell offers is a command with a stable string id, a
//! title, an optional icon and a default shortcut. The menu bar, the command
//! palette and the keymap all read [`all`], so a command is defined once.
//! Document commands (formatting, tables) come from
//! `mdedit_core::command::registry()`; this module adds the UI-only ones
//! (files, tabs, panels, modes) and puts both in one list.

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
    OpenFolder,
    CloseFolder,
    ProjectNewFile,
    ProjectNewFolder,
    Checkpoint,
    Sync,
    NewBranch,
    FileHistory,
    Save,
    SaveAs,
    SaveAll,
    ExportPdf,
    ExportDocx,
    CloseTab,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FindInFiles,
    ModeEdit,
    ModeSplit,
    ModeView,
    CycleMode,
    ToggleSidebar,
    ModeRead,
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
    /// Lucide icon name, for the floating toolbar and slash menu (later).
    #[allow(dead_code)]
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
    OpenFolder, "file.open_folder", "Open Folder…", Some("folder-open"), None;
    CloseFolder, "file.close_folder", "Close Folder", None, None;
    ProjectNewFile, "project.new_file", "New File in Project…", Some("file-plus"), None;
    ProjectNewFolder, "project.new_folder", "New Folder…", Some("folder-plus"), None;
    Checkpoint, "git.checkpoint", "Checkpoint…", Some("tag"), Some("Ctrl+Alt+S");
    Sync, "git.sync", "Sync", Some("refresh-cw"), None;
    NewBranch, "git.new_branch", "New Branch…", Some("git-branch"), None;
    FileHistory, "git.file_history", "File History", Some("rotate-ccw-clock"), Some("Ctrl+Alt+H");
    Save, "file.save", "Save", Some("save"), Some("Ctrl+S");
    SaveAs, "file.save_as", "Save As…", None, Some("Ctrl+Shift+S");
    SaveAll, "file.save_all", "Save All", None, None;
    ExportPdf, "file.export_pdf", "Export as PDF…", Some("file-down"), Some("Ctrl+Shift+E");
    ExportDocx, "file.export_docx", "Export as DOCX…", Some("file-down"), None;
    CloseTab, "file.close_tab", "Close Tab", Some("x"), Some("Ctrl+W");
    Quit, "file.quit", "Quit", None, Some("Ctrl+Q");
    Undo, "edit.undo", "Undo", Some("undo"), Some("Ctrl+Z");
    Redo, "edit.redo", "Redo", Some("redo"), Some("Ctrl+Shift+Z");
    Cut, "edit.cut", "Cut", Some("scissors"), None;
    Copy, "edit.copy", "Copy", Some("copy"), None;
    Paste, "edit.paste", "Paste", Some("clipboard-paste"), None;
    SelectAll, "edit.select_all", "Select All", None, None;
    Find, "edit.find", "Find", Some("search"), Some("Ctrl+F");
    FindInFiles, "edit.find_in_files", "Find in Files", Some("search"), Some("Ctrl+Shift+F");
    ModeEdit, "view.mode_edit", "Edit Mode", Some("code"), Some("Ctrl+1");
    ModeSplit, "view.mode_split", "Split Mode", Some("columns-2"), Some("Ctrl+2");
    ModeView, "view.mode_view", "View Mode", Some("eye"), Some("Ctrl+3");
    CycleMode, "view.cycle_mode", "Cycle Mode", None, Some("Ctrl+E");
    ModeRead, "view.mode_read", "Read Mode (No Editing)", Some("lock"), Some("Ctrl+4");
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

/// Default shortcuts the app adds to document commands that have none in
/// the core registry (Tab itself is typed into the editor and handled by
/// smart input).
const EXTRA_SHORTCUTS: &[(&str, &str)] = &[("edit.outdent", "Shift+Tab")];

/// Keys the document widgets handle themselves. They are not in the keymap
/// (the widget would see them first) but menus still show them.
const WIDGET_SHORTCUTS: &[(&str, &str)] =
    &[("edit.cut", "Ctrl+X"), ("edit.copy", "Ctrl+C"), ("edit.paste", "Ctrl+V"), ("edit.select_all", "Ctrl+A")];

pub fn widget_shortcut(id: &str) -> Option<&'static str> {
    WIDGET_SHORTCUTS.iter().find(|(i, _)| *i == id).map(|(_, s)| *s)
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
        for meta in core_cmd::registry() {
            let extra = EXTRA_SHORTCUTS.iter().find(|(id, _)| *id == meta.id.0).map(|(_, s)| *s);
            out.push(CommandEntry {
                id: meta.id.0,
                title: meta.title,
                icon: meta.icon,
                shortcut: meta.shortcut.or(extra),
                command: Command::Doc(meta.command()),
            });
        }
        out
    })
}

pub fn find(id: &str) -> Option<&'static CommandEntry> {
    all().iter().find(|c| c.id == id)
}

/// The menu bar layout (DESIGN_SYSTEM 6). `None` is a separator.
pub fn menus() -> &'static [(&'static str, &'static [Option<&'static str>])] {
    &[
        (
            "File",
            &[
                Some("file.new"),
                Some("file.open"),
                Some("file.open_folder"),
                Some("view.quick_open"),
                None,
                Some("file.save"),
                Some("file.save_as"),
                Some("file.save_all"),
                None,
                Some("file.export_pdf"),
                Some("file.export_docx"),
                None,
                Some("file.close_tab"),
                Some("file.close_folder"),
                Some("file.quit"),
            ],
        ),
        (
            "Edit",
            &[
                Some("edit.undo"),
                Some("edit.redo"),
                None,
                Some("edit.cut"),
                Some("edit.copy"),
                Some("edit.paste"),
                Some("edit.select_all"),
                None,
                Some("edit.find"),
                Some("edit.find_in_files"),
                None,
                Some("edit.paste_table"),
                Some("edit.indent"),
                Some("edit.outdent"),
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
        (
            "Insert",
            &[
                Some("insert.image"),
                Some("insert.code_block"),
                Some("insert.table"),
                Some("insert.hr"),
                None,
                Some("table.format"),
                Some("table.insert_row_below"),
                Some("table.insert_column_right"),
                Some("table.delete_row"),
                Some("table.delete_column"),
            ],
        ),
        (
            "View",
            &[
                Some("view.mode_edit"),
                Some("view.mode_split"),
                Some("view.mode_view"),
                Some("view.mode_read"),
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
        (
            "Project",
            &[
                Some("project.new_file"),
                Some("project.new_folder"),
                None,
                Some("git.checkpoint"),
                Some("git.sync"),
                Some("git.new_branch"),
                Some("git.file_history"),
                None,
                Some("view.show_files"),
                Some("view.show_git"),
            ],
        ),
        ("Help", &[Some("app.open_settings"), Some("app.open_keymap"), None, Some("help.about")]),
    ]
}

/// The document's right-click menu. Table rows only appear inside a table.
pub fn context_menu(in_table: bool) -> Vec<Option<&'static str>> {
    let mut items = vec![
        Some("edit.cut"),
        Some("edit.copy"),
        Some("edit.paste"),
        Some("edit.paste_table"),
        None,
        Some("format.bold"),
        Some("format.italic"),
        Some("format.code"),
        Some("format.link"),
    ];
    if in_table {
        items.extend([
            None,
            Some("table.insert_row_below"),
            Some("table.insert_column_right"),
            Some("table.delete_row"),
            Some("table.delete_column"),
        ]);
    }
    items.extend([None, Some("edit.select_all")]);
    items
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
        let context = context_menu(true);
        for items in menus().iter().map(|(_, items)| *items).chain([context.as_slice()]) {
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
