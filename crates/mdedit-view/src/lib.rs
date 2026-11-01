//! View-mode widget (ARCHITECTURE 4.2, 4.3; DESIGN_SYSTEM 7.1, 8.1, 8.4).
//!
//! Virtualised by block: each visible top-level block is laid out with
//! `mdedit_render::layout_block_with` (cached by content) and drawn with
//! theme tokens; a height map of blocks maps scroll offsets to blocks. The
//! block under the cursor shows styled source (Typora model) and is edited
//! as text; tables stay grids and only the active cell shows its source.
//! Edits are reported as [`Action`]s, never applied here. In Split mode the
//! same widget runs read-only.
//!
//! # Contract with the app
//!
//! - [`Action::Edit`] is always followed by [`Action::Select`] with the
//!   caret after it: apply the edit with `Document::apply`, then store the
//!   selection in [`ViewState::selection`]. Edits carry `EditOrigin::User`
//!   and `TxnId(0)`; the app assigns undo grouping. Between two rebuilds
//!   the widget emits at most one edit (later keys wait for the rebuild).
//! - [`Action::Scroll`] carries the scroll offset in px; store it in
//!   [`ViewState::scroll_y`]. Setting `scroll_y` to anything else makes the
//!   view jump there (session restore).
//! - Split mode: set [`ViewState::sync`] from the editor's
//!   `mdedit_editor::Action::TopAnchor { byte, fraction }` and the preview
//!   aligns the same block (plus the proportional offset inside it) with its
//!   top edge. [`sync_anchor_estimate`] guesses that position from a pixel
//!   offset when no anchor is at hand.
//! - Copy, cut and paste use the clipboard directly; undo, redo and other
//!   shortcuts are not captured, so the app's keymap sees them.
//! - Images resolve relative to `Document::path` and decode on worker
//!   threads into a shared LRU ([`images::cache`], 64 MB by default).

mod edit;
mod geometry;
pub mod images;
mod layout;
mod style;
mod table;
mod widget;

use iced::Element;
use mdedit_core::{Document, Edit, Selection, TextBuffer};
use mdedit_theme::Theme;

pub use widget::View;

#[derive(Debug, Clone, Default)]
pub struct ViewState {
    pub selection: Selection,
    pub scroll_y: f32,
    /// Split-mode preview: no cursor, no editing.
    pub read_only: bool,
    /// Split mode: keep this source position at the top (see the crate
    /// docs). Ignored while it does not change.
    pub sync: Option<SyncAnchor>,
    /// Use the whole width instead of the 720 px measure (DESIGN_SYSTEM 3.3).
    pub full_width: bool,
    /// Text zoom (Ctrl +/-); 0 or 1 means the design scale.
    pub zoom: f32,
}

/// A source position and how far into its line (0.0-1.0) the viewport top
/// is, for scroll sync.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncAnchor {
    pub byte: usize,
    pub fraction: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Edit(Edit),
    Select(Selection),
    Scroll(f32),
    OpenLink(String),
}

/// The View-mode (or, with `state.read_only`, Split preview) view of `doc`.
pub fn view<'a, Message: 'a>(
    doc: &'a Document,
    state: &'a ViewState,
    theme: &'a Theme,
    on_action: impl Fn(Action) -> Message + 'a,
) -> Element<'a, Message> {
    View::new(doc, state, theme, on_action).into()
}

/// Guess the source position at the top of an Edit-mode pane scrolled to
/// `editor_scroll_y`, assuming one row per line of `line_height` px (the
/// editor's body line height is 27 px at zoom 1). Good enough for Split
/// sync until the editor reports its top line.
pub fn sync_anchor_estimate(doc: &Document, editor_scroll_y: f32, line_height: f32) -> SyncAnchor {
    let buf = doc.buffer();
    let lines = buf.len_lines().unwrap_or(1).max(1);
    let pos = (editor_scroll_y.max(0.0) / line_height.max(1.0)) as f64;
    let line = (pos as usize).min(lines - 1);
    SyncAnchor { byte: buf.line_to_byte(line), fraction: (pos - line as f64) as f32 }
}

/// Byte offset of the start of the line holding `byte`.
pub(crate) fn line_start(buf: &dyn TextBuffer, byte: usize) -> usize {
    buf.line_to_byte(buf.byte_to_line(byte.min(buf.len_bytes())))
}
