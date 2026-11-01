//! Standalone window for the Edit-mode widget, to test it without the app.
//!
//! ```bash
//! cargo run --release -p mdedit-editor --example edit -- [FILE] [options]
//! ```
//!
//! Options:
//! - `--generate MB`: open a generated markdown document of about MB megabytes
//!   instead of a file
//! - `--mono`, `--numbers`, `--dark`, `--full-width`: editor settings
//! - `--no-parse`: large-file mode (no block index, line-local styling)
//! - `--stats`: print keystroke and frame latency percentiles every 5 s
//!   and on exit (see `mdedit_editor::stats`)
//!
//! Undo/redo use `Document::undo`/`redo`; the clipboard goes through iced.

use std::time::Duration;

use iced::widget::{column, container, text};
use iced::{Element, Length, Subscription, Task, clipboard, time};
use mdedit_core::{Document, Edit, EditOrigin, RopeBuffer, Selection};
use mdedit_editor::{Action, EditorState, stats};
use mdedit_theme::{ColorToken, Theme};

struct Options {
    path: Option<String>,
    generate_mb: Option<usize>,
    mono: bool,
    numbers: bool,
    dark: bool,
    full_width: bool,
    parse: bool,
    stats: bool,
}

fn options() -> Options {
    let mut o = Options {
        path: None,
        generate_mb: None,
        mono: false,
        numbers: false,
        dark: false,
        full_width: false,
        parse: true,
        stats: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--generate" => o.generate_mb = args.next().and_then(|n| n.parse().ok()),
            "--mono" => o.mono = true,
            "--numbers" => o.numbers = true,
            "--dark" => o.dark = true,
            "--full-width" => o.full_width = true,
            "--no-parse" => o.parse = false,
            "--stats" => o.stats = true,
            _ => o.path = Some(a),
        }
    }
    o
}

/// A markdown document of about `mb` megabytes with headings, prose of
/// varied length (so lines wrap), lists, code blocks and inline styling.
pub fn generate(mb: usize) -> String {
    let words = [
        "markdown",
        "editor",
        "rope",
        "fast",
        "*light*",
        "`code`",
        "**bold**",
        "[link](https://example.com)",
        "wrap",
        "line",
        "the",
        "a",
        "of",
        "layout",
        "virtual",
        "scroll",
        "anchor",
        "glyph",
    ];
    let mut out = String::with_capacity(mb * 1_050_000);
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut section = 0;
    while out.len() < mb * 1_000_000 {
        section += 1;
        out.push_str(&format!("## Section {section}\n\n"));
        for _ in 0..3 {
            let n = 5 + (next() % 120) as usize;
            for i in 0..n {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(words[(next() % words.len() as u64) as usize]);
            }
            out.push_str(".\n\n");
        }
        for i in 0..(next() % 4) {
            out.push_str(&format!("- item {i} with some `inline code` and _emphasis_\n"));
        }
        out.push('\n');
        if section % 3 == 0 {
            out.push_str("```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\n");
        }
    }
    out
}

struct App {
    doc: Document,
    state: EditorState,
    theme: Theme,
    title: String,
    stats: bool,
}

#[derive(Debug, Clone)]
enum Message {
    Editor(Action),
    Pasted(Option<String>),
    Tick,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let o = options();
        stats::enable(o.stats);
        let (text, title) = match (&o.path, o.generate_mb) {
            (_, Some(mb)) => (generate(mb), format!("generated {mb} MB")),
            (Some(p), None) => (std::fs::read_to_string(p).expect("read file"), p.clone()),
            (None, None) => (generate(1), "generated 1 MB".into()),
        };
        let started = std::time::Instant::now();
        let doc = Document::with_buffer(Box::new(RopeBuffer::from_text(&text)), o.parse);
        eprintln!(
            "document: {} bytes, {:?} lines, built in {:.1?}",
            text.len(),
            doc.buffer().len_lines(),
            started.elapsed()
        );
        let state = EditorState {
            monospace: o.mono,
            line_numbers: o.numbers,
            full_width: o.full_width,
            ..EditorState::default()
        };
        let theme = if o.dark { Theme::ember_dark() } else { Theme::ember_light() };
        (Self { doc, state, theme, title, stats: o.stats }, Task::none())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Editor(action) => match action {
                Action::Edit(edit) => {
                    self.doc.apply(&edit);
                }
                Action::Select(sel) => self.state.selection = sel,
                Action::Scroll(y) => self.state.scroll_y = y,
                Action::TopAnchor { .. } => {}
                Action::Copy(text) => return clipboard::write(text),
                Action::Paste => return clipboard::read().map(Message::Pasted),
                Action::Undo => {
                    if let Some(sel) = self.doc.undo() {
                        self.state.selection = sel;
                    }
                }
                Action::Redo => {
                    if let Some(sel) = self.doc.redo() {
                        self.state.selection = sel;
                    }
                }
            },
            Message::Pasted(Some(text)) => {
                let range = self.state.selection.range();
                let at = range.start;
                self.doc.apply(&Edit { range, text: text.clone(), origin: EditOrigin::User, txn: Default::default() });
                self.state.selection = Selection::cursor(at + text.len());
            }
            Message::Pasted(None) => {}
            Message::Tick => print_stats(),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let buf = self.doc.buffer();
        let head = self.state.selection.head;
        let line = buf.byte_to_line(head.min(buf.len_bytes()));
        let status = text(format!(
            "{}  |  {} bytes  {} lines  |  Ln {}  sel {:?}",
            self.title,
            buf.len_bytes(),
            buf.len_lines().unwrap_or(0),
            line + 1,
            self.state.selection.range()
        ))
        .size(12)
        .color(self.theme.color(ColorToken::TextMuted));
        column![
            mdedit_editor::editor(&self.doc, &self.state, &self.theme, Message::Editor),
            container(status).padding([4, 12]).width(Length::Fill)
        ]
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.stats { time::every(Duration::from_secs(5)).map(|_| Message::Tick) } else { Subscription::none() }
    }
}

fn print_stats() {
    let (keys, frames) = stats::take();
    if let Some(s) = stats::Summary::of(&keys) {
        eprintln!("keystroke (event -> frame drawn): {s}");
    }
    if let Some(s) = stats::Summary::of(&frames) {
        eprintln!("frame (widget layout + draw):     {s}");
    }
}

fn main() -> iced::Result {
    let mut app = iced::application(App::new, App::update, App::view)
        .title(|app: &App| format!("{} - mdedit edit example", app.title))
        .subscription(App::subscription)
        .window_size((1000.0, 800.0));
    for font in mdedit_theme::fonts() {
        app = app.font(font);
    }
    let result = app.run();
    print_stats();
    result
}
