use std::path::PathBuf;
use std::sync::Arc;

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Element, Length, Task};
use ropey::Rope;

fn main() -> iced::Result {
    iced::application(Editor::new, Editor::update, Editor::view)
        .title(Editor::title)
        .run()
}

#[derive(Default)]
struct Editor {
    path: Option<PathBuf>,
    buffer: Rope,
    error: Option<String>,
}

#[derive(Debug, Clone)]
enum Message {
    Open,
    Opened(Result<(PathBuf, Arc<String>), String>),
}

impl Editor {
    fn new() -> Self {
        Self {
            buffer: Rope::from_str("# mdedit\n\nOpen a markdown file to get started.\n"),
            ..Self::default()
        }
    }

    fn title(&self) -> String {
        match &self.path {
            Some(path) => format!("{} - mdedit", path.display()),
            None => "mdedit".into(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Open => Task::perform(open_file(), Message::Opened),
            Message::Opened(Ok((path, contents))) => {
                self.buffer = Rope::from_str(&contents);
                self.path = Some(path);
                self.error = None;
                Task::none()
            }
            Message::Opened(Err(error)) => {
                self.error = Some(error);
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let headings = pulldown_cmark::Parser::new(&self.buffer.to_string())
            .filter(|event| {
                matches!(
                    event,
                    pulldown_cmark::Event::Start(pulldown_cmark::Tag::Heading { .. })
                )
            })
            .count();

        let status = text(format!(
            "{} lines, {} chars, {} headings{}",
            self.buffer.len_lines(),
            self.buffer.len_chars(),
            headings,
            self.error
                .as_ref()
                .map(|e| format!(" | error: {e}"))
                .unwrap_or_default(),
        ));

        // Placeholder view: the real editor widget will render only the
        // visible lines straight from the rope.
        let preview: String = self.buffer.lines().take(200).map(|l| l.to_string()).collect();

        column![
            row![button("Open…").on_press(Message::Open), status].spacing(12),
            container(scrollable(text(preview).font(iced::Font::MONOSPACE)))
                .width(Length::Fill)
                .height(Length::Fill),
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
    let contents = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| e.to_string())?;
    Ok((path, Arc::new(contents)))
}
