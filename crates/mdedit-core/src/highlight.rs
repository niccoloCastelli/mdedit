//! Line-local markdown styling for Edit mode (parsing workstream).
//!
//! The Edit-mode widget asks for the styled ranges of each *visible* line.
//! Small documents can derive them from the block index; large files
//! (no block index) use the line-local tokenizer with the fence state
//! carried from the previous visible line (ARCHITECTURE 3.3). Token names map
//! one-to-one to the `syntax-*` design tokens (DESIGN_SYSTEM 2.4).

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxToken {
    Marker,
    Heading(u8),
    Emphasis,
    Strong,
    Strikethrough,
    Link,
    Url,
    Code,
    Html,
    Math,
}

/// A styled byte range **relative to the start of the line**. Ranges may
/// overlap (strong inside a link); the widget applies them in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledRange {
    pub range: Range<usize>,
    pub token: SyntaxToken,
}

/// State carried from one line to the next (open code fence, HTML block).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineState {
    pub in_fence: Option<String>,
}

/// Styled ranges for one line. Placeholder: returns nothing.
pub fn highlight_line(line: &str, state: &mut LineState) -> Vec<StyledRange> {
    let _ = (line, state);
    Vec::new()
}
