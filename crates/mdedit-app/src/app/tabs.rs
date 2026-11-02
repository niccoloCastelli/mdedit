//! Tab bar extras: the right-click menu on a tab (close it, the others, all,
//! or those to its left or right) and the list of every tab at the right
//! end of the bar when they don't all fit (JetBrains style).

use super::*;

/// The scrollable holding the tabs.
pub const TAB_STRIP: &str = "tab-strip";
/// Width of the overflow list button at the right end of the tab bar.
pub const LIST_BUTTON: f32 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TabMenu {
    /// Right-click on a tab, at a window position.
    Context(TabId, Point),
    /// The list of all tabs, under the overflow button.
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseScope {
    This,
    Others,
    All,
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub enum TabMsg {
    Context(TabId, Point),
    ToggleList,
    Dismiss,
    Close(TabId, CloseScope),
    Pick(TabId),
    /// The tab strip scrolled: its horizontal offset.
    Scrolled(f32),
}

/// Tab bar state kept between frames.
#[derive(Debug, Default)]
pub struct TabBar {
    pub menu: Option<TabMenu>,
    /// Horizontal scroll offset of the tab strip.
    offset: f32,
    /// Dirty tabs of a multi-close still waiting for their unsaved dialog.
    close_queue: Vec<TabId>,
}

impl App {
    pub(super) fn on_tabs(&mut self, msg: TabMsg) -> Task<Message> {
        match msg {
            TabMsg::Context(id, at) => {
                self.menu_open = None;
                self.context_menu = None;
                // Inside the strip the cursor comes in scrolled coordinates.
                let at = Point::new(at.x - self.tab_bar.offset, at.y);
                self.tab_bar.menu = Some(TabMenu::Context(id, at));
            }
            TabMsg::ToggleList => {
                self.menu_open = None;
                self.context_menu = None;
                self.tab_bar.menu = match self.tab_bar.menu {
                    Some(TabMenu::List) => None,
                    _ => Some(TabMenu::List),
                };
            }
            TabMsg::Dismiss => self.tab_bar.menu = None,
            TabMsg::Close(id, scope) => {
                self.tab_bar.menu = None;
                self.close_tabs(id, scope);
            }
            TabMsg::Pick(id) => {
                self.tab_bar.menu = None;
                if let Some(i) = self.tab_index(id) {
                    self.active = i;
                }
            }
            TabMsg::Scrolled(x) => self.tab_bar.offset = x,
        }
        Task::none()
    }

    /// The tabs `scope` closes, relative to the tab `id`.
    pub fn tabs_in_scope(&self, id: TabId, scope: CloseScope) -> Vec<TabId> {
        let Some(i) = self.tab_index(id) else { return Vec::new() };
        self.tabs
            .iter()
            .enumerate()
            .filter(|(j, _)| match scope {
                CloseScope::This => *j == i,
                CloseScope::Others => *j != i,
                CloseScope::All => true,
                CloseScope::Left => *j < i,
                CloseScope::Right => *j > i,
            })
            .map(|(_, t)| t.id)
            .collect()
    }

    /// Close the tabs in `scope`: saved ones at once, then one unsaved
    /// dialog per modified tab (Cancel stops there).
    fn close_tabs(&mut self, id: TabId, scope: CloseScope) {
        let ids = self.tabs_in_scope(id, scope);
        // The clicked tab stays in front when it is not closed.
        if !matches!(scope, CloseScope::This | CloseScope::All)
            && let Some(i) = self.tab_index(id)
        {
            self.active = i;
        }
        let (dirty, clean): (Vec<TabId>, Vec<TabId>) =
            ids.into_iter().partition(|id| self.tabs.iter().any(|t| t.id == *id && t.is_dirty()));
        for id in clean {
            if let Some(i) = self.tab_index(id) {
                self.close_tab_now(i);
            }
        }
        self.tab_bar.close_queue = dirty.into_iter().rev().collect();
        self.close_next_queued();
    }

    /// Hook (a tab closed, or its unsaved dialog was answered): ask about
    /// the next modified tab of a multi-close.
    pub(super) fn close_next_queued(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        while let Some(id) = self.tab_bar.close_queue.pop() {
            if self.tab_index(id).is_some() {
                let _ = self.request_close(id);
                return;
            }
        }
    }

    /// Hook: a close was cancelled, so the rest of a multi-close is too.
    pub(super) fn cancel_queued_closes(&mut self) {
        self.tab_bar.close_queue.clear();
    }

    /// Scroll the tab strip so the active tab is in view (after switching
    /// to a tab that was scrolled out, or opening one at the end).
    pub(super) fn reveal_active_tab(&self) -> Task<Message> {
        let Some(active) = self.active_tab() else { return Task::none() };
        let start: f32 = self.tabs.iter().take_while(|t| t.id != active.id).map(tab_width).sum();
        let end = start + tab_width(active);
        let visible = self.tab_strip_width();
        let offset = self.tab_bar.offset;
        // The estimate is not exact: leave some room on the far side.
        const MARGIN: f32 = 24.0;
        let x = if start < offset {
            (start - MARGIN).max(0.0)
        } else if end > offset + visible {
            end - visible + MARGIN
        } else {
            return Task::none();
        };
        iced::widget::operation::scroll_to(TAB_STRIP, iced::widget::scrollable::AbsoluteOffset { x: Some(x), y: None })
    }

    /// Room for the tabs: the window minus the sidebar, the comments panel
    /// and the overflow button.
    fn tab_strip_width(&self) -> f32 {
        let mut w = self.window_size.width - LIST_BUTTON;
        if self.sidebar.visible {
            w -= self.sidebar.width;
        }
        if self.comments_visible {
            w -= mdedit_theme::space::COMMENTS_WIDTH;
        }
        w.max(100.0)
    }

    /// Whether the tabs are wider than `available`.
    pub fn tabs_overflow(&self, available: f32) -> bool {
        // A little early rather than late: the widths are estimates.
        self.tabs.iter().map(tab_width).sum::<f32>() * 1.05 > available
    }
}

/// Estimated width of a tab in the strip: title, modified dot, close
/// button, padding and the separator line.
pub fn tab_width(tab: &Tab) -> f32 {
    use mdedit_theme::space::*;
    let chars = tab.title().chars().count() as f32;
    let dot = if tab.is_dirty() { 8.0 + SPACE_2 } else { 0.0 };
    // 6.3 px per character fits the 13 px UI font (measured on Xephyr).
    chars * 6.3 + dot + 18.0 + 2.0 * SPACE_2 + 2.0 * SPACE_3 + 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longer_titles_are_wider() {
        let a = Tab::new(1, Document::from_text(""), Mode::Edit, 1);
        let mut b = Tab::new(2, Document::from_text(""), Mode::Edit, 2);
        b.doc.path = Some(PathBuf::from("/x/a-much-longer-file-name.md"));
        assert!(tab_width(&b) > tab_width(&a));
    }
}
