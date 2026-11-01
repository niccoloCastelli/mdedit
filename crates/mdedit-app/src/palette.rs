//! Command palette (DESIGN_SYSTEM 7): Ctrl+Shift+P searches every command,
//! Ctrl+P the same palette scoped to files (open tabs, then recent files).

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Commands,
    Files,
    Projects,
}

#[derive(Debug, Clone)]
pub struct Palette {
    pub scope: Scope,
    pub query: String,
    pub selected: usize,
}

/// One row of results.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Command { id: &'static str, title: &'static str, shortcut: Option<String> },
    Tab { index: usize, title: String, detail: String },
    Recent { path: PathBuf, title: String, detail: String },
    Project { path: PathBuf, title: String, detail: String },
}

impl Item {
    pub fn title(&self) -> &str {
        match self {
            Item::Command { title, .. } => title,
            Item::Tab { title, .. } | Item::Recent { title, .. } | Item::Project { title, .. } => title,
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Item::Command { shortcut, .. } => shortcut.as_deref(),
            Item::Tab { detail, .. } | Item::Recent { detail, .. } | Item::Project { detail, .. } => Some(detail),
        }
    }
}

pub const MAX_ROWS: usize = 12;

impl Palette {
    pub fn new(scope: Scope) -> Self {
        Self { scope, query: String::new(), selected: 0 }
    }

    /// Filter and rank `items` by the query, best first.
    pub fn filter(&self, items: Vec<Item>) -> Vec<Item> {
        if self.query.trim().is_empty() {
            return items;
        }
        let mut scored: Vec<(i64, usize, Item)> = items
            .into_iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let hay = match &item {
                    Item::Command { id, title, .. } => format!("{title} {id}"),
                    Item::Tab { title, detail, .. }
                    | Item::Recent { title, detail, .. }
                    | Item::Project { title, detail, .. } => format!("{title} {detail}"),
                };
                fuzzy_score(&self.query, &hay).map(|s| (s, i, item))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, _, item)| item).collect()
    }

    pub fn move_selection(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
    }
}

/// Subsequence match, case-insensitive. Rewards consecutive matches and
/// matches at word starts; `None` when not every query char is found.
pub fn fuzzy_score(query: &str, hay: &str) -> Option<i64> {
    let hay: Vec<char> = hay.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut prev: Option<usize> = None;
    for q in query.chars().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()) {
        let found = (pos..hay.len()).find(|&i| hay[i] == q)?;
        score += 1;
        if prev.is_some_and(|p| p + 1 == found) {
            score += 5;
        }
        if found == 0 || !hay[found - 1].is_alphanumeric() {
            score += 8;
        }
        score -= (found - pos).min(10) as i64;
        prev = Some(found);
        pos = found + 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(id: &'static str, title: &'static str) -> Item {
        Item::Command { id, title, shortcut: None }
    }

    #[test]
    fn fuzzy_basics() {
        assert!(fuzzy_score("sv", "Save").is_some());
        assert!(fuzzy_score("xyz", "Save").is_none());
        assert!(fuzzy_score("save", "Save As").unwrap() > fuzzy_score("save", "Show All Views").unwrap_or(i64::MIN));
        assert!(fuzzy_score("tsb", "Toggle Sidebar").unwrap() > fuzzy_score("tsb", "Hotstub").unwrap());
    }

    #[test]
    fn filter_ranks_and_wraps() {
        let mut p = Palette::new(Scope::Commands);
        p.query = "split".into();
        let out = p.filter(vec![cmd("view.mode_edit", "Edit Mode"), cmd("view.mode_split", "Split Mode")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title(), "Split Mode");
        p.move_selection(-1, 3);
        assert_eq!(p.selected, 2);
        p.move_selection(1, 3);
        assert_eq!(p.selected, 0);
    }
}
