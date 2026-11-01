//! Fenced-code highlighting map (DESIGN_SYSTEM 2.4): syntect scope selectors
//! to colours, defined per theme in its `[code]` table.

use crate::Rgba;

/// One `[code]` rule, with its colour already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeStyle {
    /// A syntect/TextMate scope selector, e.g. `"entity.name.function"`.
    pub scope: String,
    pub color: Rgba,
    pub bold: bool,
    pub italic: bool,
}

#[cfg(feature = "syntect")]
impl crate::Theme {
    /// A syntect theme from [`crate::Theme::code`]: default foreground
    /// `syntax-code` on `syntax-code-bg`, selection `accent-soft`, caret
    /// `accent`. Rules whose selector syntect cannot parse are skipped.
    pub fn syntect_theme(&self) -> syntect::highlighting::Theme {
        use crate::ColorToken;
        use std::str::FromStr;
        use syntect::highlighting::{FontStyle, ScopeSelectors, StyleModifier, ThemeItem, ThemeSettings};

        let color = |c: Rgba| syntect::highlighting::Color { r: c.r, g: c.g, b: c.b, a: c.a };
        let scopes = self
            .code
            .iter()
            .filter_map(|style| {
                let scope = ScopeSelectors::from_str(&style.scope).ok()?;
                let mut font_style = FontStyle::empty();
                font_style.set(FontStyle::BOLD, style.bold);
                font_style.set(FontStyle::ITALIC, style.italic);
                Some(ThemeItem {
                    scope,
                    style: StyleModifier {
                        foreground: Some(color(style.color)),
                        background: None,
                        font_style: Some(font_style),
                    },
                })
            })
            .collect();
        syntect::highlighting::Theme {
            name: Some(self.name.clone()),
            author: None,
            settings: ThemeSettings {
                foreground: Some(color(self.color(ColorToken::SyntaxCode))),
                background: Some(color(self.color(ColorToken::SyntaxCodeBg))),
                caret: Some(color(self.color(ColorToken::Accent))),
                selection: Some(color(self.color(ColorToken::Selection))),
                ..Default::default()
            },
            scopes,
        }
    }
}

#[cfg(all(test, feature = "syntect"))]
mod tests {
    use crate::{ColorToken, Theme};
    use syntect::easy::HighlightLines;
    use syntect::highlighting::FontStyle;
    use syntect::parsing::SyntaxSet;

    #[test]
    fn syntect_theme_colours_rust_code() {
        let theme = Theme::ember_light();
        let st = theme.syntect_theme();
        assert_eq!(st.scopes.len(), theme.code.len());

        let ss = SyntaxSet::load_defaults_newlines();
        let syntax = ss.find_syntax_by_extension("rs").unwrap();
        let mut hl = HighlightLines::new(syntax, &st);
        let spans = hl.highlight_line("fn main() { // hi\n", &ss).unwrap();

        let keyword = theme.color(ColorToken::AccentText);
        let (style, _) = spans.iter().find(|(_, text)| *text == "fn").unwrap();
        assert_eq!((style.foreground.r, style.foreground.g, style.foreground.b), (keyword.r, keyword.g, keyword.b));
        let (style, _) = spans.iter().find(|(_, text)| text.contains("hi")).unwrap();
        assert!(style.font_style.contains(FontStyle::ITALIC));
        let bg = theme.color(ColorToken::SyntaxCodeBg);
        assert_eq!(style.background.r, bg.r);
    }
}
