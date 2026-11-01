//! Standalone window for the View-mode widget, to test it without the app.
//!
//! ```bash
//! cargo run --release -p mdedit-view --example view -- [FILE] [options]
//! ```
//!
//! Options:
//! - `--split`: Edit-mode editor on the left, read-only preview on the
//!   right, scroll-synced through the editor's `TopAnchor`
//! - `--read-only`, `--dark`, `--full-width`
//!
//! Ctrl+Z and Ctrl+Shift+Z undo and redo through `Document::undo`/`redo`;
//! Ctrl+S prints the document's source to stderr.

use iced::keyboard::{self, Key};
use iced::widget::{column, container, row, text};
use iced::{Element, Length, Subscription, Task, clipboard};
use mdedit_core::{Document, RopeBuffer};
use mdedit_theme::{ColorToken, Theme};
use mdedit_view::{Action, SyncAnchor, ViewState};

struct App {
    doc: Document,
    state: ViewState,
    editor: mdedit_editor::EditorState,
    split: bool,
    theme: Theme,
    title: String,
}

#[derive(Debug, Clone)]
enum Message {
    View(Action),
    Editor(mdedit_editor::Action),
    Pasted(Option<String>),
    Undo,
    Redo,
    Dump,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let mut path = None;
        let (mut split, mut read_only, mut dark, mut full_width) = (false, false, false, false);
        for a in std::env::args().skip(1) {
            match a.as_str() {
                "--split" => split = true,
                "--read-only" => read_only = true,
                "--dark" => dark = true,
                "--full-width" => full_width = true,
                _ => path = Some(a),
            }
        }
        let path = path.unwrap_or_else(|| {
            concat!(env!("CARGO_MANIFEST_DIR"), "/../mdedit-export/examples/sample/sample.md").to_owned()
        });
        let text = std::fs::read_to_string(&path).expect("read file");
        let mut doc = Document::with_buffer(Box::new(RopeBuffer::from_text(&text)), true);
        doc.path = Some(std::fs::canonicalize(&path).unwrap_or(path.clone().into()));
        let state = ViewState { read_only: read_only || split, full_width, ..ViewState::default() };
        let theme = if dark { Theme::ember_dark() } else { Theme::ember_light() };
        let editor = mdedit_editor::EditorState { full_width, ..Default::default() };
        (Self { doc, state, editor, split, theme, title: path }, Task::none())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(action) => match action {
                Action::Edit(edit) => {
                    self.doc.apply(&edit);
                }
                Action::Select(sel) => self.state.selection = sel,
                Action::Scroll(y) => self.state.scroll_y = y,
                Action::OpenLink(url) => eprintln!("open link: {url}"),
            },
            Message::Editor(action) => match action {
                mdedit_editor::Action::Edit(edit) => {
                    self.doc.apply(&edit);
                }
                mdedit_editor::Action::Select(sel) => self.editor.selection = sel,
                mdedit_editor::Action::Scroll(y) => self.editor.scroll_y = y,
                mdedit_editor::Action::TopAnchor { byte, fraction } => {
                    self.state.sync = Some(SyncAnchor { byte, fraction });
                }
                mdedit_editor::Action::Copy(t) => return clipboard::write(t),
                mdedit_editor::Action::Paste => return clipboard::read().map(Message::Pasted),
                mdedit_editor::Action::Undo => return Task::done(Message::Undo),
                mdedit_editor::Action::Redo => return Task::done(Message::Redo),
            },
            Message::Pasted(Some(t)) => {
                let range = self.editor.selection.range();
                let at = range.start;
                self.doc.apply(&mdedit_core::Edit {
                    range,
                    text: t.clone(),
                    origin: mdedit_core::EditOrigin::User,
                    txn: Default::default(),
                });
                self.editor.selection = mdedit_core::Selection::cursor(at + t.len());
            }
            Message::Pasted(None) => {}
            Message::Dump => eprintln!("----\n{}\n----", self.doc.buffer().slice(0..self.doc.buffer().len_bytes())),
            Message::Undo => {
                if let Some(sel) = self.doc.undo() {
                    self.state.selection = sel;
                    self.editor.selection = sel;
                }
            }
            Message::Redo => {
                if let Some(sel) = self.doc.redo() {
                    self.state.selection = sel;
                    self.editor.selection = sel;
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let preview = mdedit_view::view(&self.doc, &self.state, &self.theme, Message::View);
        let body: Element<'_, Message> = if self.split {
            row![mdedit_editor::editor(&self.doc, &self.editor, &self.theme, Message::Editor), preview].into()
        } else {
            preview
        };
        let status = text(format!(
            "{}  |  {} bytes  |  sel {:?}  scroll {:.0}",
            self.title,
            self.doc.buffer().len_bytes(),
            self.state.selection.range(),
            self.state.scroll_y
        ))
        .size(12)
        .color(self.theme.color(ColorToken::TextMuted));
        column![body, container(status).padding([4, 12]).width(Length::Fill)].into()
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.split {
            // The editor reports undo and redo itself.
            return Subscription::none();
        }
        keyboard::listen().filter_map(|event| match event {
            keyboard::Event::KeyPressed { key: Key::Character(c), modifiers, .. }
                if modifiers.command() && matches!(c.as_str(), "z" | "s") =>
            {
                Some(match (c.as_str(), modifiers.shift()) {
                    ("s", _) => Message::Dump,
                    (_, true) => Message::Redo,
                    (_, false) => Message::Undo,
                })
            }
            _ => None,
        })
    }
}

fn main() -> iced::Result {
    let mut app = iced::application(App::new, App::update, App::view)
        .title(|app: &App| format!("{} - mdedit view example", app.title))
        .subscription(App::subscription)
        .window_size((1100.0, 850.0));
    for font in mdedit_theme::fonts() {
        app = app.font(font);
    }
    app.run()
}
