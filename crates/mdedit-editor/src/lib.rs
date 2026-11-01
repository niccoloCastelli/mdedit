//! Edit-mode widget (ARCHITECTURE 4.1).
//!
//! A custom `iced::advanced::Widget` that lays out and draws only the
//! visible lines of a [`Document`]: Fenwick-tree height map, shaped-line LRU
//! cache, cursor/selection/search/comment quads, gutter, IME, clipboard and
//! keyboard input. The widget **never mutates the document**: it reports
//! [`Action`]s and the app applies them, so undo, anchors and the block index
//! all see every change through `Document::apply`.
//!
//! # Contract with the app
//!
//! - [`Action::Edit`] is always followed by an [`Action::Select`] with the
//!   caret after the edit. Apply the edit with `Document::apply`, then store
//!   the selection in [`EditorState::selection`].
//! - [`Action::Scroll`] carries the new scroll offset; store it in
//!   [`EditorState::scroll_y`]. Setting `scroll_y` to anything else makes the
//!   widget jump there (restoring a session, Split-mode sync). Not storing it
//!   is fine too: the widget keeps its own scroll anchor.
//! - Every `Scroll` is followed by an [`Action::TopAnchor`] naming the source
//!   position at the top of the viewport (Split-mode sync, ARCHITECTURE 4.3).
//!   [`EditorState::scroll_to_byte`] is the reverse direction.
//! - [`Action::Copy`] asks the app to put text on the clipboard (the widget
//!   does not write the clipboard itself); cut sends `Copy` then `Edit`.
//!   [`Action::Paste`] asks the app to read the clipboard and send the text
//!   back as an edit (images may go to `assets/` instead).
//! - Shortcuts the widget does not handle (Ctrl+B, Ctrl+S, ...) are not
//!   captured, so the app's keymap sees them.
//! - Edits carry `EditOrigin::User` and `TxnId(0)`; the app assigns undo
//!   grouping.
//!
//! Between two view rebuilds the widget emits at most one `Edit`: further
//! key presses in the same event batch are queued and replayed after the app
//! has applied the first one, so every edit is computed on current text.

mod cache;
mod geometry;
mod height_map;
mod motion;
pub mod stats;
mod style;
mod widget;

use iced::Element;
use mdedit_core::{Document, Edit, Selection};
use mdedit_theme::Theme;

pub use height_map::HeightMap;
pub use widget::Editor;

/// State the app keeps per open document for the Edit view.
#[derive(Debug, Clone, Default)]
pub struct EditorState {
    pub selection: Selection,
    /// Scroll offset in logical px from the top of the document.
    pub scroll_y: f32,
    /// Whole buffer in monospace (setting; default is proportional prose).
    pub monospace: bool,
    pub line_numbers: bool,
    /// Byte ranges to highlight (search matches, comment anchors), sorted by
    /// start.
    pub highlights: Vec<Highlight>,
    /// Use the whole width instead of the 720 px measure (DESIGN_SYSTEM 3.3).
    pub full_width: bool,
    /// Scroll so the line holding this byte is at the top of the viewport
    /// (Split sync driven by the preview). The widget jumps whenever the
    /// value differs from the one in the previous view; set it back to
    /// `None` to jump to the same byte again later.
    pub scroll_to_byte: Option<usize>,
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
    /// Sent after every `Scroll`: `byte` is the start of the line at the top
    /// of the viewport and `fraction` (0..1) how far into that line's height
    /// the viewport top is. Unlike the pixel offset, this maps to the source.
    TopAnchor {
        byte: usize,
        fraction: f32,
    },
    /// Put text on the clipboard (copy/cut; cut also sends an `Edit`).
    Copy(String),
    /// Ctrl+V: the app reads the clipboard (images may go to assets/).
    Paste,
    Undo,
    Redo,
    /// Window-space bounds of the visible part of the selection, sent when
    /// it changes (for the floating format toolbar). `None` when the
    /// selection is empty, scrolled away or the editor is not focused.
    SelectionRect(Option<iced::Rectangle>),
}

/// The Edit-mode view of `doc`. Use [`Editor::new`] to set a widget id
/// (for focusing it with `iced::widget::operation::focus`).
pub fn editor<'a, Message: 'a>(
    doc: &'a Document,
    state: &'a EditorState,
    theme: &'a Theme,
    on_action: impl Fn(Action) -> Message + 'a,
) -> Element<'a, Message> {
    Editor::new(doc, state, theme, on_action).into()
}
