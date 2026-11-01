//! Edit-mode widget (ARCHITECTURE 4.1).
//!
//! A custom `iced::advanced::Widget` that lays out and draws only the
//! visible lines of a [`Document`]: Fenwick-tree height map, shaped-line LRU
//! cache, cursor/selection/search/comment quads, gutter, IME, clipboard and
//! keyboard input. The widget **never mutates the document**: it reports
//! [`Action`]s and the app applies them, so undo, anchors and the block index
//! all see every change through `Document::apply`.
//!
//! Editor-widget workstream: replace the placeholder in [`editor`] with the
//! real widget, keeping this public API (extend it as needed, but tell the
//! app-shell workstream).

use iced::widget::{scrollable, text};
use iced::{Element, Font, Length};
use mdedit_core::{Document, Edit, Selection};
use mdedit_theme::{ColorToken, Theme};

/// State the app keeps per open document for the Edit view.
#[derive(Debug, Clone, Default)]
pub struct EditorState {
    pub selection: Selection,
    /// Scroll offset in logical px from the top of the document.
    pub scroll_y: f32,
    /// Whole buffer in monospace (setting; default is proportional prose).
    pub monospace: bool,
    pub line_numbers: bool,
    /// Byte ranges to highlight (search matches, comment anchors).
    pub highlights: Vec<Highlight>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    pub range: std::ops::Range<usize>,
    pub kind: HighlightKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    SearchMatch,
    CurrentSearchMatch,
    Comment,
}

/// What the widget asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Apply this edit (typing, paste, delete). The app assigns undo grouping.
    Edit(Edit),
    Select(Selection),
    Scroll(f32),
    /// Put text on the clipboard (copy/cut; cut also sends an `Edit`).
    Copy(String),
    /// Ctrl+V: the app reads the clipboard (images may go to assets/).
    Paste,
    Undo,
    Redo,
}

/// The Edit-mode view of `doc`. Placeholder: a scrollable, read-only text of
/// the first 500 lines.
pub fn editor<'a, Message: 'a>(
    doc: &'a Document,
    state: &'a EditorState,
    theme: &'a Theme,
    on_action: impl Fn(Action) -> Message + 'a,
) -> Element<'a, Message> {
    let _ = (state, on_action);
    let buf = doc.buffer();
    let lines = buf.len_lines().unwrap_or(0).min(500);
    let preview: String = (0..lines).map(|i| format!("{}\n", buf.line(i))).collect();
    scrollable(text(preview).font(Font::MONOSPACE).color(theme.color(ColorToken::Text)))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
