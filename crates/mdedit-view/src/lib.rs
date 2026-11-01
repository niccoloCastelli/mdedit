//! View-mode widget (ARCHITECTURE 4.2, 4.3).
//!
//! Virtualised by block: each visible top-level block is laid out with
//! `mdedit_render::layout_block` (cached by generation) and drawn with theme
//! tokens. The block under the cursor shows styled source (Typora model);
//! edits are reported as `mdedit_editor`-style actions, never applied here.
//! In Split mode the same widget runs read-only.
//!
//! View workstream: real widget, images (off-thread decode, LRU by memory),
//! table grid editing, cursor mapping through source ranges, scroll sync.

use iced::widget::{column, scrollable, text};
use iced::{Element, Length};
use mdedit_core::{Document, Edit, Selection};
use mdedit_theme::{ColorToken, Theme};

#[derive(Debug, Clone, Default)]
pub struct ViewState {
    pub selection: Selection,
    pub scroll_y: f32,
    /// Split-mode preview: no cursor, no editing.
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Edit(Edit),
    Select(Selection),
    Scroll(f32),
    OpenLink(String),
}

/// Placeholder: lists the blocks with their kind.
pub fn view<'a, Message: 'a>(
    doc: &'a Document,
    state: &'a ViewState,
    theme: &'a Theme,
    on_action: impl Fn(Action) -> Message + 'a,
) -> Element<'a, Message> {
    let _ = (state, on_action);
    let color = theme.color(ColorToken::TextSecondary);
    let items = doc
        .blocks()
        .map(|index| index.blocks())
        .unwrap_or_default()
        .iter()
        .take(200)
        .map(|b| text(format!("{:?} {:?}", b.kind, b.bytes)).color(color).into());
    scrollable(column(items).spacing(4)).width(Length::Fill).height(Length::Fill).into()
}
