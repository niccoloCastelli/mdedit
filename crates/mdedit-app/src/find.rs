//! Find and replace bar (SPEC 5, DESIGN_SYSTEM 8.5), JetBrains style: a
//! strip above the document with the query, icon toggles for match case
//! (Aa), whole words (W) and regex (.*), the match count, previous / next
//! and close; Ctrl+H adds the replace row (Replace, Replace All).
//!
//! Ctrl+F fills the query with the selection (one line) and selects it.
//! Typing searches incrementally from where the caret was when the bar
//! opened; Enter / Shift+Enter (or F3 / Shift+F3) step through matches,
//! selecting each in the document. Matches are highlighted in every mode
//! (`search-match`, current one `accent-soft` with an `accent` border).
//! Replace All is one undo step; `$1` / `${name}` expand in regex mode.
//!
//! Large files are searched on a worker. The sidebar's Search panel uses the
//! same three options.
//!
//! The app hooks in through the `find` field, `Message::Find`, `on_find`,
//! `find_key`, `find_refresh` (after every update) and `find_bar`.

use std::ops::{ControlFlow, Range};
use std::sync::Arc;

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{Space, button, column, container, row, svg, text, text_input};
use iced::{Alignment, Element, Length, Padding, Task};
use mdedit_core::search::{SearchQuery, Searcher};
use mdedit_core::{Edit, EditOrigin, Selection, TextBuffer, TxnId};
use mdedit_editor::{Highlight, HighlightKind};
use mdedit_theme::space::*;
use mdedit_theme::{ColorToken as T, Theme};

use crate::app::{App, Message};
use crate::style::{self, Btn, c};
use crate::tab::TabId;

pub const FIND_INPUT: &str = "find-input";
pub const REPLACE_INPUT: &str = "replace-input";

const UI: f32 = 13.0;
const UI_SMALL: f32 = 12.0;
/// Matches kept (and counted); more are reported as "10000+".
const MAX_MATCHES: usize = 10_000;
/// Longest selection Ctrl+F copies into the query.
const MAX_PREFILL: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Find,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Case,
    Word,
    Regex,
}

#[derive(Debug, Clone)]
pub enum FindMsg {
    /// Ctrl+F (`false`) or Ctrl+H (`true`).
    Open {
        replace: bool,
    },
    Query(String),
    Replacement(String),
    Toggle(Toggle),
    Next,
    Prev,
    Replace,
    ReplaceAll,
    ShowReplace(bool),
    Close,
    /// A large-file search finished.
    Found(Key4, Result<Vec<Range<usize>>, String>),
}

/// What a result list belongs to: tab, buffer version and query.
pub type Key4 = (TabId, u64, SearchQuery);

#[derive(Debug, Default)]
pub struct FindUi {
    pub open: bool,
    pub replace: bool,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// The bar field that has the keyboard, as far as we know (set when we
    /// focus it or it is typed in, cleared when the document is used).
    pub focus: Option<Field>,
    /// Alt is held (without Ctrl): the text inputs still insert the letter
    /// of an Alt+C / W / X toggle, which we drop.
    pub alt: bool,
    /// The field text before its last edit. The input's edit for an Alt
    /// toggle letter can arrive before the key event (and before `alt` is
    /// known), so the toggle undoes it from here.
    before_edit: Option<(Field, String)>,
    matches: Vec<Range<usize>>,
    truncated: bool,
    current: Option<usize>,
    error: Option<String>,
    /// Incremental search starts here (the caret when the bar opened, or
    /// the last match stepped to).
    origin: usize,
    /// Select the first match from `origin` once results come in.
    navigate: bool,
    for_key: Option<Key4>,
    pending: Option<Key4>,
    /// Tab whose widgets show our highlights.
    highlighted: Option<TabId>,
    notice: Option<String>,
}

impl FindUi {
    pub fn search_query(&self) -> SearchQuery {
        SearchQuery {
            pattern: self.query.clone(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
        }
    }

    fn summary(&self) -> String {
        if let Some(e) = &self.error {
            return e.clone();
        }
        if let Some(n) = &self.notice {
            return n.clone();
        }
        if self.query.is_empty() {
            return String::new();
        }
        if self.pending.is_some() {
            return "Searching…".into();
        }
        let total = if self.truncated { format!("{MAX_MATCHES}+") } else { self.matches.len().to_string() };
        match (self.matches.len(), self.current) {
            (0, _) => "No results".into(),
            (_, Some(i)) => format!("{} of {total}", i + 1),
            (_, None) => format!("{total} results"),
        }
    }
}

/// The text `replacement` becomes for the match at `range` (`$1`, `${name}`
/// expanded in regex mode).
pub fn expansion(buf: &dyn TextBuffer, query: &SearchQuery, range: &Range<usize>, replacement: &str) -> String {
    if !(query.regex && replacement.contains('$')) {
        return replacement.to_string();
    }
    let Ok(searcher) = Searcher::new(query) else { return replacement.to_string() };
    let mut out = None;
    let _ = searcher.run(
        buf,
        range.clone(),
        true,
        |f| {
            if f.range == *range {
                out = Some(f.expand(replacement));
            }
            ControlFlow::Break(())
        },
        |_| ControlFlow::Continue(()),
    );
    out.unwrap_or_else(|| replacement.to_string())
}

/// Every replacement for Replace All, last first (so they apply in order).
pub fn replace_all_edits(buf: &dyn TextBuffer, query: &SearchQuery, replacement: &str) -> Result<Vec<Edit>, String> {
    let searcher = Searcher::new(query).map_err(|e| e.to_string())?;
    let captures = query.regex && replacement.contains('$');
    let mut edits = Vec::new();
    let _ = searcher.run(
        buf,
        0..buf.len_bytes(),
        captures,
        |f| {
            edits.push(Edit {
                range: f.range.clone(),
                text: f.expand(replacement),
                origin: EditOrigin::Command,
                txn: TxnId(0),
            });
            ControlFlow::Continue(())
        },
        |_| ControlFlow::Continue(()),
    );
    edits.reverse();
    Ok(edits)
}

/// Whether `new` is `old` with one Alt toggle letter typed in: `letter`,
/// or any of c, w and x.
fn is_toggle_letter(old: &str, new: &str, letter: Option<char>) -> bool {
    new.len() == old.len() + 1
        && new.char_indices().any(|(i, c)| {
            letter.map_or(matches!(c, 'c' | 'w' | 'x'), |l| c == l) && new[..i] == old[..i] && new[i + 1..] == old[i..]
        })
}

fn search(buf: &dyn TextBuffer, query: &SearchQuery) -> Result<Vec<Range<usize>>, String> {
    let mut out = Vec::new();
    mdedit_core::search::search(buf, query, |m| {
        out.push(m.range);
        if out.len() > MAX_MATCHES { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    })
    .map_err(|e| e.to_string())?;
    Ok(out)
}

impl App {
    pub(crate) fn on_find(&mut self, msg: FindMsg) -> Task<Message> {
        let f = &mut self.find;
        f.notice = None;
        match msg {
            FindMsg::Open { replace } => {
                let was_open = f.open;
                f.open = true;
                f.replace = replace || (was_open && f.replace);
                f.focus = Some(Field::Find);
                // The floating format bar would sit over the find bar.
                self.hide_toolbar_for_typing();
                if let Some(tab) = self.tabs.get(self.active) {
                    let sel = tab.selection();
                    let range = sel.range();
                    let f = &mut self.find;
                    f.origin = range.start;
                    if !sel.is_empty() && range.len() <= MAX_PREFILL {
                        let picked = tab.doc.buffer().slice(range).into_owned();
                        if !picked.contains(['\n', '\r']) {
                            let picked = if f.regex { regex_escape(&picked) } else { picked };
                            if picked != f.query {
                                f.query = picked;
                                f.navigate = true;
                            }
                        }
                    }
                }
                if !self.tabs.get(self.active).is_some_and(|t| t.editable()) {
                    self.find.replace = false;
                }
                let input = if replace && was_open && self.find.replace { REPLACE_INPUT } else { FIND_INPUT };
                return Task::batch([
                    iced::widget::operation::focus(input),
                    iced::widget::operation::select_all(input),
                ]);
            }
            FindMsg::Query(q) => {
                if f.alt && is_toggle_letter(&f.query, &q, None) {
                    return Task::none();
                }
                f.before_edit = Some((Field::Find, std::mem::replace(&mut f.query, q)));
                f.focus = Some(Field::Find);
                f.navigate = true;
            }
            FindMsg::Replacement(r) => {
                if f.alt && is_toggle_letter(&f.replacement, &r, None) {
                    return Task::none();
                }
                f.before_edit = Some((Field::Replace, std::mem::replace(&mut f.replacement, r)));
                f.focus = Some(Field::Replace);
            }
            FindMsg::Toggle(t) => {
                match t {
                    Toggle::Case => f.case_sensitive = !f.case_sensitive,
                    Toggle::Word => f.whole_word = !f.whole_word,
                    Toggle::Regex => f.regex = !f.regex,
                }
                f.navigate = true;
                // The sidebar results follow the same options.
                self.search_options_changed();
            }
            FindMsg::ShowReplace(on) => {
                f.replace = on;
                if on {
                    f.focus = Some(Field::Replace);
                    return iced::widget::operation::focus(REPLACE_INPUT);
                }
            }
            FindMsg::Next => self.step(1),
            FindMsg::Prev => self.step(-1),
            FindMsg::Replace => self.replace_current(),
            FindMsg::ReplaceAll => self.replace_all(),
            FindMsg::Close => {
                f.open = false;
                f.focus = None;
                self.clear_find_highlights();
                return self.focus_document();
            }
            FindMsg::Found(key, result) => {
                if self.find.pending.as_ref() == Some(&key) {
                    self.find.pending = None;
                    self.take_results(key, result);
                }
            }
        }
        Task::none()
    }

    /// Keys for the bar: Enter / Shift+Enter in the query step through
    /// matches, Enter in the replacement replaces, Esc closes, Alt+C / W / X
    /// toggle the options (as in JetBrains editors).
    pub(crate) fn find_key(&mut self, key: &Key, modifiers: Modifiers) -> Option<Task<Message>> {
        if !self.find.open {
            return None;
        }
        match key {
            Key::Named(Named::Enter) => match self.find.focus? {
                Field::Find => Some(self.on_find(if modifiers.shift() { FindMsg::Prev } else { FindMsg::Next })),
                Field::Replace => Some(self.on_find(FindMsg::Replace)),
            },
            Key::Named(Named::Escape) if self.find.focus.is_some() => Some(self.on_find(FindMsg::Close)),
            Key::Character(ch)
                if modifiers.alt() && !modifiers.command() && !modifiers.shift() && self.find.focus.is_some() =>
            {
                let (toggle, letter) = match ch.as_str() {
                    "c" => (Toggle::Case, 'c'),
                    "w" => (Toggle::Word, 'w'),
                    "x" => (Toggle::Regex, 'x'),
                    _ => return None,
                };
                let f = &mut self.find;
                if let Some((field, prev)) = f.before_edit.take() {
                    let text = match field {
                        Field::Find => &mut f.query,
                        Field::Replace => &mut f.replacement,
                    };
                    if is_toggle_letter(&prev, text, Some(letter)) {
                        *text = prev;
                    }
                }
                Some(self.on_find(FindMsg::Toggle(toggle)))
            }
            _ => None,
        }
    }

    /// Keep the results and highlights in step with the active document and
    /// the query. Called after every update.
    pub(crate) fn find_refresh(&mut self) -> Task<Message> {
        let active = self.tabs.get(self.active).map(|t| t.id);
        if self.find.highlighted.is_some() && (self.find.highlighted != active || !self.find.open) {
            self.clear_find_highlights();
        }
        if !self.find.open {
            return Task::none();
        }
        let Some(tab) = self.tabs.get(self.active) else { return Task::none() };
        let query = self.find.search_query();
        let key: Key4 = (tab.id, tab.doc.buffer().version(), query.clone());
        if self.find.for_key.as_ref() == Some(&key) || self.find.pending.as_ref() == Some(&key) {
            return Task::none();
        }
        if query.pattern.is_empty() {
            self.take_results(key, Ok(Vec::new()));
            return Task::none();
        }
        if tab.doc.is_large() {
            self.find.pending = Some(key.clone());
            let snapshot: Arc<dyn TextBuffer> = tab.doc.buffer().snapshot();
            return Task::perform(
                async move {
                    let k = key.clone();
                    let result = tokio::task::spawn_blocking(move || search(&*snapshot, &k.2))
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()));
                    (key, result)
                },
                |(key, result)| Message::Find(FindMsg::Found(key, result)),
            );
        }
        let result = search(tab.doc.buffer(), &query);
        self.take_results(key, result);
        Task::none()
    }

    fn take_results(&mut self, key: Key4, result: Result<Vec<Range<usize>>, String>) {
        let f = &mut self.find;
        let previous = f.current.and_then(|i| f.matches.get(i).cloned());
        let version_only = f.for_key.as_ref().is_some_and(|k| k.0 == key.0 && k.2 == key.2);
        f.for_key = Some(key);
        match result {
            Ok(mut m) => {
                f.truncated = m.len() > MAX_MATCHES;
                m.truncate(MAX_MATCHES);
                f.matches = m;
                f.error = None;
            }
            Err(e) => {
                f.matches.clear();
                f.truncated = false;
                f.error = Some(e);
            }
        }
        let caret = self.tabs.get(self.active).map_or(0, |t| t.selection().range().start);
        let f = &mut self.find;
        if f.navigate {
            f.navigate = false;
            let i = f.matches.iter().position(|r| r.start >= f.origin).or((!f.matches.is_empty()).then_some(0));
            f.current = i;
            if let Some(i) = i {
                self.select_match(i);
            }
        } else {
            // The document changed: keep the same match if it still exists,
            // else the first one from the caret, without moving anything.
            f.current = previous
                .filter(|_| version_only)
                .and_then(|p| f.matches.iter().position(|r| *r == p))
                .or_else(|| f.matches.iter().position(|r| r.start >= caret));
        }
        self.show_find_highlights();
    }

    fn step(&mut self, delta: isize) {
        let f = &mut self.find;
        if !f.open {
            f.open = true;
        }
        let n = f.matches.len();
        if n == 0 {
            return;
        }
        let caret = self.tabs.get(self.active).map_or(0, |t| t.selection().range().start);
        let f = &mut self.find;
        let i = match f.current {
            Some(i) => (i as isize + delta).rem_euclid(n as isize) as usize,
            None if delta > 0 => f.matches.iter().position(|r| r.start >= caret).unwrap_or(0),
            None => f.matches.iter().rposition(|r| r.start < caret).unwrap_or(n - 1),
        };
        f.current = Some(i);
        self.select_match(i);
        self.show_find_highlights();
    }

    fn select_match(&mut self, i: usize) {
        let Some(r) = self.find.matches.get(i).cloned() else { return };
        self.find.origin = r.start;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.set_selection(Selection { anchor: r.start, head: r.end });
            tab.editor.scroll_to_byte = Some(r.start);
        }
    }

    fn replace_current(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        if !tab.editable() {
            return;
        }
        let Some(r) = self.find.current.and_then(|i| self.find.matches.get(i).cloned()) else {
            self.step(1);
            return;
        };
        // The first press only shows which match will change.
        if tab.selection().range() != r {
            if let Some(i) = self.find.current {
                self.select_match(i);
            }
            return;
        }
        let query = self.find.search_query();
        let text = expansion(tab.doc.buffer(), &query, &r, &self.find.replacement);
        let end = r.start + text.len();
        let result = mdedit_core::command::CommandResult {
            edits: vec![Edit { range: r, text, origin: EditOrigin::Command, txn: TxnId(0) }],
            selection: Selection::cursor(end),
        };
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.apply_command(result);
        }
        // Move on to the next match after the replacement.
        self.find.origin = end;
        self.find.navigate = true;
    }

    fn replace_all(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if !tab.editable() || self.find.query.is_empty() {
            return;
        }
        let query = self.find.search_query();
        match replace_all_edits(tab.doc.buffer(), &query, &self.find.replacement) {
            Ok(edits) if edits.is_empty() => self.find.notice = Some("No results".into()),
            Ok(edits) => {
                let n = edits.len();
                let txn = tab.doc.next_txn();
                for edit in edits {
                    tab.apply(&Edit { txn, ..edit });
                }
                self.find.notice =
                    Some(if n == 1 { "Replaced 1 match".into() } else { format!("Replaced {n} matches") });
            }
            Err(e) => self.find.error = Some(e),
        }
    }

    fn show_find_highlights(&mut self) {
        let f = &self.find;
        let hl: Vec<Highlight> = f
            .matches
            .iter()
            .enumerate()
            .map(|(i, r)| Highlight {
                range: r.clone(),
                kind: if Some(i) == f.current { HighlightKind::CurrentSearchMatch } else { HighlightKind::SearchMatch },
            })
            .collect();
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if hl.is_empty() && !tab.has_search_highlights() {
            return;
        }
        tab.set_highlights(None, Some(hl));
        self.find.highlighted = Some(tab.id);
    }

    fn clear_find_highlights(&mut self) {
        if let Some(id) = self.find.highlighted.take()
            && let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id)
        {
            tab.set_highlights(None, Some(Vec::new()));
        }
        self.find.for_key = None;
    }

    /// The find bar, shown above the document while open.
    pub(crate) fn find_bar(&self) -> Option<Element<'_, Message>> {
        let f = &self.find;
        if !f.open {
            return None;
        }
        let t = &self.theme;
        let editable = self.active_tab().is_some_and(|t| t.editable());
        let fm = Message::Find;
        let input = |id: &'static str, placeholder: &'static str, value: &str, on: fn(String) -> FindMsg| {
            text_input(placeholder, value)
                .id(id)
                .on_input(move |s| fm(on(s)))
                .size(UI)
                .padding(Padding::from([SPACE_1, SPACE_2]))
                .width(Length::Fixed(300.0))
                .style(style::input(t))
        };
        let toggle = |name: &'static str, on: bool, tip: &'static str, which: Toggle| {
            crate::view::with_tooltip(
                t,
                icon_button(t, name, if on { Btn::Toggled } else { Btn::Ghost }, Some(fm(FindMsg::Toggle(which)))),
                tip,
            )
        };
        let has = !f.matches.is_empty();
        let no_results = !has && !f.query.is_empty() && f.pending.is_none() && f.notice.is_none();
        let summary_color = if f.error.is_some() || no_results { T::Danger } else { T::TextMuted };
        let find_row = row![
            crate::view::with_tooltip(
                t,
                icon_button(
                    t,
                    if f.replace { "chevron-down" } else { "chevron-right" },
                    Btn::Ghost,
                    editable.then(|| fm(FindMsg::ShowReplace(!f.replace))),
                ),
                "Replace (Ctrl+H)",
            ),
            input(FIND_INPUT, "Find", &f.query, FindMsg::Query),
            toggle("case-sensitive", f.case_sensitive, "Match case (Alt+C)", Toggle::Case),
            toggle("whole-word", f.whole_word, "Words (Alt+W)", Toggle::Word),
            toggle("regex", f.regex, "Regex (Alt+X)", Toggle::Regex),
            container(text(f.summary()).size(UI_SMALL).color(c(t, summary_color)))
                .padding(Padding::from([0.0, SPACE_2])),
            crate::view::with_tooltip(
                t,
                icon_button(t, "chevron-up", Btn::Ghost, has.then(|| fm(FindMsg::Prev))),
                "Previous (Shift+Enter)",
            ),
            crate::view::with_tooltip(
                t,
                icon_button(t, "chevron-down", Btn::Ghost, has.then(|| fm(FindMsg::Next))),
                "Next (Enter)",
            ),
            Space::new().width(Length::Fill),
            crate::view::with_tooltip(t, icon_button(t, "x", Btn::Ghost, Some(fm(FindMsg::Close))), "Close (Esc)"),
        ]
        .spacing(SPACE_1)
        .align_y(Alignment::Center);
        let mut col = column![find_row].spacing(SPACE_1);
        if f.replace && editable {
            let can = has && !f.query.is_empty();
            col = col.push(
                row![
                    Space::new().width(Length::Fixed(24.0)),
                    input(REPLACE_INPUT, "Replace", &f.replacement, FindMsg::Replacement),
                    crate::view::small_button(t, "Replace", Btn::Secondary, can.then(|| fm(FindMsg::Replace))),
                    crate::view::small_button(t, "Replace All", Btn::Secondary, can.then(|| fm(FindMsg::ReplaceAll))),
                ]
                .spacing(SPACE_1)
                .align_y(Alignment::Center),
            );
        }
        Some(
            container(col)
                .padding(Padding::from([SPACE_1, SPACE_2]))
                .width(Length::Fill)
                .style(style::chrome(t))
                .into(),
        )
    }
}

/// 24×24 icon button.
pub(crate) fn icon_button<'a>(t: &'a Theme, name: &str, kind: Btn, msg: Option<Message>) -> Element<'a, Message> {
    let color = match kind {
        Btn::Toggled => T::Accent,
        _ if msg.is_none() => T::TextMuted,
        _ => T::TextSecondary,
    };
    let icon: Element<'a, Message> = match mdedit_theme::icon(name) {
        Some(bytes) => svg(svg::Handle::from_memory(bytes))
            .width(16)
            .height(16)
            .style(move |_, _| svg::Style { color: Some(c(t, color)) })
            .into(),
        None => text(name.to_string()).size(UI_SMALL).into(),
    };
    button(container(icon).center(Length::Fill))
        .width(24)
        .height(24)
        .padding(0)
        .style(style::btn(t, kind))
        .on_press_maybe(msg)
        .into()
}

/// Escape regex metacharacters (a selection searched in regex mode).
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdedit_core::Document;

    fn doc(text: &str) -> Document {
        Document::from_text(text)
    }

    fn q(pattern: &str, regex: bool) -> SearchQuery {
        SearchQuery { pattern: pattern.into(), regex, ..Default::default() }
    }

    #[test]
    fn replace_all_expands_groups_and_goes_last_first() {
        let d = doc("a1 b2 c3");
        let edits = replace_all_edits(d.buffer(), &q(r"([a-z])(\d)", true), "$2$1").unwrap();
        let got: Vec<_> = edits.iter().map(|e| (e.range.clone(), e.text.as_str())).collect();
        assert_eq!(got, [(6..8, "3c"), (3..5, "2b"), (0..2, "1a")]);
        // Plain mode keeps `$` literally.
        let edits = replace_all_edits(d.buffer(), &q("b2", false), "$1").unwrap();
        assert_eq!(edits[0].text, "$1");
    }

    #[test]
    fn alt_toggle_letters_are_recognised() {
        assert!(is_toggle_letter("fox", "foxc", None));
        assert!(is_toggle_letter("fox", "fwox", Some('w')));
        assert!(!is_toggle_letter("fox", "fwox", Some('c')));
        assert!(!is_toggle_letter("fox", "foxa", None));
        assert!(!is_toggle_letter("fox", "foxcc", None));
        assert!(!is_toggle_letter("fox", "fo", None));
    }

    #[test]
    fn expansion_of_one_match() {
        let d = doc("x = foo(1); y = foo(22);");
        let query = q(r"foo\((\d+)\)", true);
        assert_eq!(expansion(d.buffer(), &query, &(16..23), "bar[$1]"), "bar[22]");
        assert_eq!(expansion(d.buffer(), &q("foo", false), &(4..7), "$1"), "$1");
    }

    #[test]
    fn invalid_regex_is_an_error_and_escaping_round_trips() {
        let d = doc("a+b (c)");
        assert!(search(d.buffer(), &q("(", true)).is_err());
        let found = search(d.buffer(), &q(&regex_escape("a+b (c)"), true)).unwrap();
        assert_eq!(found, vec![Range { start: 0, end: 7 }]);
    }

    #[test]
    fn search_caps_the_matches() {
        let d = doc(&"a".repeat(MAX_MATCHES + 50));
        assert_eq!(search(d.buffer(), &q("a", false)).unwrap().len(), MAX_MATCHES + 1);
    }
}
