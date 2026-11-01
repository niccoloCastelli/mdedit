//! Project and git UI (DESIGN_SYSTEM 6, 7): the file tree, the Git panel,
//! the status bar segment, the text-input dialog and the history overlay.

use iced::Color;
use iced::widget::{column, row, svg};

use super::*;
use crate::app::project::{DiffKind, HistoryView, Msg, PROMPT_INPUT, Prompt, PromptMsg, time_label};

const ROW: f32 = 24.0;
const INDENT: f32 = 12.0;

fn icon<'a>(t: &'a Theme, name: &str, size: f32, color: T) -> Element<'a, Message> {
    match mdedit_theme::icon(name) {
        Some(bytes) => svg(svg::Handle::from_memory(bytes))
            .width(size)
            .height(size)
            .style(move |_, _| svg::Style { color: Some(c(t, color)) })
            .into(),
        None => Space::new().width(size).into(),
    }
}

/// 24×24 ghost icon button with a tooltip.
fn tool<'a>(t: &'a Theme, name: &str, tip: &str, msg: Option<Message>) -> Element<'a, Message> {
    let b = button(container(icon(t, name, 14.0, T::TextSecondary)).center(Length::Fill))
        .width(ROW)
        .height(ROW)
        .padding(0)
        .style(style::btn(t, Btn::Ghost))
        .on_press_maybe(msg);
    with_tooltip(t, b, tip)
}

fn pm(m: Msg) -> Message {
    Message::Project(m)
}

impl App {
    pub(super) fn project_files_panel(&self) -> Option<Element<'_, Message>> {
        let t = &self.theme;
        let p = self.project.as_ref()?;
        let name = if p.project.config.project.name.is_empty() {
            p.project.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            p.project.config.project.name.clone()
        };
        let has_selection = p.selected.is_some();
        let header = row![
            text(name).size(UI).font(bold()).wrapping(iced::widget::text::Wrapping::None).width(Length::Fill),
            tool(t, "file-plus", "New file", Some(pm(Msg::NewFile))),
            tool(t, "folder-plus", "New folder", Some(pm(Msg::NewFolder))),
            tool(t, "pencil", "Rename or move", has_selection.then(|| pm(Msg::Rename))),
            tool(t, "trash", "Delete", has_selection.then(|| pm(Msg::Delete))),
            tool(t, "x", "Close project", Some(Message::Run("file.close_folder"))),
        ]
        .spacing(2)
        .align_y(Alignment::Center)
        .padding(Padding::from([0.0, SPACE_2]));

        let active = self.active_tab().and_then(|t| t.doc.path.as_deref());
        let mut col = column![].spacing(0);
        for r in p.tree.rows() {
            let path = &r.entry.path;
            let selected = p.selected.as_deref() == Some(path.as_path());
            let open = active == Some(path.as_path());
            let (glyph, glyph_color) = if r.entry.is_dir {
                (if r.expanded { "chevron-down" } else { "chevron-right" }, T::TextMuted)
            } else if r.entry.is_markdown() {
                ("file-text", T::TextMuted)
            } else {
                ("file", T::TextMuted)
            };
            let color = if open { c(t, T::Text) } else { c(t, T::TextSecondary) };
            let label = text(r.entry.name())
                .size(UI)
                .color(color)
                .wrapping(iced::widget::text::Wrapping::None)
                .height(Length::Fill)
                .align_y(Alignment::Center);
            let content = row![icon(t, glyph, 14.0, glyph_color), container(label).clip(true)]
                .spacing(SPACE_1)
                .align_y(Alignment::Center)
                .height(Length::Fill);
            col = col.push(
                button(content)
                    .width(Length::Fill)
                    .height(ROW)
                    .padding(Padding { top: 0.0, right: SPACE_2, bottom: 0.0, left: SPACE_2 + r.depth as f32 * INDENT })
                    .style(style::btn(t, if selected { Btn::Selected } else { Btn::MenuItem }))
                    .on_press(pm(Msg::Click(path.clone()))),
            );
        }
        if p.tree.rows().is_empty() {
            col = col.push(placeholder(t, "The folder is empty. Create a file with the + button."));
        }
        Some(
            column![header, scrollable(col.padding(Padding::from([0.0, SPACE_2]))).height(Length::Fill)]
                .spacing(SPACE_1)
                .into(),
        )
    }

    pub(super) fn git_panel(&self) -> Element<'_, Message> {
        let t = &self.theme;
        let muted = c(t, T::TextMuted);
        let Some(p) = &self.project else {
            return placeholder(t, "Open a folder as a project (File → Open Folder…) to version it with git.");
        };
        let Some(g) = &p.git else {
            return column![
                placeholder(t, "This project is not versioned yet."),
                container(small_button(t, "Initialize git", Btn::Primary, p.busy.is_none().then(|| pm(Msg::InitGit))))
                    .padding(Padding::from([0.0, SPACE_3])),
            ]
            .spacing(SPACE_2)
            .into();
        };
        let busy = p.busy.is_some();
        let s = &g.status;
        let mut col = column![].spacing(SPACE_2).padding(Padding::from([0.0, SPACE_3]));

        // Actions.
        let sync_tip = if mdedit_git::push_available() {
            "Checkpoint if needed, pull, then push"
        } else {
            "Pull only: pushing is not available in this build"
        };
        col = col.push(
            row![
                small_button(t, "Checkpoint…", Btn::Primary, (!busy).then(|| pm(Msg::Checkpoint))),
                with_tooltip(
                    t,
                    small_button(
                        t,
                        if p.busy == Some("sync") { "Syncing…" } else { "Sync" },
                        Btn::Secondary,
                        (!busy).then(|| pm(Msg::Sync))
                    ),
                    sync_tip
                ),
            ]
            .spacing(SPACE_2),
        );
        let mut facts = Vec::new();
        if s.pending_autocommits > 0 {
            facts.push(format!(
                "{} autocommit{} since the last checkpoint",
                s.pending_autocommits,
                if s.pending_autocommits == 1 { "" } else { "s" }
            ));
        }
        if !s.changed_files.is_empty() {
            facts.push(format!(
                "{} uncommitted file{}",
                s.changed_files.len(),
                if s.changed_files.len() == 1 { "" } else { "s" }
            ));
        }
        if s.ahead > 0 || s.behind > 0 {
            facts.push(format!("{} ahead, {} behind the remote", s.ahead, s.behind));
        }
        if facts.is_empty() {
            facts.push("Everything is committed.".into());
        }
        for f in facts {
            col = col.push(text(f).size(UI_SMALL).color(muted));
        }
        if let Some(n) = &p.notice {
            col = col.push(
                container(text(n.clone()).size(UI_SMALL))
                    .padding(SPACE_2)
                    .width(Length::Fill)
                    .style(style::banner(t, style::Banner::Warning)),
            );
        }

        // Branches.
        col = col.push(
            row![
                text("BRANCHES").size(EYEBROW).font(bold()).color(muted).width(Length::Fill),
                tool(t, "plus", "New branch", (!busy).then(|| pm(Msg::NewBranch))),
            ]
            .align_y(Alignment::Center),
        );
        let mut branches = column![].spacing(0);
        for b in &g.branches {
            let current = *b == s.branch;
            let label = text(b.clone())
                .size(UI)
                .color(if current { c(t, T::Accent) } else { c(t, T::TextSecondary) })
                .font(if current { bold() } else { Font::DEFAULT })
                .wrapping(iced::widget::text::Wrapping::None);
            let name = button(
                row![
                    icon(t, "git-branch", 14.0, if current { T::Accent } else { T::TextMuted }),
                    container(label).clip(true)
                ]
                .spacing(SPACE_1)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .height(ROW)
            .padding(Padding::from([0.0, SPACE_1]))
            .style(style::btn(t, Btn::MenuItem))
            .on_press_maybe((!current && !busy).then(|| pm(Msg::Switch(b.clone()))));
            let delete: Element<'_, Message> = if current {
                Space::new().width(ROW).into()
            } else {
                tool(t, "trash", "Delete branch", (!busy).then(|| pm(Msg::DeleteBranch(b.clone()))))
            };
            let r = row![
                name,
                tool(t, "pencil", "Rename branch", (!busy).then(|| pm(Msg::RenameBranch(b.clone())))),
                delete
            ]
            .align_y(Alignment::Center);
            branches = branches.push(r);
        }
        col = col.push(branches);

        // History.
        col = col.push(
            row![
                text("HISTORY").size(EYEBROW).font(bold()).color(muted).width(Length::Fill),
                tool(t, "rotate-ccw-clock", "History of the current file", Some(pm(Msg::FileHistory))),
            ]
            .align_y(Alignment::Center),
        );
        let mut hist = column![].spacing(2);
        if g.history.is_empty() {
            hist = hist.push(text("No commits yet.").size(UI_SMALL).color(muted));
        }
        for commit in &g.history {
            let open = g.expanded.contains(&commit.id);
            let mut head = row![].spacing(SPACE_1).align_y(Alignment::Center);
            if !commit.autocommits.is_empty() {
                head = head.push(icon(t, if open { "chevron-down" } else { "chevron-right" }, 12.0, T::TextMuted));
            }
            let summary_color = if commit.is_checkpoint { c(t, T::Text) } else { c(t, T::TextSecondary) };
            head = head.push(
                container(
                    text(commit.summary.clone())
                        .size(UI)
                        .color(summary_color)
                        .wrapping(iced::widget::text::Wrapping::None),
                )
                .clip(true)
                .width(Length::Fill),
            );
            if let Some(tag) = &commit.tag {
                head = head.push(
                    container(text(tag.clone()).size(EYEBROW).color(c(t, T::Accent)))
                        .padding(Padding::from([0.0, SPACE_1]))
                        .style(move |_| container::Style {
                            background: Some(c(t, T::AccentSoft).into()),
                            border: iced::Border { radius: RADIUS_SM.into(), ..Default::default() },
                            ..Default::default()
                        }),
                );
            }
            let when = text(time_label(commit.time_unix)).size(EYEBROW).color(muted);
            let entry = column![head, when].spacing(0);
            let b = button(entry)
                .width(Length::Fill)
                .padding(Padding::from([2.0, SPACE_1]))
                .style(style::btn(t, Btn::MenuItem))
                .on_press_maybe((!commit.autocommits.is_empty()).then(|| pm(Msg::ToggleCommit(commit.id.clone()))));
            hist = hist.push(b);
            if open {
                for a in &commit.autocommits {
                    hist = hist.push(
                        container(
                            column![
                                text(a.summary.clone())
                                    .size(UI_SMALL)
                                    .color(c(t, T::TextSecondary))
                                    .wrapping(iced::widget::text::Wrapping::None),
                                text(time_label(a.time_unix)).size(EYEBROW).color(muted),
                            ]
                            .spacing(0),
                        )
                        .clip(true)
                        .padding(Padding { top: 0.0, right: 0.0, bottom: 0.0, left: SPACE_4 }),
                    );
                }
            }
        }
        col = col.push(hist);
        scrollable(col).height(Length::Fill).into()
    }

    /// Status bar: the branch (before the save state) and the autocommit
    /// state (after it).
    pub(super) fn project_status(&self) -> (Option<Element<'_, Message>>, Option<Element<'_, Message>>) {
        let t = &self.theme;
        let muted = c(t, T::TextMuted);
        let Some(p) = &self.project else { return (None, None) };
        let (mut branch_el, mut state) = (None, None);
        if let Some(g) = &p.git {
            let branch = if g.status.branch.is_empty() { "…".to_string() } else { g.status.branch.clone() };
            let b = button(
                row![icon(t, "git-branch", 12.0, T::TextSecondary), text(branch).size(UI_SMALL)]
                    .spacing(SPACE_1)
                    .align_y(Alignment::Center),
            )
            .padding(Padding::from([0.0, SPACE_1]))
            .height(STATUS_BAR_HEIGHT - 4.0)
            .style(style::btn(t, Btn::Ghost))
            .on_press(Message::Sidebar(SidebarTab::Git));
            branch_el = Some(with_tooltip(t, b, "Branches and history"));
            if let Some(op) = p.busy {
                let label = match op {
                    "sync" => "syncing…",
                    "pull" => "pulling…",
                    "switch" => "switching…",
                    _ => "working…",
                };
                state = Some(text(label).size(UI_SMALL).color(muted).into());
            } else if let Some(secs) = self.autocommit_due() {
                state = Some(text(format!("autocommit in {secs}s")).size(UI_SMALL).color(muted).into());
            } else if p.committed {
                state = Some(text("committed").size(UI_SMALL).color(c(t, T::Success)).into());
            }
        }
        (branch_el, state)
    }

    pub(super) fn prompt_view(&self, p: &Prompt) -> Element<'_, Message> {
        let t = &self.theme;
        let mut body = column![text(p.title.clone()).size(16).font(bold())].spacing(SPACE_3);
        if !p.body.is_empty() {
            body = body.push(text(p.body.clone()).size(UI).color(c(t, T::TextSecondary)));
        }
        if let Some(v) = &p.input {
            body = body.push(
                text_input("", v)
                    .id(PROMPT_INPUT)
                    .on_input(|s| pm(Msg::Prompt(PromptMsg::Input(s))))
                    .on_submit(pm(Msg::Prompt(PromptMsg::Submit)))
                    .padding(SPACE_2)
                    .size(UI)
                    .style(style::input(t)),
            );
        }
        if let Some((label, v)) = &p.input2 {
            body = body.push(
                column![
                    text(*label).size(UI_SMALL).color(c(t, T::TextMuted)),
                    text_input("", v)
                        .on_input(|s| pm(Msg::Prompt(PromptMsg::Input2(s))))
                        .on_submit(pm(Msg::Prompt(PromptMsg::Submit)))
                        .padding(SPACE_2)
                        .size(UI)
                        .style(style::input(t)),
                ]
                .spacing(SPACE_1),
            );
        }
        let confirm = small_button(
            t,
            p.confirm,
            if p.danger { Btn::Danger } else { Btn::Primary },
            Some(pm(Msg::Prompt(PromptMsg::Submit))),
        );
        body = body.push(row![
            Space::new().width(Length::Fill),
            row![small_button(t, "Cancel", Btn::Secondary, Some(pm(Msg::Prompt(PromptMsg::Cancel)))), confirm]
                .spacing(SPACE_2)
        ]);
        let card = container(body.spacing(SPACE_4)).width(480).padding(SPACE_5).style(style::popover(t, RADIUS_LG));
        modal(t, card, pm(Msg::Prompt(PromptMsg::Cancel)), true)
    }

    /// File history: versions on the left, the diff to the current text on
    /// the right (DESIGN_SYSTEM 7, "Diff view").
    pub(super) fn history_view(&self, h: &HistoryView) -> Element<'_, Message> {
        let t = &self.theme;
        let muted = c(t, T::TextMuted);
        let name = h.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut list = column![].spacing(2);
        match &h.commits {
            None => list = list.push(text("Loading…").size(UI_SMALL).color(muted)),
            Some(c0) if c0.is_empty() => {
                list = list.push(text("No committed versions yet.").size(UI_SMALL).color(muted))
            }
            Some(commits) => {
                for (i, commit) in commits.iter().enumerate() {
                    let on = h.selected == Some(i);
                    let mut title = row![
                        container(text(commit.summary.clone()).size(UI).wrapping(iced::widget::text::Wrapping::None))
                            .clip(true)
                            .width(Length::Fill)
                    ];
                    if let Some(tag) = &commit.tag {
                        title = title.push(text(tag.clone()).size(EYEBROW).color(c(t, T::Accent)));
                    }
                    list = list.push(
                        button(column![title, text(time_label(commit.time_unix)).size(EYEBROW).color(muted)])
                            .width(Length::Fill)
                            .padding(Padding::from([SPACE_1, SPACE_2]))
                            .style(style::btn(t, if on { Btn::Selected } else { Btn::MenuItem }))
                            .on_press(pm(Msg::PickVersion(i))),
                    );
                }
            }
        }
        let mono = Font::MONOSPACE;
        let mut diff = column![].spacing(0);
        match &h.diff {
            None if h.selected.is_some() => diff = diff.push(text("Comparing…").size(UI_SMALL).color(muted)),
            None => {}
            Some(lines) if lines.iter().all(|l| matches!(l.kind, DiffKind::Same | DiffKind::Skipped)) => {
                diff = diff.push(text("Same as the current text.").size(UI_SMALL).color(muted));
            }
            Some(lines) => {
                for l in lines.iter().take(5000) {
                    let (sign, bg, fg) = match l.kind {
                        DiffKind::Same => (" ", None, c(t, T::Text)),
                        DiffKind::Added => ("+", Some(tint(c(t, T::Success))), c(t, T::Text)),
                        DiffKind::Removed => ("−", Some(tint(c(t, T::Danger))), c(t, T::Text)),
                        DiffKind::Skipped => ("⋯", None, muted),
                    };
                    let line = row![
                        text(sign).size(UI).font(mono).color(muted).width(16),
                        text(l.text.clone()).size(UI).font(mono).color(fg).wrapping(iced::widget::text::Wrapping::None)
                    ];
                    diff = diff.push(
                        container(line)
                            .clip(true)
                            .width(Length::Fill)
                            .padding(Padding::from([1.0, SPACE_2]))
                            .style(move |_| container::Style { background: bg.map(Into::into), ..Default::default() }),
                    );
                }
            }
        }
        let can_restore = h.selected.is_some()
            && h.diff.as_ref().is_some_and(|d| d.iter().any(|l| matches!(l.kind, DiffKind::Added | DiffKind::Removed)));
        let header = row![
            column![
                text(format!("History of {name}")).size(16).font(bold()),
                text("Lines marked − are in the selected version; + lines were added since.")
                    .size(UI_SMALL)
                    .color(muted),
            ]
            .spacing(2)
            .width(Length::Fill),
            small_button(t, "Restore this version", Btn::Secondary, can_restore.then(|| pm(Msg::Restore))),
            small_button(t, "Close", Btn::Primary, Some(pm(Msg::CloseHistory))),
        ]
        .spacing(SPACE_2)
        .align_y(Alignment::Center);
        let body = row![
            scrollable(list).width(260).height(Length::Fill),
            vline(t),
            scrollable(diff).width(Length::Fill).height(Length::Fill),
        ]
        .spacing(SPACE_2);
        let w = (self.window_size.width - 80.0).clamp(600.0, 1100.0);
        let hgt = (self.window_size.height - 120.0).clamp(400.0, 800.0);
        let card = container(column![header, hline(t), body].spacing(SPACE_3))
            .width(w)
            .height(hgt)
            .padding(SPACE_4)
            .style(style::popover(t, RADIUS_LG));
        modal(t, card, pm(Msg::CloseHistory), true)
    }
}

/// `success` / `danger` line backgrounds at 12 % (DESIGN_SYSTEM 7).
fn tint(color: Color) -> Color {
    Color { a: 0.12, ..color }
}
