//! Mermaid blocks in exports (SPEC 10): rendered with [`mdedit_mermaid`] in
//! the colours of the template's `code_theme`, once per distinct source.
//! A block that doesn't render stays a code block.

use std::collections::HashMap;

use mdedit_mermaid::{Diagram, Style};

/// CSS px to points: diagrams are laid out at 96 px per inch, so their
/// 14 px labels come out at 10.5 pt.
pub(crate) const PT_PER_PX: f32 = 0.75;

pub(crate) struct Diagrams {
    style: Style,
    rendered: HashMap<String, Option<Diagram>>,
}

impl Diagrams {
    pub(crate) fn new(code_theme: &str) -> Self {
        let theme =
            mdedit_theme::ThemeSet::builtin().get(code_theme).unwrap_or_else(|_| mdedit_theme::Theme::ember_light());
        Diagrams { style: Style::from_theme(&theme), rendered: HashMap::new() }
    }

    pub(crate) fn get(&mut self, src: &str) -> Option<&Diagram> {
        let style = &self.style;
        self.rendered.entry(src.to_string()).or_insert_with(|| mdedit_mermaid::render(src, style).ok()).as_ref()
    }
}
