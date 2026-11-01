//! Document-global definitions a block needs from elsewhere: link reference
//! definitions and footnote definitions.

use std::collections::{HashMap, HashSet};

use mdedit_core::parse::markdown_options;
use pulldown_cmark::{Event, Parser, Tag};

/// Target of a link reference definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkDef {
    pub dest: String,
    pub title: String,
}

/// Looks up definitions by label as written (the resolver normalises).
pub trait LinkResolver {
    fn link_ref(&self, label: &str) -> Option<LinkDef>;
    fn has_footnote(&self, label: &str) -> bool;
}

/// CommonMark label matching: case-folded, inner whitespace collapsed,
/// trimmed.
pub fn normalize_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for word in label.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        for c in word.chars() {
            match c {
                // Full case folding maps these to "ss"; `to_lowercase` does not.
                'ß' | 'ẞ' => out.push_str("ss"),
                c => out.extend(c.to_lowercase()),
            }
        }
    }
    out
}

/// Definitions keyed by normalised label; the first definition of a label
/// wins, as in CommonMark.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefTable {
    links: HashMap<String, LinkDef>,
    footnotes: HashSet<String>,
}

impl RefTable {
    /// Collect every definition in a markdown text (one full parse).
    pub fn scan(text: &str) -> Self {
        let mut table = Self::default();
        let parser = Parser::new_ext(text, markdown_options());
        for (label, def) in parser.reference_definitions().iter() {
            let title = def.title.as_deref().unwrap_or("").to_string();
            table.insert_link(label, LinkDef { dest: def.dest.to_string(), title });
        }
        for event in parser {
            if let Event::Start(Tag::FootnoteDefinition(label)) = event {
                table.insert_footnote(&label);
            }
        }
        table
    }

    pub fn insert_link(&mut self, label: &str, def: LinkDef) {
        self.links.entry(normalize_label(label)).or_insert(def);
    }

    pub fn insert_footnote(&mut self, label: &str) {
        self.footnotes.insert(normalize_label(label));
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty() && self.footnotes.is_empty()
    }
}

impl LinkResolver for RefTable {
    fn link_ref(&self, label: &str) -> Option<LinkDef> {
        self.links.get(&normalize_label(label)).cloned()
    }

    fn has_footnote(&self, label: &str) -> bool {
        self.footnotes.contains(&normalize_label(label))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_fold_case_and_space() {
        assert_eq!(normalize_label("  Foo \n  BAR "), "foo bar");
        assert_eq!(normalize_label("ẞ"), normalize_label("SS"));
    }

    #[test]
    fn scan_finds_links_and_footnotes() {
        let t = RefTable::scan("[Foo]: /url \"T\"\n[foo]: /second\n\n[^n]: note\n");
        assert_eq!(t.link_ref("FOO"), Some(LinkDef { dest: "/url".into(), title: "T".into() }));
        assert!(t.has_footnote("N"));
        assert!(!t.has_footnote("m"));
    }
}
