//! The window layout (DESIGN_SYSTEM 6): menu bar, sidebar, tabs, document
//! area, comments panel and status bar, plus the floating layers (menus,
//! palette, dialogs, toasts).

use iced::widget::{
    Space, button, column, container, mouse_area, opaque, pin, row, scrollable, stack, text, text_input, tooltip,
};
use iced::{Alignment, Element, Font, Length, Padding};
use mdedit_theme::space::*;
use mdedit_theme::{ColorToken as T, Theme};

use crate::app::{
    App, BannerAction, Dialog, DialogChoice, Message, PALETTE_INPUT, PaletteMsg, SEARCH_INPUT, SidebarTab,
};
use crate::command;
use crate::palette::{self, Scope};
use crate::settings::Mode;
use crate::style::{self, Btn, c};
use crate::tab::{External, Tab};

const UI: f32 = 13.0;
const UI_SMALL: f32 = 12.0;
const EYEBROW: f32 = 11.0;
const MENU_BAR_HEIGHT: f32 = 28.0;
const MENU_WIDTH: f32 = 280.0;

/// Width of a menu-bar title, fixed so dropdowns can be placed under it.
fn menu_title_width(label: &str) -> f32 {
    label.chars().count() as f32 * 7.5 + 2.0 * SPACE_3
}

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let mut middle = row![];
        if self.sidebar.visible {
            middle = middle.push(self.sidebar_view()).push(
                mouse_area(container(Space::new()).width(4).height(Length::Fill).style(style::divider(t)))
                    .on_press(Message::SidebarDragStart)
                    .interaction(iced::mouse::Interaction::ResizingHorizontally),
            );
        }
        middle = middle.push(self.document_area());
        if self.comments_visible {
            middle = middle.push(vline(t)).push(self.comments_view());
        }

        let base =
            container(column![self.menu_bar(), hline(t), middle.height(Length::Fill), hline(t), self.status_bar()])
                .width(Length::Fill)
                .height(Length::Fill)
                .style(style::app_bg(t));

        let mut layers: Vec<Element<'_, Message>> = vec![base.into()];
        if let Some(i) = self.menu_open {
            layers.push(self.menu_dropdown(i));
        }
        if self.palette.is_some() {
            layers.push(self.palette_view());
        }
        if let Some(d) = &self.dialog {
            layers.push(self.dialog_view(d));
        }
        if !self.toasts.is_empty() {
            layers.push(self.toasts_view());
        }
        stack(layers).width(Length::Fill).height(Length::Fill).into()
    }

    fn menu_bar(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let mut bar = row![].spacing(0).align_y(Alignment::Center);
        for (i, (label, _)) in command::menus().iter().enumerate() {
            let open = self.menu_open == Some(i);
            let b = button(text(*label).size(UI))
                .padding(Padding::from([0.0, SPACE_3]))
                .width(menu_title_width(label))
                .height(MENU_BAR_HEIGHT - 4.0)
                .style(style::btn(t, if open { Btn::Selected } else { Btn::MenuItem }))
                .on_press(Message::MenuOpen(Some(i)));
            bar = bar.push(mouse_area(b).on_enter(Message::MenuHover(i)));
        }
        container(bar.padding(Padding::from([2.0, SPACE_1])))
            .width(Length::Fill)
            .height(MENU_BAR_HEIGHT)
            .style(style::chrome(t))
            .into()
    }

    fn menu_dropdown(&self, index: usize) -> Element<'_, Message> {
        let t = &self.theme;
        let (_, items) = command::menus()[index];
        let mut list = column![].spacing(0);
        for item in items.iter() {
            match item {
                None => list = list.push(container(hline(t)).padding(Padding::from([SPACE_1, 0.0]))),
                Some(id) => {
                    let Some(entry) = command::find(id) else { continue };
                    let shortcut = self.keymap.shortcut_for(id).map(|k| k.to_string()).unwrap_or_default();
                    let checked = self.menu_checked(id);
                    let label = row![
                        text(if checked { "✓" } else { "" }).size(UI).width(16),
                        text(entry.title).size(UI).width(Length::Fill),
                        text(shortcut).size(UI_SMALL).color(c(t, T::TextMuted)),
                    ]
                    .align_y(Alignment::Center);
                    list = list.push(
                        button(label)
                            .width(Length::Fill)
                            .height(MENU_ITEM_HEIGHT)
                            .padding(Padding::from([0.0, SPACE_2]))
                            .style(style::btn(t, Btn::MenuItem))
                            .on_press(Message::Run(entry.id)),
                    );
                }
            }
        }
        let x: f32 = SPACE_1 + command::menus()[..index].iter().map(|(l, _)| menu_title_width(l)).sum::<f32>();
        let dropdown = container(list).width(MENU_WIDTH).padding(SPACE_1).style(style::popover(t, RADIUS_MD));
        let catcher = mouse_area(container(Space::new()).width(Length::Fill).height(Length::Fill))
            .on_press(Message::MenuOpen(None));
        stack![column![Space::new().height(MENU_BAR_HEIGHT), catcher], pin(opaque(dropdown)).x(x).y(MENU_BAR_HEIGHT),]
            .into()
    }

    fn menu_checked(&self, id: &str) -> bool {
        use crate::settings::AppearanceMode as A;
        let mode = self.active_tab().map(|t| t.mode);
        match id {
            "view.mode_edit" => mode == Some(Mode::Edit),
            "view.mode_split" => mode == Some(Mode::Split),
            "view.mode_view" => mode == Some(Mode::View),
            "view.toggle_sidebar" => self.sidebar.visible,
            "view.toggle_comments" => self.comments_visible,
            "view.toggle_full_width" => self.settings.editor.full_width,
            "view.toggle_line_numbers" => self.settings.editor.line_numbers,
            "view.appearance_system" => self.settings.appearance.mode == A::System,
            "view.appearance_light" => self.settings.appearance.mode == A::Light,
            "view.appearance_dark" => self.settings.appearance.mode == A::Dark,
            _ => false,
        }
    }

    fn tab_bar(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let mut tabs = row![].spacing(0);
        for (i, tab) in self.tabs.iter().enumerate() {
            let active = i == self.active;
            let dirty = if tab.is_dirty() { "●" } else { "" };
            let close = button(text("×").size(UI))
                .padding(Padding::from([0.0, SPACE_1]))
                .style(style::btn(t, Btn::Ghost))
                .on_press(Message::CloseTab(i));
            let label = row![text(tab.title()).size(UI), text(dirty).size(UI_SMALL).color(c(t, T::Accent)), close]
                .spacing(SPACE_2)
                .align_y(Alignment::Center);
            let b = button(label)
                .height(TAB_HEIGHT - 2.0)
                .padding(Padding::from([0.0, SPACE_3]))
                .style(style::tab(t, active))
                .on_press(Message::SelectTab(i));
            let line = container(Space::new()).height(2).width(Length::Fill).style(style::accent_line(t, active));
            let cell = mouse_area(column![b, line].width(Length::Shrink)).on_middle_press(Message::CloseTab(i));
            tabs = tabs.push(cell).push(vline(t));
        }
        container(
            scrollable(tabs)
                .direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(2).scroller_width(2))),
        )
        .width(Length::Fill)
        .height(TAB_HEIGHT)
        .style(style::chrome(t))
        .into()
    }

    fn document_area(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let Some(tab) = self.active_tab() else {
            return container(text("No document").color(c(t, T::TextMuted))).center(Length::Fill).into();
        };
        let mut col = column![self.tab_bar(), hline(t)];
        if let Some(b) = self.banner(tab) {
            col = col.push(container(b).padding(Padding::from([SPACE_2, SPACE_4])));
        }
        col.push(self.document_body(tab)).width(Length::Fill).height(Length::Fill).into()
    }

    fn document_body<'a>(&'a self, tab: &'a Tab) -> Element<'a, Message> {
        let t = &self.theme;
        let id = tab.id;
        let measure = if self.settings.editor.full_width { f32::INFINITY } else { self.settings.editor.measure };
        let column_of = |content: Element<'a, Message>| -> Element<'a, Message> {
            container(container(content).max_width(measure).width(Length::Fill).height(Length::Fill))
                .padding(Padding::from([SPACE_4, SPACE_6]))
                .center_x(Length::Fill)
                .height(Length::Fill)
                .into()
        };
        let edit = || column_of(mdedit_editor::editor(&tab.doc, &tab.editor, t, move |a| Message::Editor(id, a)));
        match tab.mode {
            Mode::Edit => edit(),
            Mode::View => column_of(mdedit_view::view(&tab.doc, &tab.view, t, move |a| Message::View(id, a))),
            Mode::Split => row![
                edit(),
                vline(t),
                column_of(mdedit_view::view(&tab.doc, &tab.preview, t, move |a| Message::View(id, a))),
            ]
            .into(),
        }
    }

    fn banner<'a>(&'a self, tab: &'a Tab) -> Option<Element<'a, Message>> {
        let t = &self.theme;
        let id = tab.id;
        let (kind, title, body, actions): (style::Banner, &str, &str, Vec<Element<'a, Message>>) = match tab.external {
            Some(External::Changed) => (
                style::Banner::Warning,
                "CHANGED ON DISK",
                "This file was changed by another program and you have unsaved edits.",
                vec![
                    small_button(t, "Reload", Btn::Secondary, Some(Message::Banner(id, BannerAction::Reload))),
                    small_button(t, "Keep mine", Btn::Primary, Some(Message::Banner(id, BannerAction::KeepMine))),
                    with_tooltip(
                        t,
                        small_button(t, "Diff", Btn::Secondary, None),
                        "The diff view comes with Git history",
                    ),
                ],
            ),
            Some(External::Deleted) => (
                style::Banner::Warning,
                "DELETED ON DISK",
                "This file was deleted or moved. Save to write it again.",
                vec![
                    small_button(t, "Keep", Btn::Primary, Some(Message::Banner(id, BannerAction::KeepMine))),
                    small_button(t, "Close tab", Btn::Secondary, Some(Message::Banner(id, BannerAction::Close))),
                ],
            ),
            None if tab.doc.format.read_only => (
                style::Banner::Info,
                "READ-ONLY",
                "This file is not valid UTF-8. It is shown with replacement characters and can't be saved.",
                vec![],
            ),
            None if tab.doc.is_large() => (
                style::Banner::Warning,
                "LARGE FILE",
                "Opened in large-file mode: Edit mode only, styling limited to visible lines.",
                vec![],
            ),
            None => return None,
        };
        let content = row![
            column![text(title).size(EYEBROW).font(bold()), text(body).size(UI)].spacing(2).width(Length::Fill),
            row(actions).spacing(SPACE_2),
        ]
        .spacing(SPACE_4)
        .align_y(Alignment::Center);
        Some(
            container(content)
                .padding(Padding::from([SPACE_3, SPACE_4]))
                .width(Length::Fill)
                .style(style::banner(t, kind))
                .into(),
        )
    }

    fn sidebar_view(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let mut tabs = row![].spacing(SPACE_1);
        for st in SidebarTab::ALL {
            let on = st == self.sidebar.tab;
            tabs = tabs.push(
                button(text(st.label()).size(UI_SMALL))
                    .padding(Padding::from([SPACE_1, SPACE_2]))
                    .style(style::btn(t, if on { Btn::Toggled } else { Btn::Ghost }))
                    .on_press(Message::Sidebar(st)),
            );
        }
        let body: Element<'_, Message> = match self.sidebar.tab {
            SidebarTab::Files => self.files_panel(),
            SidebarTab::Outline => self.outline_panel(),
            SidebarTab::Search => self.search_panel(),
            SidebarTab::Git => placeholder(t, "Branches, history and checkpoints appear here in project mode."),
        };
        container(
            column![
                container(tabs).padding(SPACE_2),
                container(
                    text(self.sidebar.tab.label().to_uppercase()).size(EYEBROW).font(bold()).color(c(t, T::TextMuted))
                )
                .padding(Padding::from([SPACE_1, SPACE_3])),
                body
            ]
            .spacing(SPACE_1),
        )
        .width(self.sidebar.width)
        .height(Length::Fill)
        .style(style::chrome(t))
        .into()
    }

    fn files_panel(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let mut col = column![
            text("Open a folder as a project to browse its files. Recent files:")
                .size(UI_SMALL)
                .color(c(t, T::TextMuted))
        ]
        .spacing(2);
        for path in self.session.recent.iter().take(20) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            col = col.push(
                tooltip(
                    sidebar_row(t, text(name).size(UI), Message::OpenPicked(vec![path.clone()]), 0.0),
                    container(text(path.display().to_string()).size(UI_SMALL))
                        .padding(SPACE_1)
                        .style(style::popover(t, RADIUS_SM)),
                    tooltip::Position::Bottom,
                )
                .delay(std::time::Duration::from_millis(500)),
            );
        }
        scrollable(col.padding(Padding::from([0.0, SPACE_2]))).height(Length::Fill).into()
    }

    fn outline_panel(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let Some(tab) = self.active_tab() else { return placeholder(t, "No document.") };
        let entries = self.outline.iter().find(|(id, _, _)| *id == tab.id).map(|(_, _, e)| e.as_slice()).unwrap_or(&[]);
        if tab.doc.is_large() {
            return placeholder(
                t,
                "The outline of large files is built in the background (coming with large-file mode).",
            );
        }
        if entries.is_empty() {
            return placeholder(t, "No headings yet.");
        }
        let cursor = tab.selection().head;
        let current = entries.iter().rposition(|e| e.byte <= cursor);
        let mut col = column![].spacing(0);
        for (i, e) in entries.iter().enumerate() {
            let color = if Some(i) == current { c(t, T::Text) } else { c(t, T::TextSecondary) };
            let indent = (e.level.saturating_sub(1)) as f32 * SPACE_3;
            col = col.push(sidebar_row(
                t,
                text(e.title.clone()).size(UI).color(color),
                Message::OutlineJump(e.byte),
                indent,
            ));
        }
        scrollable(col.padding(Padding::from([0.0, SPACE_2]))).height(Length::Fill).into()
    }

    fn search_panel(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let input = text_input("Search in document", &self.search.query)
            .id(SEARCH_INPUT)
            .on_input(Message::SearchQuery)
            .size(UI)
            .padding(SPACE_2)
            .style(style::input(t));
        let mut col = column![].spacing(0);
        let n = self.search.results.len();
        let summary = match (self.search.query.is_empty(), n) {
            (true, _) => String::new(),
            (false, 0) => "No matches".into(),
            (false, 1) => "1 match".into(),
            (false, n) => format!("{n} matches"),
        };
        for (start, end, label) in &self.search.results {
            col = col.push(sidebar_row(t, text(label.clone()).size(UI_SMALL), Message::SearchJump(*start, *end), 0.0));
        }
        column![
            container(input).padding(Padding::from([0.0, SPACE_2])),
            container(text(summary).size(UI_SMALL).color(c(t, T::TextMuted))).padding(Padding::from([0.0, SPACE_3])),
            scrollable(col.padding(Padding::from([0.0, SPACE_2]))).height(Length::Fill),
        ]
        .spacing(SPACE_2)
        .into()
    }

    fn comments_view(&self) -> Element<'_, Message> {
        let t = &self.theme;
        container(
            column![
                text("COMMENTS").size(EYEBROW).font(bold()).color(c(t, T::TextMuted)),
                text("No comments yet. Select text and add one with Ctrl+Alt+M once comment support lands.")
                    .size(UI_SMALL)
                    .color(c(t, T::TextMuted)),
            ]
            .spacing(SPACE_3),
        )
        .padding(SPACE_4)
        .width(COMMENTS_WIDTH)
        .height(Length::Fill)
        .style(style::chrome(t))
        .into()
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let muted = c(t, T::TextMuted);
        let mut left = row![].spacing(SPACE_3).align_y(Alignment::Center);
        let mut right = row![].spacing(SPACE_3).align_y(Alignment::Center);
        if let Some(tab) = self.active_tab() {
            let (label, color) = if tab.saving {
                ("saving…", muted)
            } else if tab.is_dirty() {
                ("● unsaved", c(t, T::Accent))
            } else if tab.doc.path.is_some() {
                ("saved", c(t, T::Success))
            } else {
                ("new file", muted)
            };
            left = left.push(text(label).size(UI_SMALL).color(color));
            if tab.doc.format.read_only {
                left = left.push(text("read-only").size(UI_SMALL).color(c(t, T::WarningText)));
            }
            if tab.doc.is_large() {
                left = left.push(text("large file").size(UI_SMALL).color(c(t, T::WarningText)));
            }
            if tab.doc.format.line_ending == mdedit_core::io::LineEnding::CrLf {
                left = left.push(text("CRLF").size(UI_SMALL).color(muted));
            }
            if tab.doc.format.bom {
                left = left.push(text("BOM").size(UI_SMALL).color(muted));
            }

            let buf = tab.doc.buffer();
            let head = tab.selection().head.min(buf.len_bytes());
            let line = buf.byte_to_line(head);
            let line_text = buf.line(line);
            let off = (head - buf.line_to_byte(line)).min(line_text.len());
            let col = line_text.get(..off).map_or(off, |s| s.chars().count()) + 1;
            right = right.push(text(format!("Ln {}, Col {col}", line + 1)).size(UI_SMALL).color(muted));
            if let Some((_, _, s)) = self.stats.iter().find(|(id, _, _)| *id == tab.id) {
                let minutes = s.words.div_ceil(230).max(1);
                right = right.push(
                    text(format!("{} words · {} chars · {minutes} min read", s.words, s.chars))
                        .size(UI_SMALL)
                        .color(muted),
                );
            }
            let zoom = self.settings.appearance.zoom;
            if (zoom - 1.0).abs() > 0.01 {
                right = right.push(text(format!("{:.0}%", zoom * 100.0)).size(UI_SMALL).color(muted));
            }
            let mut modes = row![].spacing(2);
            for m in [Mode::Edit, Mode::Split, Mode::View] {
                let on = tab.mode == m;
                let enabled = m == Mode::Edit || !tab.doc.is_large();
                let b = button(text(m.label()).size(UI_SMALL))
                    .padding(Padding::from([0.0, SPACE_2]))
                    .height(STATUS_BAR_HEIGHT - 4.0)
                    .style(style::btn(t, if on { Btn::Toggled } else { Btn::Ghost }))
                    .on_press_maybe(enabled.then_some(Message::SetMode(m)));
                let tip = if enabled {
                    let id = match m {
                        Mode::Edit => "view.mode_edit",
                        Mode::Split => "view.mode_split",
                        Mode::View => "view.mode_view",
                    };
                    let key = self.keymap.shortcut_for(id).map(|k| format!(" ({k})")).unwrap_or_default();
                    format!("{} mode{key}", m.label())
                } else {
                    "Large files open in Edit mode only".into()
                };
                modes = modes.push(with_tooltip(t, b, &tip));
            }
            right = right.push(modes);
        }
        container(
            row![left, Space::new().width(Length::Fill), right]
                .align_y(Alignment::Center)
                .padding(Padding::from([0.0, SPACE_3])),
        )
        .height(STATUS_BAR_HEIGHT)
        .width(Length::Fill)
        .center_y(STATUS_BAR_HEIGHT)
        .style(style::chrome(t))
        .into()
    }

    fn palette_view(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let Some(p) = &self.palette else { return Space::new().into() };
        let placeholder = match p.scope {
            Scope::Commands => "Type a command",
            Scope::Files => "Go to file (open tabs and recent files)",
        };
        let input = text_input(placeholder, &p.query)
            .id(PALETTE_INPUT)
            .on_input(|q| Message::Palette(PaletteMsg::Query(q)))
            .on_submit(Message::Palette(PaletteMsg::Submit))
            .size(14)
            .padding(SPACE_2)
            .style(style::input(t));
        let items = self.palette_items();
        let mut list = column![].spacing(0);
        for (i, item) in items.iter().take(palette::MAX_ROWS).enumerate() {
            let selected = i == p.selected;
            let detail = item.detail().unwrap_or_default().to_string();
            list = list.push(
                button(
                    row![
                        text(item.title().to_string()).size(UI).width(Length::Fill),
                        text(detail).size(UI_SMALL).color(c(t, T::TextMuted)),
                    ]
                    .spacing(SPACE_3)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .height(MENU_ITEM_HEIGHT)
                .padding(Padding::from([0.0, SPACE_2]))
                .style(style::btn(t, if selected { Btn::Selected } else { Btn::MenuItem }))
                .on_press(Message::Palette(PaletteMsg::Pick(i))),
            );
        }
        if items.is_empty() {
            list = list.push(container(text("No matches").size(UI).color(c(t, T::TextMuted))).padding(SPACE_2));
        }
        let card = container(column![input, list].spacing(SPACE_2))
            .width(560)
            .padding(SPACE_2)
            .style(style::popover(t, RADIUS_LG));
        modal(t, card, Message::Palette(PaletteMsg::Close), false)
    }

    fn dialog_view(&self, dialog: &Dialog) -> Element<'_, Message> {
        let t = &self.theme;
        let (title, body, actions): (String, String, Vec<Element<'_, Message>>) = match dialog {
            Dialog::UnsavedTab(id) => {
                let name = self.tabs.iter().find(|x| x.id == *id).map(|x| x.title()).unwrap_or_default();
                (
                    format!("Save changes to {name}?"),
                    "Your changes will be lost if you don't save them.".into(),
                    vec![
                        small_button(t, "Don't save", Btn::Danger, Some(Message::Dialog(DialogChoice::Discard))),
                        small_button(t, "Cancel", Btn::Secondary, Some(Message::Dialog(DialogChoice::Cancel))),
                        small_button(t, "Save", Btn::Primary, Some(Message::Dialog(DialogChoice::Save))),
                    ],
                )
            }
            Dialog::UnsavedQuit(n) => (
                if *n == 1 {
                    "1 document has unsaved changes".into()
                } else {
                    format!("{n} documents have unsaved changes")
                },
                "Save them before quitting?".into(),
                vec![
                    small_button(t, "Quit without saving", Btn::Danger, Some(Message::Dialog(DialogChoice::Discard))),
                    small_button(t, "Cancel", Btn::Secondary, Some(Message::Dialog(DialogChoice::Cancel))),
                    small_button(t, "Save all", Btn::Primary, Some(Message::Dialog(DialogChoice::Save))),
                ],
            ),
            Dialog::About => (
                format!("mdedit {}", env!("CARGO_PKG_VERSION")),
                "A fast, lightweight markdown editor.".into(),
                vec![small_button(t, "Close", Btn::Primary, Some(Message::Dialog(DialogChoice::Cancel)))],
            ),
        };
        let card = container(
            column![
                text(title).size(16).font(bold()),
                text(body).size(UI).color(c(t, T::TextSecondary)),
                row![Space::new().width(Length::Fill), row(actions).spacing(SPACE_2)],
            ]
            .spacing(SPACE_4),
        )
        .width(480)
        .padding(SPACE_5)
        .style(style::popover(t, RADIUS_LG));
        modal(t, card, Message::Dialog(DialogChoice::Cancel), true)
    }

    fn toasts_view(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let mut col = column![].spacing(SPACE_2).align_x(Alignment::End);
        for toast in &self.toasts {
            let color = if toast.error { c(t, T::Danger) } else { c(t, T::Text) };
            col = col.push(
                container(text(toast.text.clone()).size(UI).color(color))
                    .max_width(420)
                    .padding(Padding::from([SPACE_2, SPACE_3]))
                    .style(style::popover(t, RADIUS_MD)),
            );
        }
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(Padding { top: 0.0, right: SPACE_4, bottom: STATUS_BAR_HEIGHT + SPACE_4, left: 0.0 })
            .into()
    }
}

fn bold() -> Font {
    Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT }
}

fn hline(t: &Theme) -> Element<'_, Message> {
    container(Space::new()).width(Length::Fill).height(1).style(style::divider(t)).into()
}

fn vline(t: &Theme) -> Element<'_, Message> {
    container(Space::new()).width(1).height(Length::Fill).style(style::divider(t)).into()
}

fn placeholder<'a>(t: &'a Theme, msg: &'a str) -> Element<'a, Message> {
    container(text(msg).size(UI_SMALL).color(c(t, T::TextMuted))).padding(Padding::from([0.0, SPACE_3])).into()
}

fn sidebar_row<'a>(t: &'a Theme, label: iced::widget::Text<'a>, msg: Message, indent: f32) -> Element<'a, Message> {
    button(label)
        .width(Length::Fill)
        .height(24)
        .padding(Padding { top: 3.0, right: SPACE_2, bottom: 3.0, left: SPACE_2 + indent })
        .style(style::btn(t, Btn::MenuItem))
        .on_press(msg)
        .into()
}

fn small_button<'a>(t: &'a Theme, label: &'a str, kind: Btn, msg: Option<Message>) -> Element<'a, Message> {
    button(text(label).size(UI))
        .height(28)
        .padding(Padding::from([SPACE_1, SPACE_3]))
        .style(style::btn(t, kind))
        .on_press_maybe(msg)
        .into()
}

fn with_tooltip<'a>(t: &'a Theme, content: impl Into<Element<'a, Message>>, tip: &str) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(tip.to_string()).size(UI_SMALL))
            .padding(Padding::from([SPACE_1, SPACE_2]))
            .style(style::popover(t, RADIUS_SM)),
        tooltip::Position::Top,
    )
    .delay(std::time::Duration::from_millis(500))
    .into()
}

/// A floating card over a dimmed backdrop; clicking the backdrop sends
/// `on_dismiss`. Dialogs are centred, the palette sits near the top.
fn modal<'a>(
    t: &'a Theme,
    card: impl Into<Element<'a, Message>>,
    on_dismiss: Message,
    centred: bool,
) -> Element<'a, Message> {
    let backdrop = mouse_area(container(Space::new()).width(Length::Fill).height(Length::Fill).style(style::scrim(t)))
        .on_press(on_dismiss);
    let placed = container(opaque(card)).width(Length::Fill).height(Length::Fill).center_x(Length::Fill);
    let placed =
        if centred { placed.center_y(Length::Fill) } else { placed.padding(Padding { top: 64.0, ..Padding::ZERO }) };
    stack![backdrop, placed].into()
}
