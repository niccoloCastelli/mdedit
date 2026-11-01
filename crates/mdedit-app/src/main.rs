//! mdedit application shell.
//!
//! Placeholder shell wired to the workspace crates: open a file, switch
//! between Edit / Split / View. The app-shell workstream replaces this with
//! the real layout (DESIGN_SYSTEM 6): menu bar, tabs, sidebar, status bar,
//! command registry, keymap and settings.

use std::path::PathBuf;
use std::sync::Arc;

use iced::widget::{button, column, container, row, text};
use iced::{Element, Length, Task};
use mdedit_core::Document;
use mdedit_editor::EditorState;
use mdedit_theme::{ColorToken, Theme};
use mdedit_view::ViewState;

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view).title(App::title).run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Edit,
    Split,
    View,
}

struct App {
    doc: Document,
    editor: EditorState,
    preview: ViewState,
    theme: Theme,
    mode: Mode,
    error: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    Open,
    Opened(Result<(PathBuf, Arc<String>), String>),
    SetMode(Mode),
    Editor(mdedit_editor::Action),
    View(mdedit_view::Action),
}

impl App {
    fn new() -> Self {
        Self {
            doc: Document::from_text("# mdedit\n\nOpen a markdown file to get started.\n"),
            editor: EditorState::default(),
            preview: ViewState::default(),
            theme: Theme::ember_light(),
            mode: Mode::Edit,
            error: None,
        }
    }

    fn title(&self) -> String {
        match &self.doc.path {
            Some(path) => format!("{} - mdedit", path.display()),
            None => "mdedit".into(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Open => return Task::perform(open_file(), Message::Opened),
            Message::Opened(Ok((path, contents))) => {
                self.doc = Document::from_text(&contents);
                self.doc.path = Some(path);
                self.editor = EditorState::default();
                self.error = None;
            }
            Message::Opened(Err(error)) => self.error = Some(error),
            Message::SetMode(mode) => self.mode = mode,
            Message::Editor(mdedit_editor::Action::Edit(edit)) | Message::View(mdedit_view::Action::Edit(edit)) => {
                self.doc.apply(&edit);
            }
            Message::Editor(_) | Message::View(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let blocks = self.doc.blocks().map_or(0, |b| b.blocks().len());
        let status = text(format!(
            "{} lines, {} bytes, {} blocks{}",
            self.doc.buffer().len_lines().unwrap_or(0),
            self.doc.buffer().len_bytes(),
            blocks,
            self.error.as_ref().map(|e| format!(" | error: {e}")).unwrap_or_default(),
        ))
        .color(self.theme.color(ColorToken::TextMuted));

        let mode_button = |label, mode| button(text(label)).on_press(Message::SetMode(mode));
        let edit = || mdedit_editor::editor(&self.doc, &self.editor, &self.theme, Message::Editor);
        let preview = || mdedit_view::view(&self.doc, &self.preview, &self.theme, Message::View);
        let body: Element<'_, Message> = match self.mode {
            Mode::Edit => edit(),
            Mode::View => preview(),
            Mode::Split => row![edit(), preview()].spacing(16).into(),
        };

        column![
            row![
                button("Open…").on_press(Message::Open),
                mode_button("Edit", Mode::Edit),
                mode_button("Split", Mode::Split),
                mode_button("View", Mode::View),
                status
            ]
            .spacing(12),
            container(body).width(Length::Fill).height(Length::Fill),
        ]
        .spacing(8)
        .padding(8)
        .into()
    }
}

async fn open_file() -> Result<(PathBuf, Arc<String>), String> {
    let handle = rfd::AsyncFileDialog::new()
        .add_filter("Markdown", &["md", "markdown", "txt"])
        .pick_file()
        .await
        .ok_or_else(|| "no file selected".to_string())?;
    let path = handle.path().to_path_buf();
    let contents = tokio::fs::read_to_string(&path).await.map_err(|e| e.to_string())?;
    Ok((path, Arc::new(contents)))
}
