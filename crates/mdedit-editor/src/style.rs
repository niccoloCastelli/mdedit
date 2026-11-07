//! Turning a source line and its syntax tokens into styled spans
//! (DESIGN_SYSTEM 2.4 colours, 3.2 sizes).

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use iced::advanced::text::Span;
use iced::font::{Style as FontStyle, Weight};
use iced::{Color, Font, Pixels};
use mdedit_core::highlight::{StyledRange, SyntaxToken};
use mdedit_theme::typography::{self, TextStyle};
use mdedit_theme::{ColorToken, Theme};

/// Font family names must be `&'static str` in iced; theme names are
/// owned strings, so each distinct name is leaked once.
fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&n) = names.get(name) {
        return n;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// Everything about the look that shaping depends on, resolved once per
/// widget build.
#[derive(Debug, Clone)]
pub(crate) struct Look {
    pub prose_font: Font,
    pub mono_font: Font,
    pub prose: TextStyle,
    pub code: TextStyle,
    pub number: TextStyle,
    pub monospace: bool,
    pub colors: Colors,
    /// Hash of everything above; part of the shaped-line cache key.
    pub generation: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Colors {
    pub bg: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub selection: Color,
    pub warning_bg: Color,
    pub comment: Color,
    pub comment_marker: Color,
    pub marker: Color,
    pub heading: Color,
    pub emphasis: Color,
    pub link: Color,
    pub url: Color,
    pub code: Color,
    pub code_bg: Color,
    pub html: Color,
    pub math: Color,
}

impl Look {
    pub fn new(theme: &Theme, monospace: bool, zoom: f32) -> Self {
        let zoom = if zoom > 0.0 { zoom } else { 1.0 };
        let scaled = |s: TextStyle| TextStyle { size: s.size * zoom, line_height: s.line_height * zoom, ..s };
        let c = |t| Color::from(theme.color(t));
        let colors = Colors {
            bg: c(ColorToken::Bg),
            text: c(ColorToken::Text),
            muted: c(ColorToken::TextMuted),
            accent: c(ColorToken::Accent),
            accent_soft: c(ColorToken::AccentSoft),
            selection: c(ColorToken::Selection),
            warning_bg: c(ColorToken::WarningBg),
            comment: c(ColorToken::CommentHighlight),
            comment_marker: c(ColorToken::WarningText),
            marker: c(ColorToken::SyntaxMarker),
            heading: c(ColorToken::SyntaxHeading),
            emphasis: c(ColorToken::SyntaxEmphasis),
            link: c(ColorToken::SyntaxLink),
            url: c(ColorToken::SyntaxUrl),
            code: c(ColorToken::SyntaxCode),
            code_bg: c(ColorToken::SyntaxCodeBg),
            html: c(ColorToken::SyntaxHtml),
            math: c(ColorToken::SyntaxMath),
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for &t in ColorToken::ALL {
            theme.color(t).hash(&mut h);
        }
        (&theme.fonts.prose, &theme.fonts.mono).hash(&mut h);
        monospace.hash(&mut h);
        zoom.to_bits().hash(&mut h);
        Self {
            prose_font: Font::with_name(intern(&theme.fonts.prose)),
            mono_font: Font::with_name(intern(&theme.fonts.mono)),
            prose: scaled(typography::BODY),
            code: scaled(typography::CODE),
            number: scaled(typography::NUMBER),
            monospace,
            colors,
            generation: h.finish(),
        }
    }

    /// Size and line height for a line of `kind`.
    pub fn metrics(&self, kind: LineKind) -> TextStyle {
        if self.monospace || kind == LineKind::Code { self.code } else { self.prose }
    }

    pub fn base_font(&self, kind: LineKind) -> Font {
        if self.monospace || kind == LineKind::Code { self.mono_font } else { self.prose_font }
    }

    /// Estimated height of a line that has not been shaped yet: line height
    /// times the number of rows `len` bytes would wrap into.
    pub fn estimate(&self, len: usize, width: f32) -> f32 {
        let m = self.metrics(LineKind::Prose);
        let avg_char = m.size * if self.monospace { 0.6 } else { 0.5 };
        let per_row = (width / avg_char).max(1.0);
        let rows = ((len as f32) / per_row).ceil().max(1.0);
        rows * m.line_height
    }
}

/// Prose lines use the prose face; code lines (inside fences or indented
/// code blocks) use the code face and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LineKind {
    Prose,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SpanStyle {
    color: Color,
    bold: bool,
    italic: bool,
    strike: bool,
    mono: bool,
}

/// Styled spans for `line`, plus the byte ranges of inline code (drawn
/// with a background quad).
pub(crate) fn spans<'a>(
    line: &'a str,
    tokens: &[StyledRange],
    kind: LineKind,
    look: &Look,
) -> (Vec<Span<'a, (), Font>>, Vec<Range<usize>>) {
    let colors = &look.colors;
    let base = SpanStyle {
        color: if kind == LineKind::Code { colors.code } else { colors.text },
        bold: false,
        italic: false,
        strike: false,
        mono: kind == LineKind::Code || look.monospace,
    };
    let mut cuts: Vec<usize> = vec![0, line.len()];
    for t in tokens {
        cuts.push(t.range.start.min(line.len()));
        cuts.push(t.range.end.min(line.len()));
    }
    cuts.retain(|&c| line.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();

    let mut out: Vec<Span<'a, (), Font>> = Vec::with_capacity(cuts.len());
    let mut code_ranges: Vec<Range<usize>> = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b {
            continue;
        }
        let mut s = base;
        for t in tokens.iter().filter(|t| t.range.start <= a && t.range.end >= b) {
            match t.token {
                SyntaxToken::Marker => s.color = colors.marker,
                SyntaxToken::Heading(_) => {
                    s.color = colors.heading;
                    s.bold = true;
                }
                SyntaxToken::Emphasis => {
                    s.color = colors.emphasis;
                    s.italic = true;
                }
                SyntaxToken::Strong => {
                    s.color = colors.emphasis;
                    s.bold = true;
                }
                SyntaxToken::Strikethrough => s.strike = true,
                SyntaxToken::Link => s.color = colors.link,
                SyntaxToken::Url => s.color = colors.url,
                SyntaxToken::Code => {
                    s.color = colors.code;
                    s.mono = true;
                    if kind == LineKind::Prose {
                        match code_ranges.last_mut() {
                            Some(r) if r.end == a => r.end = b,
                            _ => code_ranges.push(a..b),
                        }
                    }
                }
                SyntaxToken::Html => s.color = colors.html,
                SyntaxToken::Math => s.color = colors.math,
            }
        }
        let mut font = if s.mono { look.mono_font } else { look.prose_font };
        if s.bold {
            font.weight = Weight::Bold;
        }
        if s.italic {
            font.style = FontStyle::Italic;
        }
        let mut span = Span::new(&line[a..b]).font(font).color(s.color).strikethrough(s.strike);
        // Inline code inside prose uses the code size so it sits visually
        // with the body text (DESIGN_SYSTEM 3.2 `code`).
        if s.mono && !look.monospace && kind == LineKind::Prose {
            span = span.size(Pixels(look.code.size));
        }
        out.push(span);
    }
    (out, code_ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_tokens_split_into_segments() {
        let look = Look::new(&Theme::ember_light(), false, 1.0);
        let line = "# Title `x`";
        let tokens = vec![
            StyledRange { range: 0..11, token: SyntaxToken::Heading(1) },
            StyledRange { range: 0..1, token: SyntaxToken::Marker },
            StyledRange { range: 9..10, token: SyntaxToken::Code },
        ];
        let (spans, code) = spans(line, &tokens, LineKind::Prose, &look);
        let texts: Vec<&str> = spans.iter().map(|s| s.text.as_ref()).collect();
        assert_eq!(texts, ["#", " Title `", "x", "`"]);
        assert_eq!(spans[0].color, Some(look.colors.marker));
        assert_eq!(spans[1].font.unwrap().weight, Weight::Bold);
        assert_eq!(code, vec![9..10]);
    }

    #[test]
    fn plain_line_is_one_span() {
        let look = Look::new(&Theme::ember_dark(), true, 1.0);
        let (spans, _) = spans("hello", &[], LineKind::Prose, &look);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].font, Some(look.mono_font));
    }

    #[test]
    fn zoom_scales_text_and_changes_generation() {
        let t = Theme::ember_light();
        let base = Look::new(&t, false, 1.0);
        assert_eq!(Look::new(&t, false, 0.0).prose.size, base.prose.size);
        let big = Look::new(&t, false, 1.5);
        assert_eq!(big.prose.size, base.prose.size * 1.5);
        assert_eq!(big.code.line_height, base.code.line_height * 1.5);
        assert_eq!(big.number.size, base.number.size * 1.5);
        assert_ne!(big.generation, base.generation);
    }
}
