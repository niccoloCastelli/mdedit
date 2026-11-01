//! Slash menu (DESIGN_SYSTEM 7 and 8.2): typing `/` at the start of a line
//! or after whitespace, in Edit, Split or View mode, opens a filterable list
//! of the commands that have a slash keyword (`/h1`, `/table`, `/comment`,
//! ...). The text typed after the slash is the filter; Up/Down move, Enter
//! or Tab runs the command (the `/query` text is removed first), Esc or a
//! space closes the menu and leaves the text as typed.
//!
//! The app hooks in through `App.slash`, `Message::Slash`, `slash_key` (top
//! of `on_key`), `after_slash_update`, `slash_caret`, `slash_layer`, and the
//! document area's `nav_blocked` so the arrows and Enter reach the menu
//! instead of the text.

use iced::keyboard::{Key, key::Named};
use iced::widget::{Space, button, column, container, mouse_area, opaque, pin, row, stack, text};
use iced::{Alignment, Element, Length, Padding, Rectangle, Task};
use mdedit_core::{Edit, EditOrigin, Selection, TextBuffer};
use mdedit_theme::space::*;
use mdedit_theme::{ColorToken as T, Theme};

use crate::app::{App, Message};
use crate::settings::Mode;
use crate::style::{self, Btn, c};
use crate::tab::TabId;

/// DESIGN_SYSTEM 7: at most 8 rows visible.
pub const MAX_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 32.0;
const WIDTH: f32 = 280.0;
/// A filter longer than this is prose, not a command name.
const MAX_QUERY: usize = 24;

#[derive(Debug, Clone)]
pub enum SlashMsg {
    Pick(usize),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashItem {
    pub keyword: &'static str,
    pub title: &'static str,
    pub icon: Option<&'static str>,
    /// Command id run on pick.
    pub id: &'static str,
}

/// Every slash command: the document commands with a keyword, plus
/// `/comment` (an app command).
pub fn items() -> Vec<SlashItem> {
    let mut v: Vec<SlashItem> = mdedit_core::command::registry()
        .iter()
        .filter_map(|m| Some(SlashItem { keyword: m.slash?, title: m.title, icon: m.icon, id: m.id.0 }))
        .collect();
    v.push(SlashItem { keyword: "comment", title: "Comment", icon: Some("message-square-plus"), id: "comment.add" });
    v
}

/// Items for `query`: keyword prefix matches first, then keyword or title
/// substring matches.
pub fn filter(query: &str) -> Vec<SlashItem> {
    let q = query.to_lowercase();
    let all = items();
    let mut out: Vec<SlashItem> = all.iter().filter(|i| i.keyword.starts_with(&q)).copied().collect();
    for i in &all {
        if !out.contains(i) && (i.keyword.contains(&q) || i.title.to_lowercase().contains(&q)) {
            out.push(*i);
        }
    }
    out
}

#[derive(Debug)]
pub struct SlashMenu {
    pub tab: TabId,
    /// Byte offset of the `/`.
    pub start: usize,
    pub query: String,
    pub selected: usize,
    /// Window-space caret, reported by the document widget.
    pub caret: Option<Rectangle>,
}

#[derive(Debug, Default)]
pub struct SlashState {
    pub menu: Option<SlashMenu>,
    /// (tab, edit log version) last looked at for a typed `/`.
    seen: Option<(TabId, u64)>,
}

impl App {
    /// Keys while the menu is open. Arrows, Enter and Tab never reach the
    /// document then (`nav_blocked`), so they arrive here uncaptured.
    pub(crate) fn slash_key(&mut self, key: &Key) -> Option<Task<Message>> {
        let menu = self.slash.menu.as_mut()?;
        let n = filter(&menu.query).len().clamp(1, MAX_ROWS);
        match key {
            Key::Named(Named::ArrowDown) => menu.selected = (menu.selected + 1) % n,
            Key::Named(Named::ArrowUp) => menu.selected = (menu.selected + n - 1) % n,
            Key::Named(Named::Enter) | Key::Named(Named::Tab) => {
                let i = menu.selected;
                return Some(self.on_slash(SlashMsg::Pick(i)));
            }
            Key::Named(Named::Escape) => self.close_slash(),
            _ => return None,
        }
        Some(Task::none())
    }

    pub(crate) fn on_slash(&mut self, msg: SlashMsg) -> Task<Message> {
        match msg {
            SlashMsg::Close => self.close_slash(),
            SlashMsg::Pick(i) => {
                let Some(menu) = self.slash.menu.take() else { return Task::none() };
                self.set_report_caret(menu.tab, false);
                let Some(item) = filter(&menu.query).get(i).copied() else { return Task::none() };
                let Some(tab) = self.tabs.iter_mut().find(|t| t.id == menu.tab) else { return Task::none() };
                let end = menu.start + 1 + menu.query.len();
                let txn = tab.doc.next_txn();
                tab.apply(&Edit::delete(menu.start..end, EditOrigin::User, txn));
                tab.set_selection(Selection::cursor(menu.start));
                // The deletion is not a new `/` to look at.
                self.slash.seen = Some((menu.tab, tab.doc.log().last_version()));
                return Task::batch([self.run(item.id), self.focus_document()]);
            }
        }
        Task::none()
    }

    fn close_slash(&mut self) {
        if let Some(m) = self.slash.menu.take() {
            self.set_report_caret(m.tab, false);
        }
    }

    fn set_report_caret(&mut self, tab: TabId, on: bool) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
            t.editor.report_caret = on;
            t.view.report_caret = on;
        }
    }

    /// A document widget reported its caret. The menu stays where the `/`
    /// was typed rather than sliding along with the filter.
    pub(crate) fn slash_caret(&mut self, tab: TabId, rect: Option<Rectangle>) {
        if let Some(m) = self.slash.menu.as_mut().filter(|m| m.tab == tab && m.caret.is_none()) {
            m.caret = rect;
        }
    }

    /// After every update: open the menu when a `/` was just typed, keep its
    /// filter in step with the text, close it when the caret leaves.
    pub(crate) fn after_slash_update(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            self.close_slash();
            return;
        };
        let (id, version) = (tab.id, tab.doc.log().last_version());
        let usable = tab.editable() && !tab.doc.is_large() && self.palette.is_none() && !self.modal_open();
        if let Some(m) = &self.slash.menu {
            let sel = tab.selection();
            let buf = tab.doc.buffer();
            let open = usable
                && m.tab == id
                && sel.is_empty()
                && sel.head > m.start
                && sel.head - m.start <= MAX_QUERY + 1
                && buf.slice(m.start..m.start + 1) == "/";
            let query = if open { buf.slice(m.start + 1..sel.head).into_owned() } else { String::new() };
            if !open || query.chars().any(char::is_whitespace) || filter(&query).is_empty() {
                self.close_slash();
            } else if let Some(m) = &mut self.slash.menu
                && m.query != query
            {
                m.query = query;
                m.selected = 0;
            }
        }
        if self.slash.seen == Some((id, version)) {
            return;
        }
        let last_seen = self.slash.seen.filter(|(t, _)| *t == id).map(|(_, v)| v);
        self.slash.seen = Some((id, version));
        if self.slash.menu.is_some() || !usable || last_seen.is_none() {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else { return };
        let Some(last) = tab.doc.log().since(version.saturating_sub(1)).and_then(|it| it.last()) else {
            return;
        };
        let typed_slash = last.edit.origin == EditOrigin::User && last.edit.text == "/" && last.edit.range.is_empty();
        let start = last.edit.range.start;
        if !typed_slash {
            return;
        }
        if start > 0 && !opens_after(tab.doc.buffer(), start) {
            return;
        }
        if !matches!(tab.mode, Mode::Edit | Mode::Split | Mode::View) {
            return;
        }
        self.slash.menu = Some(SlashMenu { tab: id, start, query: String::new(), selected: 0, caret: None });
        self.set_report_caret(id, true);
    }

    /// The menu under the caret (above it when there is no room below).
    pub(crate) fn slash_layer(&self) -> Option<Element<'_, Message>> {
        let m = self.slash.menu.as_ref()?;
        let caret = m.caret?;
        let t = &self.theme;
        let found = filter(&m.query);
        let mut list = column![].spacing(0);
        for (i, item) in found.iter().take(MAX_ROWS).enumerate() {
            list = list.push(slash_row(t, item, i == m.selected, i));
        }
        let rows = found.len().min(MAX_ROWS) as f32;
        let height = rows * ROW_HEIGHT + 2.0 * SPACE_1 + 2.0;
        let x = caret.x.min(self.window_size.width - WIDTH - SPACE_1).max(SPACE_1);
        let below = caret.y + caret.height + 4.0;
        let y =
            if below + height <= self.window_size.height - 24.0 { below } else { (caret.y - 4.0 - height).max(0.0) };
        let card = container(list).padding(SPACE_1).width(WIDTH).style(style::popover(t, RADIUS_MD));
        let catcher = mouse_area(container(Space::new()).width(Length::Fill).height(Length::Fill))
            .on_press(Message::Slash(SlashMsg::Close));
        Some(stack![catcher, pin(opaque(card)).x(x).y(y)].into())
    }
}

/// The `/` at `start` follows whitespace (or starts the line).
fn opens_after(buf: &dyn TextBuffer, start: usize) -> bool {
    let from = start.saturating_sub(4);
    let before = buf.slice(from..start);
    before.chars().next_back().is_none_or(char::is_whitespace)
}

fn slash_row<'a>(t: &'a Theme, item: &SlashItem, selected: bool, i: usize) -> Element<'a, Message> {
    let icon: Element<'a, Message> =
        match item.icon.and_then(mdedit_theme::icon).map(iced::widget::svg::Handle::from_memory) {
            Some(h) => iced::widget::svg(h)
                .width(16)
                .height(16)
                .style(move |_, _| iced::widget::svg::Style { color: Some(c(t, T::TextSecondary)) })
                .into(),
            None => Space::new().width(16).into(),
        };
    let label = row![
        icon,
        text(item.title).size(13.0).color(c(t, T::Text)),
        Space::new().width(Length::Fill),
        text(format!("/{}", item.keyword)).size(12.0).color(c(t, T::TextMuted)),
    ]
    .spacing(SPACE_2)
    .height(Length::Fill)
    .align_y(Alignment::Center);
    button(label)
        .width(Length::Fill)
        .height(ROW_HEIGHT)
        .padding(Padding::from([0.0, SPACE_2]))
        .style(style::btn(t, if selected { Btn::Selected } else { Btn::MenuItem }))
        .on_press(Message::Slash(SlashMsg::Pick(i)))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_cover_the_design_table() {
        let all = items();
        for k in ["link", "h1", "h6", "list", "numbered", "todo", "quote", "code", "table", "image", "hr", "comment"] {
            assert!(all.iter().any(|i| i.keyword == k), "missing /{k}");
        }
        for i in &all {
            assert!(crate::command::find(i.id).is_some(), "/{} runs unknown command {}", i.keyword, i.id);
        }
    }

    #[test]
    fn filter_prefers_prefixes() {
        let f = filter("co");
        assert!(f.len() >= 2);
        assert!(f[0].keyword.starts_with("co") && f[1].keyword.starts_with("co"));
        assert_eq!(filter("comm")[0].id, "comment.add");
        assert!(filter("zzz").is_empty());
        assert_eq!(filter("").len(), items().len());
    }

    #[test]
    fn slash_opens_after_whitespace_only() {
        let doc = mdedit_core::Document::from_text("a /b x/");
        assert!(opens_after(doc.buffer(), 2));
        assert!(!opens_after(doc.buffer(), 6));
    }
}
