//! The tab bar (DESIGN_SYSTEM 6): the tabs, the right-click menu on a tab
//! and the overflow list at the right end when the tabs don't fit.

use iced::widget::{column, responsive, row, svg};

use super::*;
use crate::app::tabs::{CloseScope, LIST_BUTTON, TAB_STRIP, TabMenu, TabMsg};
use crate::tab::TabId;

const TAB_MENU_WIDTH: f32 = 240.0;
const LIST_WIDTH: f32 = 320.0;
const LIST_MAX_ROWS: usize = 20;

fn tm(m: TabMsg) -> Message {
    Message::Tabs(m)
}

impl App {
    pub(super) fn tab_bar(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let bar = responsive(move |size| {
            let mut tabs = row![].spacing(0);
            for (i, tab) in self.tabs.iter().enumerate() {
                tabs = tabs.push(self.tab_cell(i, tab)).push(vline(t));
            }
            let strip = scrollable(tabs)
                .id(TAB_STRIP)
                .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(2).scroller_width(2)))
                .on_scroll(|v| tm(TabMsg::Scrolled(v.absolute_offset().x)))
                .width(Length::Fill);
            let mut bar = row![strip].height(Length::Fill);
            if self.tabs_overflow(size.width) {
                let open = self.tab_bar.menu == Some(TabMenu::List);
                let glyph: Element<'_, Message> = match mdedit_theme::icon("chevron-down") {
                    Some(bytes) => svg(svg::Handle::from_memory(bytes))
                        .width(16)
                        .height(16)
                        .style(move |_, _| svg::Style { color: Some(c(t, T::TextSecondary)) })
                        .into(),
                    None => text("⌄").size(UI).into(),
                };
                let b = button(container(glyph).center(Length::Fill))
                    .width(LIST_BUTTON)
                    .height(Length::Fill)
                    .padding(0)
                    .style(style::btn(t, if open { Btn::Selected } else { Btn::Ghost }))
                    .on_press(tm(TabMsg::ToggleList));
                bar = bar.push(vline(t)).push(with_tooltip(t, b, &format!("All tabs ({})", self.tabs.len())));
            }
            container(bar).width(Length::Fill).height(TAB_HEIGHT).style(style::chrome(t)).into()
        });
        // `responsive` fills both ways; the bar is one tab high.
        container(bar).width(Length::Fill).height(TAB_HEIGHT).into()
    }

    fn tab_cell<'a>(&'a self, i: usize, tab: &'a Tab) -> Element<'a, Message> {
        let t = &self.theme;
        let active = i == self.active;
        let dirty = if tab.is_dirty() { "●" } else { "" };
        let close = button(text("×").size(UI))
            .padding(Padding::from([0.0, SPACE_1]))
            .style(style::btn(t, Btn::Ghost))
            .on_press(Message::CloseTab(i));
        let label = row![text(tab.title()).size(UI), text(dirty).size(UI_SMALL).color(c(t, T::Accent)), close]
            .spacing(SPACE_2)
            .height(Length::Fill)
            .align_y(Alignment::Center);
        let b = button(label)
            .height(TAB_HEIGHT - 2.0)
            .padding(Padding::from([0.0, SPACE_3]))
            .style(style::tab(t, active))
            .on_press(Message::SelectTab(i));
        let line = container(Space::new()).height(2).width(Length::Fill).style(style::accent_line(t, active));
        let id = tab.id;
        let cell = mouse_area(column![b, line].width(Length::Shrink)).on_middle_press(Message::CloseTab(i));
        crate::context::context_area(cell, move |p| tm(TabMsg::Context(id, p))).into()
    }

    /// The open tab menu, if any, as a floating layer.
    pub(super) fn tab_menu_layer(&self) -> Option<Element<'_, Message>> {
        let menu = self.tab_bar.menu?;
        let catcher = mouse_area(container(Space::new()).width(Length::Fill).height(Length::Fill))
            .on_press(tm(TabMsg::Dismiss))
            .on_right_press(tm(TabMsg::Dismiss));
        let (panel, x, y, width) = match menu {
            TabMenu::Context(id, at) => (self.tab_context_menu(id), at.x, at.y, TAB_MENU_WIDTH),
            TabMenu::List => {
                let right = self.window_size.width - if self.comments_visible { COMMENTS_WIDTH } else { 0.0 };
                (self.tab_list(), right - LIST_WIDTH - SPACE_1, MENU_BAR_HEIGHT + TAB_HEIGHT, LIST_WIDTH)
            }
        };
        let x = x.min(self.window_size.width - width - SPACE_1).max(0.0);
        Some(stack![catcher, pin(opaque(panel)).x(x).y(y)].into())
    }

    fn tab_context_menu(&self, id: TabId) -> Element<'_, Message> {
        let t = &self.theme;
        let item = |label: &'static str, scope: CloseScope| -> Element<'_, Message> {
            let enabled = !self.tabs_in_scope(id, scope).is_empty();
            button(container(text(label).size(UI)).height(Length::Fill).align_y(Alignment::Center))
                .width(Length::Fill)
                .height(MENU_ITEM_HEIGHT)
                .padding(Padding::from([0.0, SPACE_2]))
                .style(style::btn(t, Btn::MenuItem))
                .on_press_maybe(enabled.then_some(tm(TabMsg::Close(id, scope))))
                .into()
        };
        let close_shortcut = self.keymap.shortcut_for("file.close_tab").map(|k| k.to_string()).unwrap_or_default();
        let sep = || container(hline(t)).padding(Padding::from([SPACE_1, 0.0]));
        let close = {
            let row = row![
                text("Close").size(UI).width(Length::Fill),
                text(close_shortcut).size(UI_SMALL).color(c(t, T::TextMuted)),
            ]
            .height(Length::Fill)
            .align_y(Alignment::Center);
            button(row)
                .width(Length::Fill)
                .height(MENU_ITEM_HEIGHT)
                .padding(Padding::from([0.0, SPACE_2]))
                .style(style::btn(t, Btn::MenuItem))
                .on_press(tm(TabMsg::Close(id, CloseScope::This)))
        };
        let list = column![
            close,
            item("Close Others", CloseScope::Others),
            item("Close All", CloseScope::All),
            sep(),
            item("Close Tabs to the Left", CloseScope::Left),
            item("Close Tabs to the Right", CloseScope::Right),
        ];
        container(list).width(TAB_MENU_WIDTH).padding(SPACE_1).style(style::popover(t, RADIUS_MD)).into()
    }

    /// Every tab, the active one marked, with its folder as detail.
    fn tab_list(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let mut list = column![].spacing(0);
        for (i, tab) in self.tabs.iter().enumerate() {
            let active = i == self.active;
            let detail = tab
                .doc
                .path
                .as_deref()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let title = text(tab.title()).size(UI).font(if active { bold() } else { Font::default() });
            let row = row![
                text(if active { "✓" } else { "" }).size(UI).width(16),
                title,
                text(if tab.is_dirty() { "●" } else { "" }).size(UI_SMALL).color(c(t, T::Accent)),
                Space::new().width(Length::Fill),
                text(detail).size(UI_SMALL).color(c(t, T::TextMuted)).wrapping(iced::widget::text::Wrapping::None),
            ]
            .spacing(SPACE_1)
            .height(Length::Fill)
            .align_y(Alignment::Center);
            list = list.push(
                button(row)
                    .width(Length::Fill)
                    .height(MENU_ITEM_HEIGHT)
                    .padding(Padding::from([0.0, SPACE_2]))
                    .style(style::btn(t, Btn::MenuItem))
                    .on_press(tm(TabMsg::Pick(tab.id))),
            );
        }
        let max = LIST_MAX_ROWS as f32 * MENU_ITEM_HEIGHT;
        let body = scrollable(list)
            .height(Length::Shrink)
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(4).scroller_width(4)));
        container(container(body).max_height(max))
            .width(LIST_WIDTH)
            .padding(SPACE_1)
            .style(style::popover(t, RADIUS_MD))
            .into()
    }
}
