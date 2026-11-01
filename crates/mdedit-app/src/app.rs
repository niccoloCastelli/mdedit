//! Application state and update loop.
//!
//! The shell owns the open tabs, panels, settings and keymap. Widgets
//! (editor, view) report actions; text changes go through
//! `Document::apply` (via [`Tab::apply`]) so the edit log sees everything.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::{Event, Point, Rectangle, Size, Subscription, Task, event, window};
use mdedit_core::command as core_cmd;
use mdedit_core::document::SavePoint;
use mdedit_core::parse::OutlineEntry;
use mdedit_core::search::{SearchQuery, find_all};
use mdedit_core::{Document, Edit, EditOrigin, Selection};
use mdedit_theme::Theme;

use crate::command::{self, AppCommand, Command};
use crate::comments::{CommentMsg, CommentsUi};
use crate::export_ui::{ExportMsg, ExportUi};
use crate::keymap::{self, KeyChord, Keymap, Overrides};
use crate::palette::{self, Palette, Scope};
use crate::session::{Session, TabSession};
use crate::settings::{AppearanceMode, Mode, Paths, Settings};
use crate::slash::{SlashMsg, SlashState};
use crate::tab::{DiskStamp, External, Handoff, Tab, TabId};
use crate::watch;

mod assets;
pub mod project;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Files,
    Outline,
    Search,
    Git,
}

impl SidebarTab {
    pub const ALL: [SidebarTab; 4] = [SidebarTab::Files, SidebarTab::Outline, SidebarTab::Search, SidebarTab::Git];

    pub fn label(self) -> &'static str {
        match self {
            SidebarTab::Files => "Files",
            SidebarTab::Outline => "Outline",
            SidebarTab::Search => "Search",
            SidebarTab::Git => "Git",
        }
    }

    fn key(self) -> &'static str {
        match self {
            SidebarTab::Files => "files",
            SidebarTab::Outline => "outline",
            SidebarTab::Search => "search",
            SidebarTab::Git => "git",
        }
    }

    fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|t| t.key() == key).unwrap_or(SidebarTab::Outline)
    }
}

/// A modal question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// Closing a tab with unsaved changes.
    UnsavedTab(TabId),
    /// Quitting with unsaved changes.
    UnsavedQuit(usize),
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogChoice {
    Save,
    Discard,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerAction {
    Reload,
    KeepMine,
    Close,
}

/// Word count and friends for the status bar, computed off the UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub words: usize,
    pub chars: usize,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Run a command by id (menus, palette, keymap).
    Run(&'static str),
    Key {
        key: Key,
        modifiers: Modifiers,
        captured: bool,
    },
    MenuOpen(Option<usize>),
    /// Right-click menu at a window position; `None` closes it.
    ContextMenu(Option<Point>),
    WindowResized(Size),
    /// 150 ms after the selection last moved: show the format toolbar.
    ToolbarTick,
    WindowId(Option<window::Id>),
    Window(WindowCmd),
    Maximized(bool),
    ToolbarHeading(bool),
    MenuHover(usize),
    SelectTab(usize),
    CloseTab(usize),
    Editor(TabId, mdedit_editor::Action),
    View(TabId, mdedit_view::Action),
    OpenPicked(Vec<PathBuf>),
    Loaded(PathBuf, Result<Handoff<Document>, String>),
    SaveAsPicked(TabId, Option<PathBuf>),
    Saved(TabId, PathBuf, SavePoint, Result<Handoff<Option<mdedit_core::mapped::Remap>>, String>),
    Paste(TabId, Option<String>),
    PasteTable(TabId, Option<String>),
    SetMode(Mode),
    /// Read mode from the status bar (the command toggles; this doesn't).
    SetRead,
    Sidebar(SidebarTab),
    /// Sidebar separator dragged to this window x.
    SidebarDrag(f32),
    SidebarDragEnd,
    /// Split separator dragged to this window x.
    SplitDrag(f32),
    OutlineJump(usize),
    SearchQuery(String),
    SearchJump(usize, usize),
    Palette(PaletteMsg),
    Banner(TabId, BannerAction),
    FileChanged(PathBuf),
    Dialog(DialogChoice),
    CloseRequested,
    SystemDark(bool),
    Stats(TabId, u64, Stats),
    Tick,
    SessionTick,
    /// A frame was drawn (only with `MDEDIT_TRACE`).
    Frame,
    /// Synthetic keystroke for `MDEDIT_TRACE_TYPE`.
    TraceType,
    Parsed(TabId, Handoff<mdedit_core::parse::ParseResult>),
    /// Project mode and git (`project.rs`).
    Project(project::Msg),
    OpenFolder(PathBuf),
    Export(ExportMsg),
    Comments(CommentMsg),
    Slash(SlashMsg),
}

/// Custom title bar actions (no OS decorations).
#[derive(Debug, Clone, Copy)]
pub enum WindowCmd {
    Drag,
    Minimize,
    ToggleMaximize,
    Resize(window::Direction),
}

#[derive(Debug, Clone)]
pub enum PaletteMsg {
    Query(String),
    Submit,
    Pick(usize),
    Close,
}

/// The floating format toolbar (DESIGN_SYSTEM 7): it follows the selection
/// rectangle the document widgets report and shows once it settles.
#[derive(Debug)]
pub struct FormatToolbar {
    pub rect: Option<Rectangle>,
    changed: Instant,
    pub visible: bool,
    /// Hidden by typing or Esc until the selection moves again.
    dismissed: bool,
    pub heading_open: bool,
}

impl Default for FormatToolbar {
    fn default() -> Self {
        Self { rect: None, changed: Instant::now(), visible: false, dismissed: false, heading_open: false }
    }
}

const TOOLBAR_DELAY: Duration = Duration::from_millis(150);

pub struct SidebarState {
    pub visible: bool,
    pub tab: SidebarTab,
    pub width: f32,
}

pub struct App {
    trace: Option<crate::trace::Trace>,
    pub paths: Paths,
    pub settings: Settings,
    keymap_file: Overrides,
    pub keymap: Keymap,
    system_dark: bool,
    themes: mdedit_theme::ThemeSet,
    pub theme: Theme,
    pub tabs: Vec<Tab>,
    pub active: usize,
    next_id: TabId,
    next_untitled: usize,
    pub sidebar: SidebarState,
    pub comments_visible: bool,
    pub menu_open: Option<usize>,
    pub context_menu: Option<Point>,
    pub toolbar: FormatToolbar,
    pub window_size: Size,
    window_id: Option<window::Id>,
    pub maximized: bool,
    /// Drawn by us instead of the OS; fixed at start-up.
    pub custom_title_bar: bool,
    pub palette: Option<Palette>,
    pub dialog: Option<Dialog>,
    pub export: ExportUi,
    pub comments: CommentsUi,
    pub slash: SlashState,
    pub toasts: Vec<Toast>,
    pub session: Session,
    /// Session tabs still loading, in their saved order.
    restoring: Vec<TabSession>,
    restore_active: Option<PathBuf>,
    /// Editor share of the Split view (DESIGN_SYSTEM 6: 50/50, draggable).
    pub split_ratio: f32,
    session_dirty: bool,
    quitting: bool,
    close_after_save: Vec<TabId>,
    pub search: SearchState,
    /// Per tab: outline and stats with the buffer version they were made at.
    pub outline: Vec<(TabId, (u64, u64), Vec<OutlineEntry>)>,
    pub stats: Vec<(TabId, u64, Stats)>,
    stats_pending: Vec<TabId>,
    stats_spawned: Option<Instant>,
    /// The open project folder (`project.rs`).
    pub project: Option<project::ProjectState>,
    /// Text-input dialog of the project and git commands.
    pub prompt: Option<project::Prompt>,
}

#[derive(Debug, Default)]
pub struct SearchState {
    pub query: String,
    pub results: Vec<(usize, usize, String)>,
    /// (tab, version) the results belong to.
    for_version: Option<(TabId, u64)>,
}

pub const PALETTE_INPUT: &str = "palette-input";
pub const SEARCH_INPUT: &str = "sidebar-search";
pub const EDITOR: &str = "editor";
pub const VIEW: &str = "view";
const MAX_SEARCH_RESULTS: usize = 500;

impl App {
    pub fn new() -> (Self, Task<Message>) {
        if crate::trace::enabled() {
            eprintln!("trace: app state created at {:?}", crate::trace::start().elapsed());
        }
        let paths = Paths::detect();
        let (settings, settings_err) = Settings::load(&paths.settings());
        let custom_title_bar = settings.appearance.custom_title_bar();
        let (keymap_file, keymap_err) = load_keymap_file(&paths.keymap());
        let keymap = Keymap::new([&settings.keymap, &keymap_file]);
        let session = if settings.files.restore_session {
            Session::load(&paths.session()).unwrap_or_default()
        } else {
            Session { recent: Session::load(&paths.session()).unwrap_or_default().recent, ..Default::default() }
        };
        let fresh = session.tabs.is_empty() && session.sidebar_width == 0.0;
        let mut app = Self {
            trace: (crate::trace::enabled() || crate::trace::auto_type() > 0)
                .then(|| crate::trace::Trace::new(crate::trace::auto_type())),
            sidebar: SidebarState {
                visible: if fresh { true } else { session.sidebar_visible },
                tab: SidebarTab::from_key(&session.sidebar_tab),
                width: if session.sidebar_width > 0.0 {
                    session.sidebar_width.clamp(mdedit_theme::space::SIDEBAR_MIN, mdedit_theme::space::SIDEBAR_MAX)
                } else {
                    mdedit_theme::space::SIDEBAR_WIDTH
                },
            },
            comments_visible: session.comments_visible,
            paths,
            settings,
            keymap_file,
            keymap,
            system_dark: false,
            themes: mdedit_theme::ThemeSet::builtin(),
            theme: Theme::ember_light(),
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            next_untitled: 1,
            menu_open: None,
            context_menu: None,
            toolbar: FormatToolbar::default(),
            window_size: Size::new(1200.0, 800.0),
            window_id: None,
            maximized: false,
            custom_title_bar,
            palette: None,
            dialog: None,
            export: ExportUi::default(),
            comments: CommentsUi::default(),
            slash: SlashState::default(),
            toasts: Vec::new(),
            restoring: session.tabs.clone(),
            restore_active: session.tabs.get(session.active).map(|t| t.path.clone()),
            session,
            split_ratio: 0.5,
            session_dirty: false,
            quitting: false,
            close_after_save: Vec::new(),
            search: SearchState::default(),
            outline: Vec::new(),
            stats: Vec::new(),
            stats_pending: Vec::new(),
            stats_spawned: None,
            project: None,
            prompt: None,
        };
        app.load_themes();
        app.apply_theme();
        for err in [settings_err, keymap_err].into_iter().flatten() {
            app.toast_error(err);
        }
        for w in app.keymap.warnings.clone() {
            app.toast_error(w);
        }

        let mut tasks = vec![
            iced::system::theme().map(|m| Message::SystemDark(m == iced::theme::Mode::Dark)),
            window::oldest().map(Message::WindowId),
            iced::widget::operation::focus(EDITOR),
        ];
        let threshold = app.settings.files.large_file_threshold();
        let mut to_open: Vec<PathBuf> = app.restoring.iter().map(|t| t.path.clone()).collect();
        let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        for a in &args {
            let p = absolute(a);
            if !to_open.contains(&p) {
                to_open.push(p);
            }
        }
        if let Some(last) = args.last() {
            app.restore_active = Some(absolute(last));
        }
        if let Some(dir) = app.session.project.clone().filter(|d| d.is_dir()) {
            tasks.push(Task::done(Message::OpenFolder(dir)));
        }
        if to_open.is_empty() {
            app.new_untitled();
        }
        for path in to_open {
            tasks.push(Task::perform(crate::tab::load(path.clone(), threshold), move |r| {
                Message::Loaded(path.clone(), r)
            }));
        }
        (app, Task::batch(tasks))
    }

    pub fn title(&self) -> String {
        match self.tabs.get(self.active) {
            Some(tab) => format!("{}{} - mdedit", if tab.is_dirty() { "● " } else { "" }, tab.title()),
            None => "mdedit".into(),
        }
    }

    pub fn iced_theme(&self) -> iced::Theme {
        crate::style::iced_theme(&self.theme)
    }

    pub fn scale_factor(&self) -> f32 {
        self.settings.appearance.ui_scale.clamp(0.5, 3.0)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn is_dark(&self) -> bool {
        match self.settings.appearance.mode {
            AppearanceMode::System => self.system_dark,
            AppearanceMode::Light => false,
            AppearanceMode::Dark => true,
        }
    }

    fn apply_theme(&mut self) {
        let a = &self.settings.appearance;
        let wanted = mdedit_theme::ThemeSettings {
            appearance: if self.is_dark() { mdedit_theme::Appearance::Dark } else { mdedit_theme::Appearance::Light },
            light_theme: a.light_theme.clone(),
            dark_theme: a.dark_theme.clone(),
        };
        let id = if self.is_dark() { &a.dark_theme } else { &a.light_theme };
        if let Err(e) = self.themes.get(id) {
            let msg = e.to_string();
            self.toast_error(msg);
        }
        let mut theme = self.themes.select(&wanted, None);
        let f = &self.settings.fonts;
        for (setting, slot) in
            [(&f.ui, &mut theme.fonts.ui), (&f.prose, &mut theme.fonts.prose), (&f.mono, &mut theme.fonts.mono)]
        {
            if !setting.is_empty() {
                *slot = setting.clone();
            }
        }
        self.theme = theme;
    }

    /// Built-in themes plus `themes/*.toml` in the config dir.
    fn load_themes(&mut self) {
        let mut set = mdedit_theme::ThemeSet::builtin();
        let errors = set.load_dir(&self.paths.config_dir.join("themes"));
        self.themes = set;
        for e in errors {
            self.toast_error(e.to_string());
        }
    }

    /// Push editor settings into every tab's widget state.
    fn apply_editor_settings(&mut self) {
        let e = &self.settings.editor;
        let zoom = self.settings.appearance.zoom;
        for tab in &mut self.tabs {
            tab.editor.line_numbers = e.line_numbers;
            tab.editor.monospace = e.monospace;
            tab.editor.full_width = e.full_width;
            for v in [&mut tab.view, &mut tab.preview] {
                v.full_width = e.full_width;
                v.zoom = zoom;
            }
        }
    }

    fn new_untitled(&mut self) {
        let mut doc = Document::from_text("");
        doc.mark_saved();
        let mut tab = Tab::new(self.next_id, doc, self.settings.editor.default_mode, self.next_untitled);
        tab.editor.line_numbers = self.settings.editor.line_numbers;
        tab.editor.monospace = self.settings.editor.monospace;
        tab.editor.full_width = self.settings.editor.full_width;
        for v in [&mut tab.view, &mut tab.preview] {
            v.full_width = self.settings.editor.full_width;
            v.zoom = self.settings.appearance.zoom;
        }
        self.next_id += 1;
        self.next_untitled += 1;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), false);
    }

    pub fn toast_error(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), true);
    }

    fn push_toast(&mut self, text: String, error: bool) {
        self.toasts.push(Toast { text, error, until: Instant::now() + Duration::from_secs(if error { 8 } else { 4 }) });
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            event::listen_with(|event, status, _| match event {
                Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    Some(Message::Key { key, modifiers, captured: status == event::Status::Captured })
                }
                Event::Window(window::Event::FileDropped(path)) => Some(Message::Project(project::Msg::Dropped(path))),
                _ => None,
            }),
            window::close_requests().map(|_| Message::CloseRequested),
            window::resize_events().map(|(_, size)| Message::WindowResized(size)),
            iced::system::theme_changes().map(|m| Message::SystemDark(m == iced::theme::Mode::Dark)),
            watch::changes(self.watched_paths()).map(Message::FileChanged),
        ];
        subs.extend(self.project_subscriptions());
        let stats_stale = self
            .active_tab()
            .is_some_and(|t| !self.stats.iter().any(|(id, v, _)| *id == t.id && *v == t.doc.buffer().version()));
        if !self.toasts.is_empty() || stats_stale {
            subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick));
        }
        if let Some(t) = &self.trace {
            subs.push(window::frames().map(|_| Message::Frame));
            if t.doc_shown && t.to_type > 0 {
                subs.push(iced::time::every(Duration::from_millis(30)).map(|_| Message::TraceType));
            }
        }
        if self.session_dirty {
            subs.push(iced::time::every(Duration::from_secs(2)).map(|_| Message::SessionTick));
        }
        subs.extend(self.export.subscription());
        Subscription::batch(subs)
    }

    fn watched_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.tabs.iter().filter_map(|t| t.doc.path.clone()).collect();
        v.extend(self.comment_files());
        v.sort();
        v.dedup();
        v
    }

    /// Window x and width of the document area, between the sidebar and
    /// the comments panel (for the Split separator).
    fn document_span(&self) -> (f32, f32) {
        let left = if self.sidebar.visible { self.sidebar.width + crate::separator::HIT } else { 0.0 };
        let right = if self.comments_visible { mdedit_theme::space::COMMENTS_WIDTH + 1.0 } else { 0.0 };
        (left, self.window_size.width - left - right)
    }

    /// Focus the widget that takes typing in the active tab's mode.
    pub(crate) fn focus_document(&self) -> Task<Message> {
        match self.active_tab().map(|t| t.mode) {
            Some(Mode::View) => iced::widget::operation::focus(VIEW),
            _ => iced::widget::operation::focus(EDITOR),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let focus_key = |a: &Self| {
            let menu = a.menu_open.is_some() || a.context_menu.is_some() || a.toolbar.visible;
            let dialog = a.modal_open();
            (a.active_tab().map(|t| (t.id, t.mode)), a.palette.is_some(), dialog, menu)
        };
        let before = focus_key(self);
        let jump = matches!(message, Message::OutlineJump(_) | Message::SearchJump(..));
        let is_edit = matches!(message, Message::Editor(_, mdedit_editor::Action::Edit(_)) | Message::TraceType);
        let started = Instant::now();
        let task = self.handle(message);
        self.align_left_table();
        let comment_focus = self.after_comments_update();
        self.after_slash_update();
        // Give the keyboard back to the document when the tab, mode or a
        // modal layer changes, and after jumping from the sidebar (unless a
        // comment box just opened and wants it).
        let after = focus_key(self);
        let task = if let Some(focus) = comment_focus {
            Task::batch([task, focus])
        } else if (before != after || jump) && !after.1 && !after.2 {
            Task::batch([task, self.focus_document()])
        } else {
            task
        };
        let derived = Task::batch([self.refresh_derived(), self.spawn_parse_jobs()]);
        if is_edit && let Some(t) = &mut self.trace {
            t.update_took(started.elapsed());
        }
        // Cursor moves and layout changes reach session.toml within a few
        // seconds, so a crash or kill loses little.
        if !self.session_dirty && self.restoring.is_empty() && !self.quitting {
            let snap = self.session_snapshot();
            self.session_dirty = snap != self.session;
        }
        Task::batch([task, derived])
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Run(id) => {
                self.menu_open = None;
                self.context_menu = None;
                self.toolbar.heading_open = false;
                return self.run(id);
            }
            Message::Key { key, modifiers, captured } => return self.on_key(key, modifiers, captured),
            Message::Project(m) => return self.on_project(m),
            Message::OpenFolder(dir) => return self.open_folder(dir),
            Message::MenuOpen(m) => self.menu_open = if self.menu_open == m { None } else { m },
            Message::ContextMenu(p) => {
                self.menu_open = None;
                self.context_menu = p.filter(|_| self.palette.is_none() && self.dialog.is_none());
            }
            Message::WindowResized(size) => {
                self.window_size = size;
                if let Some(id) = self.window_id {
                    return window::is_maximized(id).map(Message::Maximized);
                }
            }
            Message::WindowId(id) => self.window_id = id,
            Message::Maximized(m) => self.maximized = m,
            Message::Window(cmd) => {
                let Some(id) = self.window_id else { return Task::none() };
                return match cmd {
                    WindowCmd::Drag => window::drag(id),
                    WindowCmd::Minimize => window::minimize(id, true),
                    WindowCmd::ToggleMaximize => window::toggle_maximize(id),
                    WindowCmd::Resize(dir) => window::drag_resize(id, dir),
                };
            }
            Message::ToolbarTick => {
                let t = &mut self.toolbar;
                if t.rect.is_some() && !t.dismissed && t.changed.elapsed() >= TOOLBAR_DELAY {
                    t.visible = true;
                }
            }
            Message::ToolbarHeading(open) => self.toolbar.heading_open = open,
            Message::MenuHover(i) => {
                if self.menu_open.is_some() {
                    self.menu_open = Some(i);
                }
            }
            Message::SelectTab(i) => {
                if i < self.tabs.len() {
                    self.active = i;
                }
            }
            Message::CloseTab(i) => {
                if let Some(id) = self.tabs.get(i).map(|t| t.id) {
                    return self.request_close(id);
                }
            }
            Message::Editor(id, action) => {
                if let (Some(t), mdedit_editor::Action::Edit(_)) = (&mut self.trace, &action) {
                    t.edit();
                }
                return self.on_editor(id, action);
            }
            Message::TraceType => {
                let Some(t) = &mut self.trace else { return Task::none() };
                if t.to_type == 0 {
                    return Task::none();
                }
                t.to_type -= 1;
                let last = t.to_type == 0;
                let Some(tab) = self.active_tab() else { return Task::none() };
                let (id, at) = (tab.id, tab.selection().head);
                let edit = Edit::insert(at, "x", EditOrigin::User, mdedit_core::TxnId(0));
                let typed = self.handle(Message::Editor(id, mdedit_editor::Action::Edit(edit)));
                let moved = self.handle(Message::Editor(id, mdedit_editor::Action::Select(Selection::cursor(at + 1))));
                if last {
                    if let Some(t) = &self.trace {
                        t.report();
                    }
                    return iced::exit();
                }
                return Task::batch([typed, moved]);
            }
            Message::Frame => {
                let has_doc = self.active_tab().is_some_and(|t| t.doc.path.is_some());
                if let Some(t) = &mut self.trace {
                    t.frame(has_doc);
                }
            }
            Message::View(id, action) => return self.on_view(id, action),
            Message::OpenPicked(paths) => {
                let threshold = self.settings.files.large_file_threshold();
                let mut tasks = Vec::new();
                for path in paths {
                    let path = absolute(&path);
                    if let Some(i) = self.tabs.iter().position(|t| t.doc.path.as_deref() == Some(&path)) {
                        self.active = i;
                        continue;
                    }
                    if let Some(dir) = path.parent() {
                        self.session.last_dir = Some(dir.to_path_buf());
                    }
                    self.restore_active = Some(path.clone());
                    tasks.push(Task::perform(crate::tab::load(path.clone(), threshold), move |r| {
                        Message::Loaded(path.clone(), r)
                    }));
                }
                return Task::batch(tasks);
            }
            Message::Loaded(path, result) => self.on_loaded(path, result),
            Message::SaveAsPicked(id, Some(path)) => {
                if let Some(dir) = path.parent() {
                    self.session.last_dir = Some(dir.to_path_buf());
                }
                return self.start_save(id, Some(path));
            }
            Message::SaveAsPicked(_, None) => {
                // A cancelled save-as also cancels the quit or close waiting on it.
                self.quitting = false;
                self.close_after_save.clear();
            }
            Message::Saved(id, path, point, result) => return self.on_saved(id, path, point, result),
            Message::Paste(id, Some(text)) => {
                if let Some(tab) = self.tab_mut(id) {
                    if let Some(result) = core_cmd::smart_paste(&tab.doc, tab.selection(), &text) {
                        tab.apply_command(result);
                        return Task::none();
                    }
                    let range = tab.selection().range();
                    let txn = tab.doc.next_txn();
                    let end = range.start + text.len();
                    tab.apply(&Edit { range, text, origin: EditOrigin::User, txn });
                    tab.set_selection(Selection::cursor(end));
                }
            }
            Message::Paste(_, None) | Message::PasteTable(_, None) => {}
            Message::PasteTable(id, Some(text)) => {
                if let Some(tab) = self.tab_mut(id) {
                    let result = core_cmd::run(&tab.doc, tab.selection(), &core_cmd::DocCommand::PasteTable { text });
                    tab.apply_command(result);
                }
            }
            Message::SetMode(mode) => self.set_mode(mode),
            Message::SetRead => self.set_read(true),
            Message::Sidebar(t) => {
                if self.sidebar.visible && self.sidebar.tab == t {
                    self.sidebar.visible = false;
                } else {
                    self.sidebar.visible = true;
                    self.sidebar.tab = t;
                }
                if self.sidebar.visible && t == SidebarTab::Search {
                    return iced::widget::operation::focus(SEARCH_INPUT);
                }
            }
            Message::SidebarDrag(x) => {
                self.sidebar.width = x.clamp(mdedit_theme::space::SIDEBAR_MIN, mdedit_theme::space::SIDEBAR_MAX);
            }
            Message::SidebarDragEnd => self.save_session(),
            Message::SplitDrag(x) => {
                let (left, width) = self.document_span();
                if width > 0.0 {
                    self.split_ratio = ((x - left) / width).clamp(0.2, 0.8);
                }
            }
            Message::OutlineJump(byte) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.set_selection(Selection::cursor(byte));
                    tab.editor.scroll_to_byte = Some(byte);
                }
            }
            Message::SearchQuery(q) => {
                self.search.query = q;
                self.search.for_version = None;
            }
            Message::SearchJump(start, end) => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.set_selection(Selection { anchor: start, head: end });
                    tab.editor.scroll_to_byte = Some(start);
                }
            }
            Message::Palette(msg) => return self.on_palette(msg),
            Message::Banner(id, action) => self.on_banner(id, action),
            Message::FileChanged(path) => self.on_file_changed(&path),
            Message::Dialog(choice) => return self.on_dialog(choice),
            Message::Export(msg) => return self.on_export(msg),
            Message::Comments(msg) => return self.on_comments(msg),
            Message::Slash(msg) => return self.on_slash(msg),
            Message::CloseRequested => return self.run_app(AppCommand::Quit),
            Message::SystemDark(dark) => {
                self.system_dark = dark;
                self.apply_theme();
            }
            Message::Stats(id, version, stats) => {
                self.stats_pending.retain(|t| *t != id);
                self.stats.retain(|(t, _, _)| *t != id);
                self.stats.push((id, version, stats));
            }
            Message::Parsed(id, result) => {
                if let (Some(tab), Some(result)) = (self.tabs.iter_mut().find(|t| t.id == id), result.take()) {
                    // A stale result is dropped; the next job is taken below.
                    tab.doc.finish_parse(result);
                }
            }
            Message::SessionTick => {
                self.session_dirty = false;
                self.save_session();
            }
            Message::Tick => {
                let now = Instant::now();
                self.toasts.retain(|t| t.until > now);
            }
        }
        Task::none()
    }

    /// Auto-align a table in Edit mode when the cursor leaves it
    /// (DESIGN_SYSTEM 8.4).
    fn align_left_table(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if tab.doc.is_large() || tab.read_only() {
            return;
        }
        let sel = tab.selection();
        let now = core_cmd::table_range_at(&tab.doc, sel.head);
        if now.is_none()
            && let Some(old) = tab.table.take()
            && let Some(result) = core_cmd::align_table_at(&tab.doc, old.start, sel)
        {
            tab.apply_command(result);
        }
        tab.table = now;
    }

    /// Reparse windows over 64 KB (and the initial parse of an opened
    /// file) run on a worker; the previous blocks stay shown meanwhile.
    fn spawn_parse_jobs(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for tab in &mut self.tabs {
            if let Some(job) = tab.doc.take_parse_job() {
                let id = tab.id;
                tasks.push(Task::perform(
                    async move { tokio::task::spawn_blocking(move || job.run()).await.ok() },
                    move |r| match r {
                        Some(r) => Message::Parsed(id, Handoff::new(r)),
                        None => Message::Tick,
                    },
                ));
            }
        }
        Task::batch(tasks)
    }

    /// Keep cheap derived data (outline, search results, stats) in step with
    /// the active buffer. Expensive work (stats) runs on a worker.
    fn refresh_derived(&mut self) -> Task<Message> {
        self.outline.retain(|(id, _, _)| self.tabs.iter().any(|t| t.id == *id));
        self.stats.retain(|(id, _, _)| self.tabs.iter().any(|t| t.id == *id));
        let Some(tab) = self.tabs.get(self.active) else { return Task::none() };
        let (id, version) = (tab.id, tab.doc.buffer().version());

        if self.sidebar.visible && self.sidebar.tab == SidebarTab::Outline {
            let key = (version, tab.doc.blocks().map_or(0, |b| b.generation()));
            let fresh = self.outline.iter().any(|(t, k, _)| *t == id && *k == key);
            if !fresh {
                let entries = match tab.doc.blocks() {
                    Some(b) => mdedit_core::parse::outline(b, tab.doc.buffer()),
                    // Large files: `scan_outline` on a worker, with large-file mode.
                    None => Vec::new(),
                };
                self.outline.retain(|(t, _, _)| *t != id);
                self.outline.push((id, key, entries));
            }
        }

        if self.sidebar.visible
            && self.sidebar.tab == SidebarTab::Search
            // Large files need the streamed background search (M2).
            && !tab.doc.is_large()
            && self.search.for_version != Some((id, version))
        {
            let query = SearchQuery { pattern: self.search.query.clone(), ..Default::default() };
            self.search.results = match find_all(tab.doc.buffer(), &query) {
                Ok(matches) => matches
                    .into_iter()
                    .take(MAX_SEARCH_RESULTS)
                    .map(|m| {
                        let buf = tab.doc.buffer();
                        let line = buf.byte_to_line(m.range.start);
                        let text = buf.line(line).trim().chars().take(80).collect::<String>();
                        (m.range.start, m.range.end, format!("{}: {text}", line + 1))
                    })
                    .collect(),
                Err(e) => vec![(0, 0, e.to_string())],
            };
            self.search.for_version = Some((id, version));
        }

        let stale = !self.stats.iter().any(|(t, v, _)| *t == id && *v == version);
        // At most one count per second while typing; the subscription's
        // tick picks up the last change once typing stops.
        let due = self.stats_spawned.is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
        if stale && due && !self.stats_pending.contains(&id) {
            self.stats_pending.push(id);
            self.stats_spawned = Some(Instant::now());
            let snapshot = tab.doc.buffer().snapshot();
            return Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || count_stats(snapshot.chunks(0..snapshot.len_bytes())))
                        .await
                        .unwrap_or_default()
                },
                move |stats| Message::Stats(id, version, stats),
            );
        }
        Task::none()
    }

    fn on_key(&mut self, key: Key, modifiers: Modifiers, captured: bool) -> Task<Message> {
        if let Some(task) = self.export_key(&key) {
            return task;
        }
        if let Some(task) = self.comments_key(&key, modifiers) {
            return task;
        }
        if let Some(task) = self.slash_key(&key) {
            return task;
        }
        if key == Key::Named(Named::Escape) {
            if self.toolbar.visible {
                self.toolbar.visible = false;
                self.toolbar.heading_open = false;
                self.toolbar.dismissed = true;
                return Task::none();
            }
            if self.palette.take().is_some() || self.menu_open.take().is_some() || self.context_menu.take().is_some() {
                return Task::none();
            }
            if self.dialog.is_some() {
                return self.on_dialog(DialogChoice::Cancel);
            }
            self.prompt = None;
            if let Some(p) = &mut self.project {
                p.history = None;
            }
            return Task::none();
        }
        if self.palette.is_some() {
            let delta = match key {
                Key::Named(Named::ArrowDown) => 1,
                Key::Named(Named::ArrowUp) => -1,
                _ => 0,
            };
            if delta != 0 {
                let len = self.palette_items().len().min(palette::MAX_ROWS);
                if let Some(p) = &mut self.palette {
                    p.move_selection(delta, len);
                }
                return Task::none();
            }
        }
        if self.prompt.is_some() {
            if key == Key::Named(Named::Enter) && !captured {
                return self.on_project(project::Msg::Prompt(project::PromptMsg::Submit));
            }
            return Task::none();
        }
        if self.dialog.is_some() {
            if key == Key::Named(Named::Enter) && !captured {
                return self.on_dialog(DialogChoice::Save);
            }
            return Task::none();
        }
        // Plain typing belongs to whatever widget took it.
        if captured && !(modifiers.command() || modifiers.alt()) {
            return Task::none();
        }
        let Some(chord) = KeyChord::from_event(&key, modifiers) else { return Task::none() };
        if std::env::var_os("MDEDIT_DEBUG_KEYS").is_some() {
            eprintln!("key {key:?} {modifiers:?} captured={captured} -> {chord} {:?}", self.keymap.lookup(&chord));
        }
        if let Some(task) = self.paste_image_key(&key, modifiers) {
            return task;
        }
        match self.keymap.lookup(&chord) {
            Some(id) => self.run(id),
            None => Task::none(),
        }
    }

    pub fn run(&mut self, id: &str) -> Task<Message> {
        let Some(entry) = command::find(id) else { return Task::none() };
        match &entry.command {
            Command::App(cmd) => self.run_app(*cmd),
            Command::Doc(cmd) => {
                let Some(tab) = self.active_tab_mut() else { return Task::none() };
                if !tab.editable() {
                    return Task::none();
                }
                if let core_cmd::DocCommand::Image { url: None } = cmd {
                    return self.pick_image();
                }
                if let core_cmd::DocCommand::PasteTable { .. } = cmd {
                    let id = tab.id;
                    return iced::clipboard::read().map(move |t| Message::PasteTable(id, t));
                }
                let result = core_cmd::run(&tab.doc, tab.selection(), cmd);
                tab.apply_command(result);
                Task::none()
            }
        }
    }

    fn run_app(&mut self, cmd: AppCommand) -> Task<Message> {
        use AppCommand::*;
        match cmd {
            NewFile => self.new_untitled(),
            Open => {
                let dir = self.session.last_dir.clone();
                return Task::perform(
                    async move {
                        let mut d = rfd::AsyncFileDialog::new()
                            .add_filter("Markdown", &["md", "markdown", "mdown", "txt"])
                            .add_filter("All files", &["*"]);
                        if let Some(dir) = dir {
                            d = d.set_directory(dir);
                        }
                        d.pick_files()
                            .await
                            .map(|v| v.iter().map(|h| h.path().to_path_buf()).collect())
                            .unwrap_or_default()
                    },
                    Message::OpenPicked,
                );
            }
            Save => {
                if let Some(tab) = self.active_tab() {
                    let id = tab.id;
                    return if tab.doc.path.is_some() { self.start_save(id, None) } else { self.save_as(id) };
                }
            }
            SaveAs => {
                if let Some(id) = self.active_tab().map(|t| t.id) {
                    return self.save_as(id);
                }
            }
            ExportPdf => return self.on_export(ExportMsg::Open(mdedit_export::ExportFormat::Pdf)),
            ExportDocx => return self.on_export(ExportMsg::Open(mdedit_export::ExportFormat::Docx)),
            SaveAll => {
                let ids: Vec<TabId> =
                    self.tabs.iter().filter(|t| t.is_dirty() && t.doc.path.is_some()).map(|t| t.id).collect();
                return Task::batch(ids.into_iter().map(|id| self.start_save(id, None)));
            }
            CloseTab => {
                if let Some(id) = self.active_tab().map(|t| t.id) {
                    return self.request_close(id);
                }
            }
            Quit => {
                let dirty = self.tabs.iter().filter(|t| t.is_dirty()).count();
                if dirty == 0 {
                    return self.exit();
                }
                self.dialog = Some(Dialog::UnsavedQuit(dirty));
            }
            Undo | Redo => {
                if let Some(tab) = self.active_tab_mut() {
                    let sel = if cmd == Undo { tab.doc.undo() } else { tab.doc.redo() };
                    if let Some(sel) = sel {
                        tab.set_selection(sel);
                    }
                }
            }
            Cut | Copy => {
                let Some(tab) = self.active_tab_mut() else { return Task::none() };
                let range = tab.selection().range();
                if range.is_empty() {
                    return Task::none();
                }
                let text = tab.doc.buffer().slice(range.clone()).into_owned();
                if cmd == Cut && tab.editable() {
                    let txn = tab.doc.next_txn();
                    tab.apply(&Edit { range: range.clone(), text: String::new(), origin: EditOrigin::User, txn });
                    tab.set_selection(Selection::cursor(range.start));
                }
                return iced::clipboard::write(text);
            }
            Paste => {
                let Some(tab) = self.active_tab() else { return Task::none() };
                if !tab.editable() {
                    return Task::none();
                }
                let id = tab.id;
                return iced::clipboard::read().map(move |t| Message::Paste(id, t));
            }
            SelectAll => {
                if let Some(tab) = self.active_tab_mut() {
                    let len = tab.doc.buffer().len_bytes();
                    tab.set_selection(Selection { anchor: 0, head: len });
                }
            }
            Find | FindInFiles | ShowSearch => {
                self.sidebar.visible = true;
                self.sidebar.tab = SidebarTab::Search;
                if let Some(tab) = self.active_tab() {
                    let sel = tab.selection();
                    if !sel.is_empty() && sel.range().len() < 200 {
                        self.search.query = tab.doc.buffer().slice(sel.range()).into_owned();
                        self.search.for_version = None;
                    }
                }
                return Task::batch([
                    iced::widget::operation::focus(SEARCH_INPUT),
                    iced::widget::operation::select_all(SEARCH_INPUT),
                ]);
            }
            OpenFolder | CloseFolder | ProjectNewFile | ProjectNewFolder | Checkpoint | Sync | NewBranch
            | FileHistory => {
                return self.project_command(cmd);
            }
            ModeEdit => self.set_mode(Mode::Edit),
            ModeSplit => self.set_mode(Mode::Split),
            ModeView => self.set_mode(Mode::View),
            CycleMode => {
                // Edit, Split, View, Read, then back to Edit.
                let Some(tab) = self.active_tab() else { return Task::none() };
                if tab.mode == Mode::View && !tab.view.read_only && !tab.read_only() {
                    self.set_read(true);
                } else {
                    let m = if tab.doc.is_large() { Mode::Edit } else { tab.mode.next() };
                    self.set_mode(m);
                }
            }
            ToggleSidebar => self.sidebar.visible = !self.sidebar.visible,
            ModeRead => {
                // From Read, the shortcut goes back to (editable) View.
                let on = !self.active_tab().is_some_and(|t| t.mode == Mode::View && t.view.read_only);
                self.set_read(on);
            }
            ToggleComments => self.comments_visible = !self.comments_visible,
            AddComment => return self.add_comment(),
            ShowFiles => return self.show_sidebar(SidebarTab::Files),
            ShowOutline => return self.show_sidebar(SidebarTab::Outline),
            ShowGit => return self.show_sidebar(SidebarTab::Git),
            Palette | QuickOpen => {
                let scope = if cmd == Palette { Scope::Commands } else { Scope::Files };
                self.palette = Some(palette::Palette::new(scope));
                self.menu_open = None;
                return iced::widget::operation::focus(PALETTE_INPUT);
            }
            NextTab | PrevTab => {
                let n = self.tabs.len();
                if n > 0 {
                    self.active = if cmd == NextTab { (self.active + 1) % n } else { (self.active + n - 1) % n };
                }
            }
            ZoomIn | ZoomOut | ZoomReset => {
                let z = &mut self.settings.appearance.zoom;
                *z = match cmd {
                    ZoomIn => (*z + 0.1).min(3.0),
                    ZoomOut => (*z - 0.1).max(0.5),
                    _ => 1.0,
                };
                *z = (*z * 10.0).round() / 10.0;
                self.apply_editor_settings();
                self.store_settings();
            }
            ToggleFullWidth => {
                self.settings.editor.full_width = !self.settings.editor.full_width;
                self.apply_editor_settings();
                self.store_settings();
            }
            ToggleLineNumbers => {
                self.settings.editor.line_numbers = !self.settings.editor.line_numbers;
                self.apply_editor_settings();
                self.store_settings();
            }
            AppearanceSystem | AppearanceLight | AppearanceDark => {
                self.settings.appearance.mode = match cmd {
                    AppearanceLight => AppearanceMode::Light,
                    AppearanceDark => AppearanceMode::Dark,
                    _ => AppearanceMode::System,
                };
                self.apply_theme();
                self.store_settings();
            }
            OpenSettings => return self.open_config_file(self.paths.settings(), crate::settings::SETTINGS_TEMPLATE),
            OpenKeymap => return self.open_config_file(self.paths.keymap(), crate::settings::KEYMAP_TEMPLATE),
            About => self.dialog = Some(Dialog::About),
        }
        Task::none()
    }

    fn show_sidebar(&mut self, t: SidebarTab) -> Task<Message> {
        self.sidebar.visible = true;
        self.sidebar.tab = t;
        Task::none()
    }

    /// Edit, Split or View; View is the editable one (Read is `set_read`).
    fn set_mode(&mut self, mode: Mode) {
        if let Some(tab) = self.active_tab_mut() {
            if tab.doc.is_large() && mode != Mode::Edit {
                self.toast("Large files open in Edit mode only.");
                return;
            }
            tab.set_mode(mode);
            tab.view.read_only = false;
        }
    }

    /// Read mode: View without editing (`on`), or back to editable View.
    fn set_read(&mut self, on: bool) {
        self.set_mode(Mode::View);
        if let Some(tab) = self.active_tab_mut()
            && tab.mode == Mode::View
        {
            tab.view.read_only = on;
        }
    }

    fn store_settings(&mut self) {
        if let Err(e) = self.settings.store_ui_change(&self.paths.settings()) {
            self.toast_error(format!("Could not save settings: {e}"));
        }
    }

    fn reload_config(&mut self) {
        let (settings, err) = Settings::load(&self.paths.settings());
        let (keymap_file, kerr) = load_keymap_file(&self.paths.keymap());
        if let Some(e) = err.or(kerr) {
            self.toast_error(e);
            return;
        }
        self.settings = settings;
        self.keymap_file = keymap_file;
        self.keymap = Keymap::new([&self.settings.keymap, &self.keymap_file]);
        for w in self.keymap.warnings.clone() {
            self.toast_error(w);
        }
        self.load_themes();
        self.apply_theme();
        self.apply_editor_settings();
        self.toast("Settings applied.");
    }

    fn open_config_file(&mut self, path: PathBuf, template: &str) -> Task<Message> {
        if !path.exists()
            && let Err(e) = crate::settings::write_atomic(&path, template.as_bytes())
        {
            self.toast_error(format!("Could not create {}: {e}", path.display()));
            return Task::none();
        }
        self.handle(Message::OpenPicked(vec![path]))
    }

    fn on_loaded(&mut self, path: PathBuf, result: Result<Handoff<Document>, String>) {
        let restored = self.restoring.iter().position(|t| t.path == path).map(|i| self.restoring.remove(i));
        let doc = match result.map(|h| h.take()) {
            Ok(Some(doc)) => doc,
            Ok(None) => return,
            Err(e) => {
                if restored.is_none() {
                    self.toast_error(format!("Could not open {}: {e}", path.display()));
                }
                self.ensure_a_tab();
                return;
            }
        };
        if self.tabs.iter().any(|t| t.doc.path.as_deref() == Some(&path)) {
            return;
        }
        let mode = restored.as_ref().map_or(self.settings.editor.default_mode, |t| t.mode);
        let mut tab = Tab::new(self.next_id, doc, mode, 0);
        self.next_id += 1;
        tab.editor.line_numbers = self.settings.editor.line_numbers;
        tab.editor.monospace = self.settings.editor.monospace;
        tab.editor.full_width = self.settings.editor.full_width;
        for v in [&mut tab.view, &mut tab.preview] {
            v.full_width = self.settings.editor.full_width;
            v.zoom = self.settings.appearance.zoom;
        }
        if tab.read_only() {
            self.toast("Not valid UTF-8: opened read-only.");
        }
        if let Some(t) = &restored {
            let len = tab.doc.buffer().len_bytes();
            let sel = Selection {
                anchor: clamp_char(&tab.doc, t.anchor.min(len)),
                head: clamp_char(&tab.doc, t.head.min(len)),
            };
            tab.set_selection(sel);
            tab.editor.scroll_y = t.scroll_y;
            tab.view.scroll_y = t.scroll_y;
            tab.view.read_only = t.view_only;
        }
        self.session.push_recent(&path);

        // Replace a pristine untitled tab instead of piling up empties.
        if let Some(i) =
            self.tabs.iter().position(|t| t.doc.path.is_none() && !t.is_dirty() && t.doc.buffer().len_bytes() == 0)
        {
            self.tabs.remove(i);
            if self.active >= i && self.active > 0 {
                self.active -= 1;
            }
        }
        // Restored tabs keep their saved order; new ones go at the end.
        let rank = |p: &Path, s: &Session| s.tabs.iter().position(|t| t.path == p);
        let pos = match rank(&path, &self.session) {
            Some(r) => self
                .tabs
                .iter()
                .position(|t| t.doc.path.as_deref().and_then(|p| rank(p, &self.session)).is_none_or(|o| o > r))
                .unwrap_or(self.tabs.len()),
            None => self.tabs.len(),
        };
        self.tabs.insert(pos, tab);
        if self.active >= pos && self.tabs.len() > 1 && self.restore_active.as_deref() != Some(&path) {
            self.active = (self.active + 1).min(self.tabs.len() - 1);
        }
        if self.restore_active.as_deref() == Some(&path) || self.tabs.len() == 1 {
            self.active = pos;
        }
        if self.restoring.is_empty() {
            self.save_session();
        }
    }

    fn ensure_a_tab(&mut self) {
        if self.tabs.is_empty() && self.restoring.is_empty() {
            self.new_untitled();
        }
    }

    fn save_as(&mut self, id: TabId) -> Task<Message> {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return Task::none() };
        let name = match &tab.doc.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => format!("{}.md", tab.title()),
        };
        let dir =
            tab.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf).or(self.session.last_dir.clone());
        Task::perform(
            async move {
                let mut d = rfd::AsyncFileDialog::new().set_file_name(name).add_filter("Markdown", &["md", "markdown"]);
                if let Some(dir) = dir {
                    d = d.set_directory(dir);
                }
                d.save_file().await.map(|h| h.path().to_path_buf())
            },
            move |p| Message::SaveAsPicked(id, p),
        )
    }

    fn start_save(&mut self, id: TabId, to: Option<PathBuf>) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else { return Task::none() };
        match tab.begin_save(to.as_deref()) {
            Ok(job) => {
                let (path, point) = (job.path.clone(), job.point);
                Task::perform(
                    async move {
                        let remap =
                            tokio::task::spawn_blocking(move || job.run()).await.map_err(|e| e.to_string())??;
                        Ok(Handoff::new(remap))
                    },
                    move |r| Message::Saved(id, path.clone(), point, r),
                )
            }
            Err(e) => {
                self.toast_error(format!("Could not save: {e}"));
                self.quitting = false;
                Task::none()
            }
        }
    }

    fn on_saved(
        &mut self,
        id: TabId,
        path: PathBuf,
        point: SavePoint,
        result: Result<Handoff<Option<mdedit_core::mapped::Remap>>, String>,
    ) -> Task<Message> {
        let Some(tab) = self.tab_mut(id) else { return Task::none() };
        let after = match result {
            Ok(remap) => {
                let was = tab.doc.path.clone();
                tab.finish_save(path.clone(), point, remap.take().flatten());
                if was.as_deref() != Some(&path) {
                    self.session.push_recent(&path);
                    self.save_session();
                }
                self.comments_saved(id, &path);
                if path == self.paths.settings() || path == self.paths.keymap() {
                    self.reload_config();
                }
                self.project_saved(&path)
            }
            Err(e) => {
                tab.saving = false;
                self.toast_error(format!("Could not save {}: {e}", path.display()));
                self.quitting = false;
                self.close_after_save.retain(|t| *t != id);
                return Task::none();
            }
        };
        if let Some(i) = self.close_after_save.iter().position(|t| *t == id) {
            self.close_after_save.remove(i);
            if let Some(idx) = self.tab_index(id) {
                self.close_tab_now(idx);
            }
        }
        if self.quitting {
            return Task::batch([after, self.continue_quit()]);
        }
        after
    }

    fn request_close(&mut self, id: TabId) -> Task<Message> {
        let Some(idx) = self.tab_index(id) else { return Task::none() };
        if self.tabs[idx].is_dirty() {
            self.active = idx;
            self.dialog = Some(Dialog::UnsavedTab(id));
        } else {
            self.close_tab_now(idx);
        }
        Task::none()
    }

    fn close_tab_now(&mut self, idx: usize) {
        self.tabs.remove(idx);
        if self.active > idx || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.ensure_a_tab();
        self.save_session();
    }

    fn on_dialog(&mut self, choice: DialogChoice) -> Task<Message> {
        let Some(dialog) = self.dialog.take() else { return Task::none() };
        match (dialog, choice) {
            (Dialog::UnsavedTab(id), DialogChoice::Save) => {
                self.close_after_save.push(id);
                let has_path = self.tabs.iter().any(|t| t.id == id && t.doc.path.is_some());
                return if has_path { self.start_save(id, None) } else { self.save_as(id) };
            }
            (Dialog::UnsavedTab(id), DialogChoice::Discard) => {
                if let Some(idx) = self.tab_index(id) {
                    self.close_tab_now(idx);
                }
            }
            (Dialog::UnsavedQuit(_), DialogChoice::Save) => {
                self.quitting = true;
                return self.continue_quit();
            }
            (Dialog::UnsavedQuit(_), DialogChoice::Discard) => return self.exit(),
            _ => {}
        }
        Task::none()
    }

    /// Save everything dirty, one save-as dialog at a time for untitled
    /// buffers, then exit.
    fn continue_quit(&mut self) -> Task<Message> {
        if self.tabs.iter().any(|t| t.saving) {
            return Task::none();
        }
        let with_path: Vec<TabId> =
            self.tabs.iter().filter(|t| t.is_dirty() && t.doc.path.is_some()).map(|t| t.id).collect();
        if !with_path.is_empty() {
            return Task::batch(with_path.into_iter().map(|id| self.start_save(id, None)));
        }
        if let Some(idx) = self.tabs.iter().position(|t| t.is_dirty()) {
            self.active = idx;
            let id = self.tabs[idx].id;
            return self.save_as(id);
        }
        self.exit()
    }

    fn exit(&mut self) -> Task<Message> {
        if let Some(t) = &self.trace {
            t.report();
        }
        self.project_flush();
        self.save_session();
        iced::exit()
    }

    pub fn session_snapshot(&self) -> Session {
        let mut s = self.session.clone();
        s.tabs = self
            .tabs
            .iter()
            .filter_map(|t| {
                let sel = t.selection();
                Some(TabSession {
                    path: t.doc.path.clone()?,
                    mode: t.mode,
                    anchor: sel.anchor,
                    head: sel.head,
                    scroll_y: if t.mode == Mode::View { t.view.scroll_y } else { t.editor.scroll_y },
                    view_only: t.view.read_only,
                })
            })
            .collect();
        // Session tabs that are still loading must not be forgotten.
        s.tabs.extend(self.restoring.iter().cloned());
        s.active = self
            .active_tab()
            .and_then(|t| t.doc.path.as_ref())
            .and_then(|p| s.tabs.iter().position(|t| &t.path == p))
            .unwrap_or(0);
        s.sidebar_visible = self.sidebar.visible;
        s.sidebar_tab = self.sidebar.tab.key().into();
        s.sidebar_width = self.sidebar.width;
        s.comments_visible = self.comments_visible;
        s.project = self.project_root().map(Path::to_path_buf);
        s
    }

    fn save_session(&mut self) {
        self.session_dirty = false;
        let s = self.session_snapshot();
        self.session = s;
        if let Err(e) = self.session.save(&self.paths.session()) {
            eprintln!("mdedit: could not save session: {e}");
        }
    }

    /// A document widget reported where its selection is. Only the widget
    /// that takes typing in the active tab counts (not the Split preview).
    fn on_selection_rect(&mut self, id: TabId, from_view: bool, rect: Option<Rectangle>) -> Task<Message> {
        let ours = self.active_tab().is_some_and(|t| t.id == id && (t.mode == Mode::View) == from_view);
        if !ours || rect == self.toolbar.rect {
            return Task::none();
        }
        self.toolbar = FormatToolbar { rect, changed: Instant::now(), ..FormatToolbar::default() };
        if rect.is_none() {
            return Task::none();
        }
        Task::perform(tokio::time::sleep(TOOLBAR_DELAY), |_| Message::ToolbarTick)
    }

    pub(crate) fn hide_toolbar_for_typing(&mut self) {
        self.toolbar.visible = false;
        self.toolbar.heading_open = false;
        self.toolbar.dismissed = true;
    }

    fn on_editor(&mut self, id: TabId, action: mdedit_editor::Action) -> Task<Message> {
        use mdedit_editor::Action;
        match &action {
            Action::SelectionRect(rect) => return self.on_selection_rect(id, false, *rect),
            Action::CaretRect(rect) => {
                self.slash_caret(id, *rect);
                return Task::none();
            }
            Action::Edit(_) => self.hide_toolbar_for_typing(),
            Action::Select(_) => self.comments.caret_moved = Some(id),
            _ => {}
        }
        let Some(tab) = self.tab_mut(id) else { return Task::none() };
        match action {
            Action::Edit(edit) => {
                if tab.read_only() {
                    return Task::none();
                }
                // Typed text (list continuation, indent, wrapping a selection)
                // may become a smarter command (DESIGN_SYSTEM 8.3).
                let sel = tab.selection();
                let typed = edit.range == sel.range() && !edit.text.is_empty();
                match typed.then(|| core_cmd::smart_input(&tab.doc, sel, &edit.text)).flatten() {
                    Some(result) => {
                        tab.apply_command(result);
                        tab.skip_select = true;
                    }
                    None => tab.apply(&edit),
                }
            }
            Action::Select(sel) => {
                if !std::mem::take(&mut tab.skip_select) {
                    tab.set_selection(sel);
                }
            }
            Action::Scroll(y) => {
                tab.editor.scroll_y = y;
                // The jump happened; clear it so the same byte can be asked for again.
                tab.editor.scroll_to_byte = None;
            }
            // Split mode: the preview follows the source (ARCHITECTURE 4.3).
            Action::TopAnchor { byte, fraction } => {
                tab.preview.sync = Some(mdedit_view::SyncAnchor { byte, fraction });
            }
            Action::Copy(text) => return iced::clipboard::write(text),
            Action::Paste => {
                if tab.read_only() {
                    return Task::none();
                }
                return iced::clipboard::read().map(move |t| Message::Paste(id, t));
            }
            Action::Undo => return self.run_app(AppCommand::Undo),
            Action::Redo => return self.run_app(AppCommand::Redo),
            Action::SelectionRect(_) | Action::CaretRect(_) => {}
        }
        Task::none()
    }

    fn on_view(&mut self, id: TabId, action: mdedit_view::Action) -> Task<Message> {
        use mdedit_view::Action;
        match &action {
            Action::SelectionRect(rect) => return self.on_selection_rect(id, true, *rect),
            Action::CaretRect(rect) => {
                if self.active_tab().is_some_and(|t| t.id == id && t.mode == Mode::View) {
                    self.slash_caret(id, *rect);
                }
                return Task::none();
            }
            Action::Edit(_) => self.hide_toolbar_for_typing(),
            Action::Select(_) => self.comments.caret_moved = Some(id),
            _ => {}
        }
        let Some(tab) = self.tab_mut(id) else { return Task::none() };
        match action {
            Action::Edit(edit) => {
                if tab.editable() && tab.mode == Mode::View {
                    tab.apply(&edit);
                }
            }
            Action::Select(sel) => {
                if tab.mode == Mode::View {
                    tab.set_selection(sel);
                }
            }
            Action::Scroll(y) => {
                if tab.mode == Mode::View {
                    tab.view.scroll_y = y;
                } else {
                    tab.preview.scroll_y = y;
                }
            }
            Action::OpenLink(url) => self.toast(format!("Link: {url}")),
            Action::SelectionRect(_) | Action::CaretRect(_) => {}
        }
        Task::none()
    }

    /// A dialog-like layer is open: the document takes no keys.
    pub fn modal_open(&self) -> bool {
        self.dialog.is_some()
            || self.export.dialog.is_some()
            || self.prompt.is_some()
            || self.palette.is_some()
            || self.project.as_ref().is_some_and(|p| p.history.is_some())
    }

    pub fn palette_items(&self) -> Vec<palette::Item> {
        let Some(p) = &self.palette else { return Vec::new() };
        let items = match p.scope {
            Scope::Commands => command::all()
                .iter()
                .map(|c| palette::Item::Command {
                    id: c.id,
                    title: c.title,
                    shortcut: self.keymap.shortcut_for(c.id).map(|k| k.to_string()),
                })
                .collect(),
            Scope::Files => {
                let mut v: Vec<palette::Item> = self
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(i, t)| palette::Item::Tab {
                        index: i,
                        title: t.title(),
                        detail: t
                            .doc
                            .path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "unsaved".into()),
                    })
                    .collect();
                for path in &self.session.recent {
                    if self.tabs.iter().any(|t| t.doc.path.as_ref() == Some(path)) {
                        continue;
                    }
                    v.push(palette::Item::Recent {
                        path: path.clone(),
                        title: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        detail: path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
                    });
                }
                v
            }
        };
        p.filter(items)
    }

    fn on_palette(&mut self, msg: PaletteMsg) -> Task<Message> {
        match msg {
            PaletteMsg::Query(q) => {
                if let Some(p) = &mut self.palette {
                    p.query = q;
                    p.selected = 0;
                }
            }
            PaletteMsg::Close => self.palette = None,
            PaletteMsg::Submit => {
                let selected = self.palette.as_ref().map_or(0, |p| p.selected);
                return self.on_palette(PaletteMsg::Pick(selected));
            }
            PaletteMsg::Pick(i) => {
                let item = self.palette_items().into_iter().nth(i);
                self.palette = None;
                match item {
                    Some(palette::Item::Command { id, .. }) => return self.run(id),
                    Some(palette::Item::Tab { index, .. }) => self.active = index,
                    Some(palette::Item::Recent { path, .. }) => return self.handle(Message::OpenPicked(vec![path])),
                    None => {}
                }
            }
        }
        Task::none()
    }

    fn on_banner(&mut self, id: TabId, action: BannerAction) {
        let threshold = self.settings.files.large_file_threshold();
        let Some(idx) = self.tab_index(id) else { return };
        let tab = &mut self.tabs[idx];
        match action {
            BannerAction::Reload => {
                if let Err(e) = tab.reload(threshold) {
                    self.toast_error(format!("Could not reload: {e}"));
                }
            }
            BannerAction::KeepMine => {
                // Stop nagging until the file changes again.
                tab.external = None;
                tab.disk = tab.doc.path.as_deref().and_then(DiskStamp::read);
            }
            BannerAction::Close => self.close_tab_now(idx),
        }
    }

    fn on_file_changed(&mut self, path: &Path) {
        if self.comments_file_changed(path) {
            return;
        }
        let threshold = self.settings.files.large_file_threshold();
        let mut errors = Vec::new();
        for tab in self.tabs.iter_mut().filter(|t| t.doc.path.as_deref() == Some(path)) {
            if tab.saving {
                continue;
            }
            let now = DiskStamp::read(path);
            if now == tab.disk && tab.external.is_none() {
                continue;
            }
            // A mapped file is never reloaded silently: the map may be stale.
            if let Some(m) = tab.doc.mapped() {
                if m.disk_change() != mdedit_core::DiskChange::Unchanged {
                    tab.external = Some(if now.is_some() { External::Changed } else { External::Deleted });
                }
                continue;
            }
            match now {
                None => {
                    tab.external = Some(External::Deleted);
                    tab.disk = None;
                }
                Some(_) if !tab.is_dirty() && tab.external.is_none() => {
                    if let Err(e) = tab.reload(threshold) {
                        errors.push(e);
                    }
                }
                Some(stamp) => {
                    tab.external = Some(External::Changed);
                    tab.disk = Some(stamp);
                }
            }
        }
        for e in errors {
            self.toast_error(format!("Could not reload {}: {e}", path.display()));
        }
    }
}

/// Words (runs of non-whitespace) and characters (excluding line breaks)
/// over byte chunks, which may split a UTF-8 sequence.
fn count_stats<'a>(chunks: impl Iterator<Item = &'a [u8]>) -> Stats {
    let mut stats = Stats::default();
    let mut in_word = false;
    for chunk in chunks {
        for &b in chunk {
            if b & 0xC0 == 0x80 {
                continue; // UTF-8 continuation byte
            }
            if b != b'\n' && b != b'\r' {
                stats.chars += 1;
            }
            let space = b.is_ascii_whitespace();
            if !space && !in_word {
                stats.words += 1;
            }
            in_word = !space;
        }
    }
    stats
}

fn load_keymap_file(path: &Path) -> (Overrides, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match keymap::parse_overrides(&text) {
            Ok(o) => (o, None),
            Err(e) => (Overrides::default(), Some(format!("keymap.toml: {}", e.message()))),
        },
        Err(_) => (Overrides::default(), None),
    }
}

/// Absolute, symlink-resolved path where possible, so the watcher's event
/// paths and tab paths compare equal.
fn absolute(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
}

/// Move `byte` back to a char boundary (session offsets may be stale).
fn clamp_char(doc: &Document, byte: usize) -> usize {
    let buf = doc.buffer();
    let byte = byte.min(buf.len_bytes());
    let line = buf.byte_to_line(byte);
    let start = buf.line_to_byte(line);
    let text = buf.line(line);
    let mut off = (byte - start).min(text.len());
    while !text.is_char_boundary(off) {
        off -= 1;
    }
    start + off
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_across_split_chunks() {
        let text = "Hello wörld,\r\n  two  words\n";
        let whole = count_stats(std::iter::once(text.as_bytes()));
        assert_eq!(whole, Stats { words: 4, chars: 24 });
        let (a, b) = text.as_bytes().split_at(8); // inside "ö"
        assert_eq!(count_stats([a, b].into_iter()), whole);
        let (a, b) = text.as_bytes().split_at(3); // inside "Hello"
        assert_eq!(count_stats([a, b].into_iter()), whole);
    }
}
