//! Code block highlighting for both outputs: syntect with the `[code]`
//! colours of a built-in mdedit theme (DESIGN_SYSTEM 2.4).

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    /// Six lowercase hex digits; `None` is the default code colour.
    pub color: Option<String>,
    pub bold: bool,
    pub italic: bool,
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme(name: &str) -> Option<Theme> {
    let set = mdedit_theme::file::ThemeSet::builtin();
    set.get(name).ok().map(|t| t.syntect_theme())
}

/// Highlight `code` as `lang` (a fence info word: name or extension).
/// Unknown languages and themes give one unstyled span per line. Lines
/// have no trailing newline.
pub fn highlight(code: &str, lang: &str, theme_name: &str) -> Vec<Vec<Span>> {
    let plain = || code.lines().map(|l| vec![plain_span(l)]).collect();
    let ss = syntaxes();
    let syntax = if lang.is_empty() {
        None
    } else {
        ss.find_syntax_by_token(lang).or_else(|| ss.find_syntax_by_name(lang))
    };
    let (Some(syntax), Some(theme)) = (syntax, theme(theme_name)) else { return plain() };
    let default_fg = theme.settings.foreground;
    let mut hl = HighlightLines::new(syntax, &theme);
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        let Ok(ranges) = hl.highlight_line(line, ss) else { return plain() };
        let mut spans: Vec<Span> = Vec::new();
        for (style, text) in ranges {
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                continue;
            }
            let fg = style.foreground;
            let color = (Some(fg) != default_fg).then(|| format!("{:02x}{:02x}{:02x}", fg.r, fg.g, fg.b));
            let bold = style.font_style.contains(FontStyle::BOLD);
            let italic = style.font_style.contains(FontStyle::ITALIC);
            match spans.last_mut() {
                Some(last) if last.color == color && last.bold == bold && last.italic == italic => {
                    last.text.push_str(text)
                }
                _ => spans.push(Span { text: text.to_string(), color, bold, italic }),
            }
        }
        lines.push(spans);
    }
    lines
}

fn plain_span(text: &str) -> Span {
    Span { text: text.to_string(), color: None, bold: false, italic: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keywords_get_the_theme_colour() {
        let lines = highlight("fn main() {}\n// done\n", "rust", "ember-light");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].text, "fn");
        assert!(lines[0][0].color.is_some());
        assert!(lines[1].iter().any(|s| s.italic));
    }

    #[test]
    fn unknown_language_is_plain() {
        let lines = highlight("a\n\nb", "nope-lang", "ember-light");
        assert_eq!(lines, vec![vec![plain_span("a")], vec![plain_span("")], vec![plain_span("b")]]);
    }
}
