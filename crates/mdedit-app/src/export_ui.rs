//! Export UI (SPEC 10, DESIGN_SYSTEM 7 and 8.6): File → Export as PDF /
//! DOCX opens a dialog (template, page size, TOC, numbering, comments as
//! footnotes), then a save dialog, then runs `mdedit_export` on a worker.
//! The status bar shows the running export; the result is a notice with
//! Open / Show in folder, or the error with its details.
//!
//! The app hooks in through one `export` field, one `Message::Export`
//! variant and a few calls (`on_export`, `export_key`, `export_layers`,
//! `export_status`, `ExportUi::subscription`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::keyboard::{Key, key::Named};
use iced::widget::{Space, button, checkbox, column, container, opaque, row, scrollable, text};
use iced::{Alignment, Background, Border, Element, Length, Padding, Subscription, Task};
use mdedit_core::TextBuffer;
use mdedit_export::{ExportComment, ExportFormat, ExportOptions, Template};
use mdedit_theme::space::*;
use mdedit_theme::{ColorToken as T, Theme};

use crate::app::{App, Message};
use crate::style::{self, Btn, c};
use crate::tab::TabId;
use crate::view::{bold, modal, small_button};

const UI: f32 = 13.0;
const UI_SMALL: f32 = 12.0;
const EYEBROW: f32 = 11.0;

/// Page sizes offered in the dialog; `None` keeps the template's.
const PAGE_SIZES: &[(Option<&str>, &str)] =
    &[(None, "Template"), (Some("a4"), "A4"), (Some("us-letter"), "Letter"), (Some("a5"), "A5")];

#[derive(Debug, Clone)]
pub enum ExportMsg {
    Open(ExportFormat),
    Template(usize),
    PageSize(Option<&'static str>),
    Toc(bool),
    Numbers(bool),
    Comments(bool),
    Confirm,
    Cancel,
    PathPicked(Option<PathBuf>),
    Done(u64, Result<PathBuf, String>),
    Tick,
    OpenFile,
    ShowFolder,
    ToggleDetails,
    Dismiss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    Global,
    Project,
}

/// One template in the dialog's list.
#[derive(Debug, Clone)]
pub struct Choice {
    pub template: Template,
    pub origin: Origin,
}

impl Choice {
    fn label(&self) -> String {
        let mut name = self.template.name.clone();
        if let Some(first) = name.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        name
    }
}

#[derive(Debug, Clone)]
pub struct ExportDialog {
    pub tab: TabId,
    pub format: ExportFormat,
    pub choices: Vec<Choice>,
    pub selected: usize,
    pub page_size: Option<&'static str>,
    pub toc: bool,
    pub numbers: bool,
    pub include_comments: bool,
    /// Open comments in the sidecar (0 hides the option).
    pub comment_count: usize,
    /// Templates that failed to load, shown under the list.
    pub warnings: Vec<String>,
}

impl ExportDialog {
    fn template(&self) -> &Template {
        &self.choices[self.selected].template
    }

    fn options(&self) -> ExportOptions {
        let t = self.template();
        ExportOptions {
            page_size: self.page_size.map(str::to_owned),
            toc: (self.toc != t.toc).then_some(self.toc),
            heading_numbers: (self.numbers != t.heading_numbers).then_some(self.numbers),
            include_comments: self.include_comments && self.comment_count > 0,
            ..ExportOptions::default()
        }
    }
}

/// A running export, shown in the status bar.
#[derive(Debug, Clone)]
pub struct Job {
    id: u64,
    pub file_name: String,
    pub started: Instant,
}

/// The outcome of the last export (DESIGN_SYSTEM 7 Toast: it has actions,
/// so it stays until dismissed or replaced).
#[derive(Debug, Clone)]
pub struct Notice {
    pub result: Result<PathBuf, String>,
    pub took: Duration,
    pub details: bool,
}

/// Dialog choices waiting for the save dialog.
#[derive(Debug, Clone)]
struct Request {
    tab: TabId,
    format: ExportFormat,
    template: Template,
    options: ExportOptions,
}

#[derive(Debug, Default)]
pub struct ExportUi {
    pub dialog: Option<ExportDialog>,
    pub job: Option<Job>,
    pub notice: Option<Notice>,
    pending: Option<Request>,
    next_id: u64,
    /// Remembered for the next export in this run.
    last_template: Option<String>,
    last_comments: Option<bool>,
}

impl ExportUi {
    pub fn subscription(&self) -> Option<Subscription<Message>> {
        self.job
            .is_some()
            .then(|| iced::time::every(Duration::from_millis(100)).map(|_| Message::Export(ExportMsg::Tick)))
    }

    pub fn busy(&self) -> bool {
        self.job.is_some() || self.pending.is_some()
    }
}

/// Built-ins, then global templates, then project ones; a later template
/// with the same name replaces an earlier one (project overrides global
/// overrides built-in, as `Template::resolve` searches).
pub fn template_choices(project_dir: Option<&Path>, global_dir: &Path) -> (Vec<Choice>, Vec<String>) {
    let mut choices: Vec<Choice> =
        Template::builtins().into_iter().map(|template| Choice { template, origin: Origin::BuiltIn }).collect();
    let mut warnings = Vec::new();
    let dirs = [(Some(global_dir), Origin::Global), (project_dir, Origin::Project)];
    for (dir, origin) in dirs {
        let Some(dir) = dir else { continue };
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut found: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.join("template.toml").is_file()).collect();
        found.sort();
        for path in found {
            match Template::load_dir(&path) {
                Ok(template) => {
                    let choice = Choice { template, origin };
                    let same = |c: &Choice| c.template.name.eq_ignore_ascii_case(&choice.template.name);
                    match choices.iter_mut().find(|c| same(c)) {
                        Some(slot) => *slot = choice,
                        None => choices.push(choice),
                    }
                }
                Err(e) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    warnings.push(format!("{name}: {e}"));
                }
            }
        }
    }
    (choices, warnings)
}

/// Comments (open ones, with replies) re-anchored against `buf`. Detached
/// comments have no place in the text and are left out.
pub fn export_comments(sidecar: &mdedit_sidecar::Sidecar, buf: &dyn TextBuffer) -> Vec<ExportComment> {
    sidecar
        .comments
        .iter()
        .filter(|c| !c.resolved)
        .filter_map(|c| {
            let range = mdedit_sidecar::reanchor(&c.anchor, buf).range()?;
            let mut body = c.body.trim().to_owned();
            for reply in &c.replies {
                body.push_str(&format!(" — {}: {}", reply.author, reply.body.trim()));
            }
            Some(ExportComment { offset: range.start, author: c.author.clone(), body })
        })
        .collect()
}

/// `dir/name.md` → `dir/name.<ext>`; untitled documents use the tab title.
fn default_output(doc_path: Option<&Path>, title: &str, format: ExportFormat) -> (Option<PathBuf>, String) {
    let stem = doc_path
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| title.to_owned());
    (doc_path.and_then(Path::parent).map(Path::to_path_buf), format!("{stem}.{}", format.extension()))
}

/// Add the format's extension when the save dialog returned a name
/// without it.
fn with_extension(path: PathBuf, format: ExportFormat) -> PathBuf {
    let ext = format.extension();
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
        path
    } else {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{ext}"));
        path.with_file_name(name)
    }
}

fn format_label(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Pdf => "PDF",
        ExportFormat::Docx => "DOCX",
    }
}

/// Everything the worker needs; built on the UI thread.
struct Work {
    snapshot: Arc<dyn TextBuffer>,
    doc_path: Option<PathBuf>,
    base_dir: PathBuf,
    template: Template,
    options: ExportOptions,
    format: ExportFormat,
    out: PathBuf,
}

impl Work {
    fn run(mut self) -> Result<PathBuf, String> {
        if self.options.include_comments
            && let Some(path) = &self.doc_path
        {
            let sidecar = mdedit_sidecar::Sidecar::load(path).map_err(|e| format!("comments: {e}"))?;
            self.options.comments = export_comments(&sidecar, &*self.snapshot);
        }
        let markdown = self.snapshot.text();
        mdedit_export::export_to_file(&markdown, &self.base_dir, &self.template, &self.options, self.format, &self.out)
            .map_err(|e| match e {
                mdedit_export::ExportError::Io(e) => format!("Could not write {}: {e}", self.out.display()),
                e => e.to_string(),
            })?;
        Ok(self.out)
    }
}

/// Open a file or folder with the desktop's default application.
fn open_with_system(path: &Path) {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg("/select,");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    // Detached: the viewer outlives nothing of ours, and errors only mean
    // there is no handler.
    let _ = cmd.arg(path).spawn();
}

impl App {
    pub(crate) fn on_export(&mut self, msg: ExportMsg) -> Task<Message> {
        match msg {
            ExportMsg::Open(format) => return self.open_export_dialog(format),
            ExportMsg::Template(i) => {
                if let Some(d) = &mut self.export.dialog
                    && i < d.choices.len()
                {
                    d.selected = i;
                    d.toc = d.template().toc;
                    d.numbers = d.template().heading_numbers;
                }
            }
            ExportMsg::PageSize(size) => {
                if let Some(d) = &mut self.export.dialog {
                    d.page_size = size;
                }
            }
            ExportMsg::Toc(on) => {
                if let Some(d) = &mut self.export.dialog {
                    d.toc = on;
                }
            }
            ExportMsg::Numbers(on) => {
                if let Some(d) = &mut self.export.dialog {
                    d.numbers = on;
                }
            }
            ExportMsg::Comments(on) => {
                if let Some(d) = &mut self.export.dialog {
                    d.include_comments = on;
                }
            }
            ExportMsg::Cancel => self.export.dialog = None,
            ExportMsg::Confirm => return self.confirm_export(),
            ExportMsg::PathPicked(None) => self.export.pending = None,
            ExportMsg::PathPicked(Some(path)) => return self.start_export(path),
            ExportMsg::Done(id, result) => {
                let Some(job) = self.export.job.take_if(|j| j.id == id) else { return Task::none() };
                self.export.notice = Some(Notice { result, took: job.started.elapsed(), details: false });
            }
            // Only redraws the status bar.
            ExportMsg::Tick => {}
            ExportMsg::OpenFile => {
                if let Some(Notice { result: Ok(path), .. }) = &self.export.notice {
                    open_with_system(path);
                }
                self.export.notice = None;
            }
            ExportMsg::ShowFolder => {
                if let Some(Notice { result: Ok(path), .. }) = &self.export.notice {
                    // Explorer selects the file itself; elsewhere open its folder.
                    let target = if cfg!(windows) { Some(path.as_path()) } else { path.parent() };
                    if let Some(target) = target {
                        open_with_system(target);
                    }
                }
                self.export.notice = None;
            }
            ExportMsg::ToggleDetails => {
                if let Some(n) = &mut self.export.notice {
                    n.details = !n.details;
                }
            }
            ExportMsg::Dismiss => self.export.notice = None,
        }
        Task::none()
    }

    fn open_export_dialog(&mut self, format: ExportFormat) -> Task<Message> {
        if self.export.busy() {
            self.toast("An export is already running.");
            return Task::none();
        }
        let Some((tab_id, doc_path)) = self.active_tab().map(|t| (t.id, t.doc.path.clone())) else {
            return Task::none();
        };
        let project = doc_path.as_deref().and_then(mdedit_project::Project::find_root);
        let project_templates = project.as_ref().map(|root| root.join(".mdedit").join("templates"));
        let (choices, warnings) =
            template_choices(project_templates.as_deref(), &self.paths.config_dir.join("templates"));
        let project_default = project
            .and_then(|root| mdedit_project::Project::open(&root).ok())
            .and_then(|p| p.config.export.default_template);
        let wanted = self.export.last_template.clone().or(project_default).unwrap_or_else(|| "guide".into());
        let selected = choices.iter().position(|c| c.template.name.eq_ignore_ascii_case(&wanted)).unwrap_or(0);
        let comment_count = doc_path
            .as_deref()
            .and_then(|p| mdedit_sidecar::Sidecar::load(p).ok())
            .map_or(0, |s| s.comments.iter().filter(|c| !c.resolved).count());
        let template = &choices[selected].template;
        self.menu_open = None;
        self.context_menu = None;
        self.palette = None;
        self.export.dialog = Some(ExportDialog {
            tab: tab_id,
            format,
            toc: template.toc,
            numbers: template.heading_numbers,
            choices,
            selected,
            page_size: None,
            include_comments: self.export.last_comments.unwrap_or(true),
            comment_count,
            warnings,
        });
        // Take the keyboard from the document so Enter and typing reach
        // the dialog only; closing it gives focus back (App::update).
        iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus())
    }

    fn confirm_export(&mut self) -> Task<Message> {
        let Some(d) = self.export.dialog.take() else { return Task::none() };
        let Some(tab) = self.tabs.iter().find(|t| t.id == d.tab) else { return Task::none() };
        self.export.last_template = Some(d.template().name.clone());
        if d.comment_count > 0 {
            self.export.last_comments = Some(d.include_comments);
        }
        let (dir, name) = default_output(tab.doc.path.as_deref(), &tab.title(), d.format);
        let dir = dir.or_else(|| self.session.last_dir.clone());
        let format = d.format;
        self.export.pending =
            Some(Request { tab: d.tab, format, template: d.template().clone(), options: d.options() });
        // Manual tests on a virtual display: skip the portal save dialog,
        // which would open on the user's real desktop.
        if let Some(to) = std::env::var_os("MDEDIT_EXPORT_DIR") {
            return self.start_export(PathBuf::from(to).join(name));
        }
        Task::perform(
            async move {
                let label = format_label(format);
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_title(format!("Export as {label}"))
                    .set_file_name(name)
                    .add_filter(label, &[format.extension()]);
                if let Some(dir) = dir {
                    dialog = dialog.set_directory(dir);
                }
                dialog.save_file().await.map(|h| with_extension(h.path().to_path_buf(), format))
            },
            |path| Message::Export(ExportMsg::PathPicked(path)),
        )
    }

    fn start_export(&mut self, out: PathBuf) -> Task<Message> {
        let Some(req) = self.export.pending.take() else { return Task::none() };
        let Some(tab) = self.tabs.iter().find(|t| t.id == req.tab) else {
            self.toast_error("The document was closed before the export started.");
            return Task::none();
        };
        let doc_path = tab.doc.path.clone();
        let base_dir = doc_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| out.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let mut options = req.options;
        // `{title}` for documents without a title heading.
        options.title = Some(
            doc_path
                .as_deref()
                .and_then(Path::file_stem)
                .map_or_else(|| tab.title(), |s| s.to_string_lossy().into_owned()),
        );
        let work = Work {
            snapshot: tab.doc.buffer().snapshot(),
            doc_path,
            base_dir,
            template: req.template,
            options,
            format: req.format,
            out: out.clone(),
        };
        self.export.next_id += 1;
        let id = self.export.next_id;
        let file_name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.export.job = Some(Job { id, file_name, started: Instant::now() });
        self.export.notice = None;
        Task::perform(
            async move { tokio::task::spawn_blocking(move || work.run()).await.map_err(|e| e.to_string())? },
            move |result| Message::Export(ExportMsg::Done(id, result)),
        )
    }

    /// Keys while the export dialog is open: Esc cancels, Enter exports,
    /// everything else is swallowed like the other dialogs do.
    pub(crate) fn export_key(&mut self, key: &Key) -> Option<Task<Message>> {
        self.export.dialog.as_ref()?;
        Some(match key {
            Key::Named(Named::Escape) => self.on_export(ExportMsg::Cancel),
            Key::Named(Named::Enter) => self.on_export(ExportMsg::Confirm),
            _ => Task::none(),
        })
    }

    /// The dialog and the result notice, as layers over the window.
    pub(crate) fn export_layers(&self) -> Vec<Element<'_, Message>> {
        let mut layers = Vec::new();
        if let Some(n) = &self.export.notice {
            layers.push(self.export_notice(n));
        }
        if let Some(d) = &self.export.dialog {
            layers.push(self.export_dialog(d));
        }
        layers
    }

    /// Status bar item while an export runs (DESIGN_SYSTEM 8.6). The export
    /// is one blocking call, so progress is indeterminate: a sliding
    /// segment and the elapsed time.
    pub(crate) fn export_status(&self) -> Option<Element<'_, Message>> {
        let job = self.export.job.as_ref()?;
        let t = &self.theme;
        const TRACK: f32 = 48.0;
        const SEGMENT: f32 = 14.0;
        let elapsed = job.started.elapsed().as_secs_f32();
        let phase = (elapsed / 1.2).fract();
        let x = phase * (TRACK - SEGMENT);
        let bar = container(
            row![Space::new().width(x), container(Space::new()).width(SEGMENT).height(3).style(fill(t, T::Accent)),]
                .height(3),
        )
        .width(TRACK)
        .height(3)
        .style(fill(t, T::Border));
        Some(
            row![
                bar,
                text(format!("Exporting {}… {elapsed:.1} s", job.file_name)).size(UI_SMALL).color(c(t, T::TextMuted)),
            ]
            .spacing(SPACE_2)
            .align_y(Alignment::Center)
            .into(),
        )
    }

    fn export_dialog<'a>(&'a self, d: &'a ExportDialog) -> Element<'a, Message> {
        let t = &self.theme;
        let label = format_label(d.format);
        let eyebrow = |s: &'a str| text(s).size(EYEBROW).font(bold()).color(c(t, T::TextMuted));

        let mut list = column![].spacing(2);
        for (i, choice) in d.choices.iter().enumerate() {
            let origin = match choice.origin {
                Origin::BuiltIn => "built-in",
                Origin::Global => "global",
                Origin::Project => "project",
            };
            let selected = i == d.selected;
            list = list.push(
                button(
                    row![
                        text(choice.label()).size(UI).width(Length::Fill),
                        text(origin).size(UI_SMALL).color(c(t, T::TextMuted)),
                    ]
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .height(MENU_ITEM_HEIGHT)
                .padding(Padding::from([0.0, SPACE_2]))
                .style(style::btn(t, if selected { Btn::Selected } else { Btn::MenuItem }))
                .on_press(Message::Export(ExportMsg::Template(i))),
            );
        }
        let mut templates = column![eyebrow("TEMPLATE"), scrollable(list).height(Length::Shrink)].spacing(SPACE_2);
        for w in &d.warnings {
            templates = templates.push(text(format!("Not loaded: {w}")).size(UI_SMALL).color(c(t, T::WarningText)));
        }

        let mut sizes = row![].spacing(2);
        let template_size = d.template().page_size.to_ascii_uppercase();
        for &(size, name) in PAGE_SIZES {
            let on = d.page_size == size;
            let name = if size.is_none() { format!("{name} ({template_size})") } else { name.to_owned() };
            sizes = sizes.push(
                button(text(name).size(UI_SMALL))
                    .padding(Padding::from([SPACE_1, SPACE_2]))
                    .style(style::btn(t, if on { Btn::Toggled } else { Btn::Secondary }))
                    .on_press(Message::Export(ExportMsg::PageSize(size))),
            );
        }

        let check = |on: bool, label: String, msg: fn(bool) -> ExportMsg| -> Element<'a, Message> {
            checkbox(on)
                .label(label)
                .size(16)
                .text_size(UI)
                .spacing(SPACE_2)
                .style(checkbox_style(t))
                .on_toggle(move |v| Message::Export(msg(v)))
                .into()
        };
        let mut options = column![
            eyebrow("OPTIONS"),
            check(d.toc, "Table of contents".into(), ExportMsg::Toc),
            check(d.numbers, "Number headings".into(), ExportMsg::Numbers),
        ]
        .spacing(SPACE_2);
        if d.comment_count > 0 {
            let n = d.comment_count;
            let label = format!("Include comments as footnotes ({n} open)");
            options = options.push(check(d.include_comments, label, ExportMsg::Comments));
        }

        let body = column![templates, column![eyebrow("PAGE SIZE"), sizes].spacing(SPACE_2), options,].spacing(SPACE_4);
        let title = self.tabs.iter().find(|x| x.id == d.tab).map(|x| x.title()).unwrap_or_default();
        let card = container(
            column![
                column![
                    text(format!("Export as {label}")).size(16).font(bold()),
                    text(title).size(UI).color(c(t, T::TextSecondary)),
                ]
                .spacing(SPACE_1),
                body,
                row![
                    Space::new().width(Length::Fill),
                    row![
                        small_button(t, "Cancel", Btn::Secondary, Some(Message::Export(ExportMsg::Cancel))),
                        small_button(t, "Export…", Btn::Primary, Some(Message::Export(ExportMsg::Confirm))),
                    ]
                    .spacing(SPACE_2)
                ],
            ]
            .spacing(SPACE_4),
        )
        .width(480)
        .padding(SPACE_5)
        .style(style::popover(t, RADIUS_LG));
        modal(t, card, Message::Export(ExportMsg::Cancel), true)
    }

    fn export_notice<'a>(&'a self, n: &'a Notice) -> Element<'a, Message> {
        let t = &self.theme;
        let close = button(text("×").size(UI)).padding(Padding::from([0.0, SPACE_1])).style(style::btn(t, Btn::Ghost));
        let close = close.on_press(Message::Export(ExportMsg::Dismiss));
        let content: Element<'a, Message> = match &n.result {
            Ok(path) => {
                let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                column![
                    row![
                        text(format!("Exported {name}")).size(UI).color(c(t, T::Text)).width(Length::Fill),
                        text(format!("{:.1} s", n.took.as_secs_f32())).size(UI_SMALL).color(c(t, T::TextMuted)),
                        close,
                    ]
                    .spacing(SPACE_2)
                    .align_y(Alignment::Center),
                    row![
                        small_button(t, "Show in folder", Btn::Secondary, Some(Message::Export(ExportMsg::ShowFolder))),
                        small_button(t, "Open", Btn::Primary, Some(Message::Export(ExportMsg::OpenFile))),
                    ]
                    .spacing(SPACE_2),
                ]
                .spacing(SPACE_2)
                .into()
            }
            Err(e) => {
                let mut col = column![
                    row![
                        text("Export failed").size(UI).color(c(t, T::Danger)).width(Length::Fill),
                        small_button(
                            t,
                            if n.details { "Hide details" } else { "Details" },
                            Btn::Ghost,
                            Some(Message::Export(ExportMsg::ToggleDetails)),
                        ),
                        close,
                    ]
                    .spacing(SPACE_2)
                    .align_y(Alignment::Center),
                ]
                .spacing(SPACE_2);
                if n.details {
                    col = col.push(
                        scrollable(text(e.as_str()).size(UI_SMALL).color(c(t, T::TextSecondary)))
                            .height(Length::Shrink),
                    );
                }
                col.into()
            }
        };
        let card = container(content)
            .width(360)
            .padding(Padding::from([SPACE_2, SPACE_3]))
            .style(style::popover(t, RADIUS_MD));
        // Above the regular toasts' column.
        container(opaque(card))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(Padding { top: 0.0, right: SPACE_4, bottom: STATUS_BAR_HEIGHT + SPACE_4, left: 0.0 })
            .into()
    }
}

fn fill(theme: &Theme, token: T) -> impl Fn(&iced::Theme) -> container::Style + '_ {
    move |_| container::Style {
        background: Some(Background::Color(c(theme, token))),
        border: Border { radius: 2.0.into(), ..Border::default() },
        ..container::Style::default()
    }
}

/// Checkbox from tokens: `border-strong` box, `accent` fill when checked,
/// `on-accent` tick.
fn checkbox_style(theme: &Theme) -> impl Fn(&iced::Theme, checkbox::Status) -> checkbox::Style + '_ {
    move |_, status| {
        use checkbox::Status::*;
        let (checked, hovered, disabled) = match status {
            Active { is_checked } => (is_checked, false, false),
            Hovered { is_checked } => (is_checked, true, false),
            Disabled { is_checked } => (is_checked, false, true),
        };
        let bg = if checked {
            c(theme, T::Accent)
        } else if hovered {
            c(theme, T::SurfaceRaised)
        } else {
            c(theme, T::Surface)
        };
        let border = if checked { c(theme, T::Accent) } else { c(theme, T::BorderStrong) };
        let mut text_color = c(theme, T::Text);
        if disabled {
            text_color.a = 0.4;
        }
        checkbox::Style {
            background: Background::Color(bg),
            icon_color: c(theme, T::OnAccent),
            border: Border { color: border, width: 1.0, radius: RADIUS_SM.into() },
            text_color: Some(text_color),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_listed_and_overridden() {
        let global = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let (choices, warnings) = template_choices(None, global.path());
        assert!(warnings.is_empty());
        let names: Vec<_> = choices.iter().map(|c| c.template.name.to_ascii_lowercase()).collect();
        assert_eq!(names, ["guide", "report", "plain"]);

        // A project template named like a built-in replaces it; a broken
        // one is reported, not fatal.
        let src = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../mdedit-export/templates/report/template.toml"),
        )
        .unwrap();
        std::fs::create_dir_all(project.path().join("report")).unwrap();
        std::fs::write(project.path().join("report/template.toml"), &src).unwrap();
        std::fs::create_dir_all(project.path().join("memo")).unwrap();
        let memo = src.replace("name = \"Report\"", "name = \"Memo\"");
        std::fs::write(project.path().join("memo/template.toml"), memo).unwrap();
        std::fs::create_dir_all(project.path().join("broken")).unwrap();
        std::fs::write(project.path().join("broken/template.toml"), "accent = \"nope\"").unwrap();

        let (choices, warnings) = template_choices(Some(project.path()), global.path());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let report = choices.iter().find(|c| c.template.name.eq_ignore_ascii_case("report")).unwrap();
        assert_eq!(report.origin, Origin::Project);
        assert!(choices.iter().any(|c| c.template.name.eq_ignore_ascii_case("memo") && c.origin == Origin::Project));
        assert_eq!(choices.len(), 4);
    }

    #[test]
    fn output_names() {
        let (dir, name) = default_output(Some(Path::new("/d/notes.md")), "notes.md", ExportFormat::Pdf);
        assert_eq!((dir.as_deref(), name.as_str()), (Some(Path::new("/d")), "notes.pdf"));
        let (dir, name) = default_output(None, "Untitled-2", ExportFormat::Docx);
        assert_eq!((dir, name.as_str()), (None, "Untitled-2.docx"));
        assert_eq!(with_extension("/d/a".into(), ExportFormat::Pdf), PathBuf::from("/d/a.pdf"));
        assert_eq!(with_extension("/d/a.PDF".into(), ExportFormat::Pdf), PathBuf::from("/d/a.PDF"));
        assert_eq!(with_extension("/d/a.v2".into(), ExportFormat::Docx), PathBuf::from("/d/a.v2.docx"));
    }

    #[test]
    fn dialog_overrides_only_what_changed() {
        let (choices, _) = template_choices(None, Path::new("/nonexistent"));
        let t = choices[0].template.clone();
        let mut d = ExportDialog {
            tab: 1,
            format: ExportFormat::Pdf,
            toc: t.toc,
            numbers: t.heading_numbers,
            choices,
            selected: 0,
            page_size: None,
            include_comments: true,
            comment_count: 0,
            warnings: Vec::new(),
        };
        let o = d.options();
        assert_eq!((o.toc, o.heading_numbers, o.page_size.clone(), o.include_comments), (None, None, None, false));
        d.toc = !t.toc;
        d.page_size = Some("a5");
        d.comment_count = 2;
        let o = d.options();
        assert_eq!((o.toc, o.page_size.as_deref(), o.include_comments), (Some(!t.toc), Some("a5"), true));
    }

    #[test]
    fn comments_reanchored_and_resolved_skipped() {
        let text = "# T\n\nThe quick brown fox.\n";
        let buf = mdedit_core::buffer::RopeBuffer::from_text(text);
        let anchor = mdedit_sidecar::make_anchor(&buf, 10..15);
        let comment = |id: &str, resolved: bool| mdedit_sidecar::Comment {
            id: id.into(),
            author: "N".into(),
            created: jiff::Timestamp::UNIX_EPOCH,
            resolved,
            body: "check".into(),
            anchor: anchor.clone(),
            replies: Vec::new(),
        };
        let sidecar =
            mdedit_sidecar::Sidecar { comments: vec![comment("a", false), comment("b", true)], ..Default::default() };
        let out = export_comments(&sidecar, &buf);
        assert_eq!(out, vec![ExportComment { offset: 10, author: "N".into(), body: "check".into() }]);
    }
}
