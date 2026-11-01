//! Block index and markdown parsing (parsing workstream).
//!
//! ARCHITECTURE 3: the document is a list of top-level blocks with byte
//! ranges. Edits mark touched blocks dirty and only a window around them is
//! reparsed with pulldown-cmark. **This file is a placeholder that reparses
//! the whole document on every edit**; the API is the contract, the
//! implementation is expected to change completely (B-tree storage,
//! incremental window reparse, link-reference side table, worker offload).

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

use crate::buffer::TextBuffer;
use crate::edit::AppliedEdit;

/// The pulldown-cmark options mdedit uses everywhere (SPEC 4). Export and
/// render must use the same set so all views agree.
pub fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading {
        level: u8,
    },
    BlockQuote,
    List {
        ordered: bool,
    },
    CodeBlock {
        fenced: bool,
        info: String,
    },
    HtmlBlock,
    Table,
    ThematicBreak,
    FootnoteDefinition,
    /// Fenced `math` / `$$` block (v1.x); parsed as code until then.
    Math,
    /// Fenced `mermaid` block (v1.x); parsed as code until then.
    Mermaid,
}

/// One top-level block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Source bytes, from the first byte of the block to the end of its last
    /// line (excluding the blank lines that separate blocks).
    pub bytes: Range<usize>,
    /// Bumped whenever the block is reparsed; render caches key on it.
    pub generation: u64,
}

#[derive(Debug, Clone, Default)]
pub struct BlockIndex {
    blocks: Vec<Block>,
    generation: u64,
}

impl BlockIndex {
    pub fn parse(buf: &dyn TextBuffer) -> Self {
        let mut index = Self::default();
        index.reparse_all(buf);
        index
    }

    /// Keep the index in step with an applied edit.
    pub fn on_edit(&mut self, _applied: &AppliedEdit, buf: &dyn TextBuffer) {
        self.reparse_all(buf);
    }

    /// Block `i` in document order.
    pub fn get(&self, i: usize) -> Option<Block> {
        self.blocks.get(i).cloned()
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Index of the block containing `byte`, or the block just before it
    /// when `byte` is in the gap between blocks.
    pub fn block_at(&self, byte: usize) -> Option<usize> {
        let i = self.blocks.partition_point(|b| b.bytes.start <= byte);
        i.checked_sub(1)
    }

    /// Blocks overlapping `range`, for rendering a viewport.
    pub fn blocks_in(&self, range: Range<usize>) -> &[Block] {
        let start = self.blocks.partition_point(|b| b.bytes.end <= range.start);
        let end = self.blocks.partition_point(|b| b.bytes.start < range.end);
        &self.blocks[start..end.max(start)]
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn reparse_all(&mut self, buf: &dyn TextBuffer) {
        self.generation += 1;
        let text = buf.text();
        self.blocks = top_level_blocks(&text, 0, self.generation);
    }
}

/// Top-level blocks of `src`, with ranges shifted by `base`.
fn top_level_blocks(src: &str, base: usize, generation: u64) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut depth = 0usize;
    for (event, range) in Parser::new_ext(src, markdown_options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0
                    && let Some(kind) = block_kind(&tag)
                {
                    let end = trim_trailing_newlines(src, range.clone());
                    blocks.push(Block { kind, bytes: base + range.start..base + end, generation });
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            Event::Rule if depth == 0 => {
                let end = trim_trailing_newlines(src, range.clone());
                blocks.push(Block {
                    kind: BlockKind::ThematicBreak,
                    bytes: base + range.start..base + end,
                    generation,
                });
            }
            _ => {}
        }
    }
    blocks
}

fn trim_trailing_newlines(src: &str, range: Range<usize>) -> usize {
    range.start + src[range].trim_end_matches(['\n', '\r']).len()
}

fn block_kind(tag: &Tag<'_>) -> Option<BlockKind> {
    use pulldown_cmark::CodeBlockKind;
    Some(match tag {
        Tag::Paragraph => BlockKind::Paragraph,
        Tag::Heading { level, .. } => BlockKind::Heading { level: *level as u8 },
        Tag::BlockQuote(_) => BlockKind::BlockQuote,
        Tag::List(start) => BlockKind::List { ordered: start.is_some() },
        Tag::CodeBlock(CodeBlockKind::Indented) => BlockKind::CodeBlock { fenced: false, info: String::new() },
        Tag::CodeBlock(CodeBlockKind::Fenced(info)) => match info.split_whitespace().next() {
            Some("mermaid") => BlockKind::Mermaid,
            Some("math") => BlockKind::Math,
            _ => BlockKind::CodeBlock { fenced: true, info: info.to_string() },
        },
        Tag::HtmlBlock => BlockKind::HtmlBlock,
        Tag::Table(_) => BlockKind::Table,
        Tag::FootnoteDefinition(_) => BlockKind::FootnoteDefinition,
        _ => return None,
    })
}

/// One entry of the document outline (SPEC 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    pub level: u8,
    pub title: String,
    pub byte: usize,
}

/// Headings in document order.
pub fn outline(index: &BlockIndex, buf: &dyn TextBuffer) -> Vec<OutlineEntry> {
    index
        .blocks()
        .iter()
        .filter_map(|b| match b.kind {
            BlockKind::Heading { level } => {
                let src = buf.slice(b.bytes.clone());
                let first = src.lines().next().unwrap_or("");
                let title = first.trim_start_matches('#').trim().to_string();
                Some(OutlineEntry { level, title, byte: b.bytes.start })
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RopeBuffer;

    #[test]
    fn finds_top_level_blocks() {
        let buf =
            RopeBuffer::from_text("# Title\n\nPara one\nstill one\n\n- a\n- b\n\n---\n\n```mermaid\ngraph\n```\n");
        let index = BlockIndex::parse(&buf);
        let kinds: Vec<_> = index.blocks().iter().map(|b| b.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Heading { level: 1 },
                BlockKind::Paragraph,
                BlockKind::List { ordered: false },
                BlockKind::ThematicBreak,
                BlockKind::Mermaid,
            ]
        );
        assert_eq!(buf.slice(index.blocks()[1].bytes.clone()), "Para one\nstill one");
        assert_eq!(index.block_at(12), Some(1));
        assert_eq!(outline(&index, &buf)[0].title, "Title");
    }
}
