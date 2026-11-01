//! Document-global definitions a block needs from elsewhere: link reference
//! definitions and footnote definitions.

use std::collections::HashMap;

use mdedit_core::parse::{BlockIndex, markdown_options};
use mdedit_core::{BlockKind, TextBuffer};
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
    /// Display number of a footnote: definitions are numbered 1, 2, ... in
    /// document order (the view shows them where they are written, so the
    /// numbers read in order there).
    fn footnote_number(&self, _label: &str) -> Option<usize> {
        None
    }
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
    footnotes: HashMap<String, usize>,
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

    /// Add a footnote definition; it gets the next number unless the
    /// label is already defined.
    pub fn insert_footnote(&mut self, label: &str) {
        let next = self.footnotes.len() + 1;
        self.footnotes.entry(normalize_label(label)).or_insert(next);
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
        self.footnotes.contains_key(&normalize_label(label))
    }

    fn footnote_number(&self, label: &str) -> Option<usize> {
        self.footnotes.get(&normalize_label(label)).copied()
    }
}

/// Definitions from a live [`BlockIndex`]: link references from its side
/// table, footnote labels from its top-level footnote definition blocks.
/// Rebuild it when the index changes (cheap: one pass over the blocks).
pub struct IndexRefs<'a> {
    index: &'a BlockIndex,
    footnotes: HashMap<String, usize>,
}

impl<'a> IndexRefs<'a> {
    pub fn new(index: &'a BlockIndex, buf: &dyn TextBuffer) -> Self {
        let mut footnotes = HashMap::new();
        for label in footnote_labels(index, buf) {
            let next = footnotes.len() + 1;
            footnotes.entry(label).or_insert(next);
        }
        Self { index, footnotes }
    }
}

/// Normalised labels of the top-level footnote definitions, in order.
pub fn footnote_labels<'b>(index: &'b BlockIndex, buf: &'b dyn TextBuffer) -> impl Iterator<Item = String> + 'b {
    index.blocks_in(0..buf.len_bytes()).filter(|b| b.kind == BlockKind::FootnoteDefinition).filter_map(|b| {
        let head = buf.line(buf.byte_to_line(b.bytes.start));
        let label = head.trim_start().strip_prefix("[^")?;
        Some(normalize_label(&label[..label.find("]:")?]))
    })
}

impl LinkResolver for IndexRefs<'_> {
    fn link_ref(&self, label: &str) -> Option<LinkDef> {
        let r = self.index.link_ref(label)?;
        Some(LinkDef { dest: r.dest.clone(), title: r.title.clone().unwrap_or_default() })
    }

    fn has_footnote(&self, label: &str) -> bool {
        self.footnotes.contains_key(&normalize_label(label))
    }

    fn footnote_number(&self, label: &str) -> Option<usize> {
        self.footnotes.get(&normalize_label(label)).copied()
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
        assert_eq!(t.footnote_number("n"), Some(1));
    }
}
