//! The comments panel (DESIGN_SYSTEM 6 and 7, "Comment card"): threads of
//! the active document in document order, a "Detached" group for comments
//! whose text is gone, the box for a new comment, replies, resolve and
//! delete. State and messages are in `comments.rs`.

use iced::keyboard::{Key, key::Named};
use iced::widget::text_editor::{self, Binding, KeyPress};
use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Background, Border, Element, Length, Padding};
use mdedit_sidecar::Comment;
use mdedit_theme::space::*;
use mdedit_theme::{ColorToken as T, Theme};

use super::{EYEBROW, UI, UI_SMALL, bold};
use crate::app::{App, Message};
use crate::comments::{self, CommentMsg, DRAFT_INPUT, REPLY_INPUT};
use crate::style::{self, Btn, c};
use crate::tab::Tab;

/// Longest quote shown above a card, in chars.
const QUOTE_CHARS: usize = 90;

impl App {
    pub(super) fn comments_view(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let body: Element<'_, Message> = match self.active_tab() {
            Some(tab) => self.comment_threads(tab),
            None => muted(t, "No document open."),
        };
        container(body).width(COMMENTS_WIDTH).height(Length::Fill).style(style::chrome(t)).into()
    }

    fn comment_threads<'a>(&'a self, tab: &'a Tab) -> Element<'a, Message> {
        let t = &self.theme;
        let ui = &self.comments;
        let doc = &tab.comments;
        let (attached, detached) = doc.ordered(ui.show_resolved);
        let open = doc.sidecar.comments.iter().filter(|c| !c.resolved).count();
        let resolved = doc.resolved_count();

        let mut header = row![
            text("COMMENTS").size(EYEBROW).font(bold()).color(c(t, T::TextMuted)),
            text(if open > 0 { open.to_string() } else { String::new() }).size(EYEBROW).color(c(t, T::AccentText)),
            Space::new().width(Length::Fill),
        ]
        .spacing(SPACE_2)
        .align_y(Alignment::Center)
        .height(28);
        if resolved > 0 {
            let label =
                if ui.show_resolved { format!("Hide resolved ({resolved})") } else { format!("Resolved ({resolved})") };
            header = header.push(
                button(text(label).size(UI_SMALL))
                    .padding(Padding::from([2.0, SPACE_2]))
                    .style(style::btn(t, if ui.show_resolved { Btn::Toggled } else { Btn::Ghost }))
                    .on_press(Message::Comments(CommentMsg::ShowResolved(!ui.show_resolved))),
            );
        }

        let mut list = column![].spacing(SPACE_3);
        if !comments::DocComments::available(&tab.doc) {
            list = list.push(muted(t, "Comments are not available for large files."));
        }
        if let Some(e) = &doc.load_error {
            let msg = format!("The comments file could not be read: {e}. It is left as it is.");
            list = list.push(
                container(text(msg).size(UI_SMALL).color(c(t, T::WarningText)))
                    .padding(SPACE_2)
                    .width(Length::Fill)
                    .style(style::banner(t, style::Banner::Warning)),
            );
        }
        if let Some(d) = ui.draft.as_ref().filter(|d| d.tab == tab.id) {
            list = list.push(self.draft_card(&d.quote, &d.content));
        }
        let active = ui.active_for(tab.id);
        for (comment, range) in &attached {
            let quote = tab.doc.buffer().slice(range.clone()).into_owned();
            list = list.push(self.comment_card(tab, comment, Some(&quote), active == Some(comment.id.as_str())));
        }
        if !detached.is_empty() {
            list = list.push(
                row![
                    text("DETACHED").size(EYEBROW).font(bold()).color(c(t, T::TextMuted)),
                    badge(t, detached.len().to_string()),
                ]
                .spacing(SPACE_2)
                .align_y(Alignment::Center),
            );
            list = list.push(muted(t, "The text these comments were attached to is gone."));
            for comment in &detached {
                list = list.push(self.comment_card(tab, comment, None, active == Some(comment.id.as_str())));
            }
        }
        if attached.is_empty() && detached.is_empty() && ui.draft.as_ref().is_none_or(|d| d.tab != tab.id) {
            let shortcut =
                self.keymap.shortcut_for("comment.add").map(|k| format!(" or press {k}")).unwrap_or_default();
            let msg = if open + resolved == 0 {
                format!("No comments yet. Select text and use the comment button{shortcut}.")
            } else {
                "All comments are resolved.".to_string()
            };
            list = list.push(muted(t, msg));
        }

        column![
            container(header).padding(Padding::from([SPACE_2, SPACE_4])),
            scrollable(container(list).padding(Padding { top: 0.0, right: SPACE_4, bottom: SPACE_4, left: SPACE_4 }))
                .height(Length::Fill),
        ]
        .into()
    }

    fn draft_card<'a>(&'a self, quote: &str, content: &'a text_editor::Content) -> Element<'a, Message> {
        let t = &self.theme;
        let input = comment_input(t, content, "Add a comment…", DRAFT_INPUT, CommentMsg::DraftAction, || {
            (CommentMsg::DraftSubmit, CommentMsg::DraftCancel)
        });
        let can_send = !content.text().trim().is_empty();
        let actions = row![
            Space::new().width(Length::Fill),
            small(t, "Cancel", Btn::Ghost, Some(CommentMsg::DraftCancel)),
            small(t, "Comment", Btn::Primary, can_send.then_some(CommentMsg::DraftSubmit)),
        ]
        .spacing(SPACE_2);
        card(t, true, column![quote_line(t, quote), input, actions].spacing(SPACE_2))
    }

    fn comment_card<'a>(
        &'a self,
        tab: &'a Tab,
        comment: &'a Comment,
        quote: Option<&str>,
        active: bool,
    ) -> Element<'a, Message> {
        let t = &self.theme;
        let ui = &self.comments;
        let id = &comment.id;
        let mut col = column![].spacing(SPACE_2);
        match quote {
            Some(q) => col = col.push(quote_line(t, q)),
            None => {
                col = col.push(row![badge(t, "Detached".into()), quote_line(t, &comment.anchor.quote)].spacing(SPACE_2))
            }
        }
        col = col.push(byline(t, &comment.author, comment.created, comment.resolved));
        col = col.push(text(comment.body.clone()).size(UI).color(c(t, T::Text)));
        for r in &comment.replies {
            col = col.push(
                container(
                    column![byline(t, &r.author, r.created, false), text(r.body.clone()).size(UI).color(c(t, T::Text))]
                        .spacing(SPACE_1),
                )
                .padding(Padding { left: SPACE_3, ..Padding::ZERO }),
            );
        }
        let replying = ui.reply.as_ref().filter(|r| r.tab == tab.id && r.id == *id);
        if let Some(r) = replying {
            let input = comment_input(t, &r.content, "Reply…", REPLY_INPUT, CommentMsg::ReplyAction, || {
                (CommentMsg::ReplySubmit, CommentMsg::ReplyCancel)
            });
            let can_send = !r.content.text().trim().is_empty();
            col = col.push(input).push(
                row![
                    Space::new().width(Length::Fill),
                    small(t, "Cancel", Btn::Ghost, Some(CommentMsg::ReplyCancel)),
                    small(t, "Reply", Btn::Primary, can_send.then_some(CommentMsg::ReplySubmit)),
                ]
                .spacing(SPACE_2),
            );
        } else {
            let confirm = ui.confirm_delete.as_deref() == Some(id.as_str());
            let mut actions = row![].spacing(SPACE_1).align_y(Alignment::Center);
            actions = actions.push(small(t, "Reply", Btn::Ghost, Some(CommentMsg::StartReply(id.clone()))));
            actions = actions.push(if comment.resolved {
                small(t, "Reopen", Btn::Ghost, Some(CommentMsg::Resolve(id.clone(), false)))
            } else {
                small(t, "Resolve", Btn::Ghost, Some(CommentMsg::Resolve(id.clone(), true)))
            });
            actions = actions.push(Space::new().width(Length::Fill));
            actions = actions.push(if confirm {
                small(t, "Delete?", Btn::Danger, Some(CommentMsg::Delete(id.clone())))
            } else {
                small(t, "Delete", Btn::Ghost, Some(CommentMsg::Delete(id.clone())))
            });
            col = col.push(actions);
        }
        mouse_area(card(t, active, col)).on_press(Message::Comments(CommentMsg::Select(id.clone()))).into()
    }
}

fn card<'a>(t: &'a Theme, active: bool, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding(SPACE_3)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(c(t, T::Surface))),
            border: Border {
                color: c(t, if active { T::Accent } else { T::Border }),
                width: 1.0,
                radius: RADIUS_MD.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// The commented text: one line, cut, behind a `warning-text` bar (the
/// margin marker colour of DESIGN_SYSTEM 2.2).
fn quote_line<'a>(t: &'a Theme, quote: &str) -> Element<'a, Message> {
    let one_line = quote.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut shown: String = one_line.chars().take(QUOTE_CHARS).collect();
    if one_line.chars().count() > QUOTE_CHARS {
        shown.push('…');
    }
    row![
        container(Space::new()).width(3).height(Length::Fill).style(move |_| container::Style {
            background: Some(Background::Color(c(t, T::WarningText))),
            border: Border { radius: 2.0.into(), ..Border::default() },
            ..container::Style::default()
        }),
        text(shown).size(UI_SMALL).color(c(t, T::TextSecondary)).wrapping(iced::widget::text::Wrapping::None),
    ]
    .spacing(SPACE_2)
    .height(18)
    .clip(true)
    .into()
}

fn byline<'a>(t: &'a Theme, author: &str, created: jiff::Timestamp, resolved: bool) -> Element<'a, Message> {
    let mut r = row![
        text(author.to_string()).size(UI).font(bold()).color(c(t, T::Text)),
        text(comments::ago(created)).size(UI_SMALL).color(c(t, T::TextMuted)),
    ]
    .spacing(SPACE_2)
    .align_y(Alignment::Center);
    if resolved {
        r = r.push(text("Resolved").size(UI_SMALL).color(c(t, T::Success)));
    }
    r.into()
}

fn badge<'a>(t: &'a Theme, label: String) -> Element<'a, Message> {
    container(text(label).size(EYEBROW).color(c(t, T::WarningText)))
        .padding(Padding::from([1.0, 6.0]))
        .style(move |_| container::Style {
            background: Some(Background::Color(c(t, T::WarningBg))),
            border: Border { radius: RADIUS_FULL.into(), ..Border::default() },
            ..container::Style::default()
        })
        .into()
}

fn muted<'a>(t: &'a Theme, msg: impl Into<String>) -> Element<'a, Message> {
    text(msg.into()).size(UI_SMALL).color(c(t, T::TextMuted)).into()
}

fn small<'a>(t: &'a Theme, label: &'a str, kind: Btn, msg: Option<CommentMsg>) -> Element<'a, Message> {
    button(text(label).size(UI_SMALL))
        .height(24)
        .padding(Padding::from([SPACE_1, SPACE_2]))
        .style(style::btn(t, kind))
        .on_press_maybe(msg.map(Message::Comments))
        .into()
}

/// A multi-line box: Enter sends, Shift+Enter breaks the line, Esc cancels.
fn comment_input<'a>(
    t: &'a Theme,
    content: &'a text_editor::Content,
    placeholder: &'a str,
    id: &'static str,
    on_action: fn(text_editor::Action) -> CommentMsg,
    submit_cancel: fn() -> (CommentMsg, CommentMsg),
) -> Element<'a, Message> {
    iced::widget::text_editor(content)
        .id(id)
        .placeholder(placeholder)
        .size(UI)
        .padding(SPACE_2)
        .min_height(56)
        .max_height(200)
        .on_action(move |a| Message::Comments(on_action(a)))
        .key_binding(move |kp: KeyPress| {
            let focused = matches!(kp.status, text_editor::Status::Focused { .. });
            let (submit, cancel) = submit_cancel();
            match kp.key {
                Key::Named(Named::Enter) if focused && !kp.modifiers.shift() => {
                    Some(Binding::Custom(Message::Comments(submit)))
                }
                Key::Named(Named::Escape) if focused => Some(Binding::Custom(Message::Comments(cancel))),
                _ => Binding::from_key_press(kp),
            }
        })
        .style(move |_, status| {
            let focused = matches!(status, text_editor::Status::Focused { .. });
            text_editor::Style {
                background: Background::Color(c(t, T::Surface)),
                border: Border {
                    color: c(t, if focused { T::Accent } else { T::BorderStrong }),
                    width: 1.0,
                    radius: RADIUS_MD.into(),
                },
                placeholder: c(t, T::TextMuted),
                value: c(t, T::Text),
                selection: c(t, T::Selection),
            }
        })
        .into()
}
