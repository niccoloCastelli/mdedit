//! Design tokens and themes (DESIGN_SYSTEM 2-5).
//!
//! Widgets never hard-code a colour, size or duration: they read a [`Theme`]
//! and the scales in [`typography`] and [`space`]. This crate has no GUI
//! dependency; enable the `iced` feature for `From` conversions and icon
//! handles.
//!
//! - Themes are TOML files ([`file`]): the two built-ins are compiled in,
//!   user themes live in [`user_theme_dir`], `extends` inherits from another
//!   theme and values may reference other tokens.
//! - [`ThemeSettings`] picks the light and dark theme and the
//!   [`Appearance`] (system, light or dark).
//! - [`assets`] holds the bundled fonts (Hanken Grotesk, JetBrains Mono,
//!   Source Serif 4) and Lucide icons with their licences.
//! - [`Theme::code`] is the fenced-code highlighting map; with the default
//!   `syntect` feature, [`Theme::syntect_theme`] turns it into a syntect theme.

pub mod assets;
mod code;
pub mod file;
pub mod space;
pub mod typography;

pub use assets::{fonts, icon};
pub use code::CodeStyle;
pub use file::{Appearance, ThemeError, ThemeInfo, ThemeSet, ThemeSettings, user_theme_dir};

/// sRGB colour with alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Opaque colour from `0xRRGGBB`.
    pub const fn hex(hex: u32) -> Self {
        Self::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    /// `#RRGGBB` or `#RRGGBBAA`, as written in theme files.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        match s.len() {
            6 => Some(Self::hex(v)),
            8 => Some(Self { a: v as u8, ..Self::hex(v >> 8) }),
            _ => None,
        }
    }

    /// WCAG 2 relative luminance (alpha ignored).
    pub fn luminance(self) -> f32 {
        fn channel(c: u8) -> f32 {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// WCAG 2 contrast ratio between two colours, from 1.0 to 21.0.
    pub fn contrast(self, other: Rgba) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

#[cfg(feature = "iced")]
impl From<Rgba> for iced::Color {
    fn from(c: Rgba) -> Self {
        iced::Color::from_rgba8(c.r, c.g, c.b, c.a as f32 / 255.0)
    }
}

macro_rules! color_tokens {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// Semantic colour tokens (DESIGN_SYSTEM 2.1 and 2.4).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ColorToken { $($variant),* }

        impl ColorToken {
            pub const ALL: &[ColorToken] = &[$(ColorToken::$variant),*];

            /// The token's name in theme files, e.g. `"bg-subtle"`.
            pub const fn name(self) -> &'static str {
                match self { $(ColorToken::$variant => $name),* }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                match name { $($name => Some(ColorToken::$variant),)* _ => None }
            }
        }
    };
}

color_tokens! {
    Bg => "bg",
    BgSubtle => "bg-subtle",
    Surface => "surface",
    SurfaceRaised => "surface-raised",
    Border => "border",
    BorderStrong => "border-strong",
    Text => "text",
    TextSecondary => "text-secondary",
    TextMuted => "text-muted",
    Accent => "accent",
    AccentText => "accent-text",
    AccentSoft => "accent-soft",
    OnAccent => "on-accent",
    WarningBg => "warning-bg",
    WarningText => "warning-text",
    Danger => "danger",
    Success => "success",
    Info => "info",
    CommentHighlight => "comment-highlight",
    SyntaxMarker => "syntax-marker",
    SyntaxHeading => "syntax-heading",
    SyntaxEmphasis => "syntax-emphasis",
    SyntaxLink => "syntax-link",
    SyntaxUrl => "syntax-url",
    SyntaxCode => "syntax-code",
    SyntaxCodeBg => "syntax-code-bg",
    SyntaxHtml => "syntax-html",
    SyntaxMath => "syntax-math",
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeKind {
    Light,
    Dark,
}

/// Font family names per role (DESIGN_SYSTEM 3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fonts {
    pub ui: String,
    pub prose: String,
    pub mono: String,
}

impl Default for Fonts {
    fn default() -> Self {
        Self { ui: "Hanken Grotesk".into(), prose: "Hanken Grotesk".into(), mono: "JetBrains Mono".into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub kind: ThemeKind,
    colors: Vec<Rgba>,
    pub fonts: Fonts,
    /// Floating-element shadow colour (DESIGN_SYSTEM 4.3).
    pub shadow: Rgba,
    /// Fenced-code highlighting rules (syntect scope selectors).
    pub code: Vec<CodeStyle>,
}

/// Text/background pairs that must meet a minimum contrast in every theme
/// (DESIGN_SYSTEM 8.7): AA 4.5:1 for text, 3:1 where the spec limits the
/// pair to large or semibold UI text.
pub const CONTRAST_PAIRS: &[(ColorToken, ColorToken, f32)] = {
    use ColorToken::*;
    &[
        (Text, Bg, 4.5),
        (Text, BgSubtle, 4.5),
        (Text, Surface, 4.5),
        (Text, SurfaceRaised, 4.5),
        (Text, AccentSoft, 4.5),
        (Text, CommentHighlight, 4.5),
        (TextSecondary, Bg, 4.5),
        (TextSecondary, BgSubtle, 4.5),
        (TextSecondary, Surface, 4.5),
        (TextSecondary, SurfaceRaised, 4.5),
        (TextMuted, Bg, 4.5),
        (TextMuted, BgSubtle, 4.5),
        (TextMuted, Surface, 4.5),
        (TextMuted, SurfaceRaised, 4.5),
        (AccentText, Bg, 4.5),
        (AccentText, Surface, 4.5),
        (AccentText, AccentSoft, 4.5),
        // on-accent: buttons only, at >= 14 px semibold (DESIGN_SYSTEM 2.2).
        (OnAccent, Accent, 3.0),
        (WarningText, WarningBg, 4.5),
        (Danger, Bg, 4.5),
        (Danger, Surface, 4.5),
        (Success, Bg, 4.5),
        (Info, Bg, 4.5),
        (SyntaxMarker, Bg, 4.5),
        (SyntaxHeading, Bg, 4.5),
        (SyntaxEmphasis, Bg, 4.5),
        (SyntaxLink, Bg, 4.5),
        (SyntaxUrl, Bg, 4.5),
        (SyntaxCode, SyntaxCodeBg, 4.5),
        (SyntaxHtml, Bg, 4.5),
        (SyntaxMath, Bg, 4.5),
    ]
};

/// One pair below its required contrast, from [`Theme::contrast_failures`].
#[derive(Debug, Clone, PartialEq)]
pub struct ContrastFailure {
    /// Token name or code scope selector.
    pub foreground: String,
    pub background: ColorToken,
    pub ratio: f32,
    pub required: f32,
}

impl Theme {
    pub fn color(&self, token: ColorToken) -> Rgba {
        self.colors[token as usize]
    }

    pub fn set_color(&mut self, token: ColorToken, value: Rgba) {
        self.colors[token as usize] = value;
    }

    /// Ember Light (DESIGN_SYSTEM 2.2).
    pub fn ember_light() -> Self {
        file::builtin(file::EMBER_LIGHT)
    }

    /// Ember Dark (DESIGN_SYSTEM 2.3).
    pub fn ember_dark() -> Self {
        file::builtin(file::EMBER_DARK)
    }

    /// The built-in theme of a kind.
    pub fn default_for(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Light => Self::ember_light(),
            ThemeKind::Dark => Self::ember_dark(),
        }
    }

    /// Every [`CONTRAST_PAIRS`] entry and code colour (against
    /// `syntax-code-bg`) below its minimum. Empty for the built-ins; the app
    /// may show these as a warning for user themes.
    pub fn contrast_failures(&self) -> Vec<ContrastFailure> {
        let mut out = Vec::new();
        for &(fg, bg, required) in CONTRAST_PAIRS {
            let ratio = self.color(fg).contrast(self.color(bg));
            if ratio < required {
                out.push(ContrastFailure { foreground: fg.name().into(), background: bg, ratio, required });
            }
        }
        let code_bg = self.color(ColorToken::SyntaxCodeBg);
        for style in &self.code {
            let ratio = style.color.contrast(code_bg);
            if ratio < 4.5 {
                out.push(ContrastFailure {
                    foreground: style.scope.clone(),
                    background: ColorToken::SyntaxCodeBg,
                    ratio,
                    required: 4.5,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_define_every_token() {
        for theme in [Theme::ember_light(), Theme::ember_dark()] {
            for &t in ColorToken::ALL {
                assert_ne!(theme.color(t), Rgba::hex(0xFF00FF), "{} missing {}", theme.name, t.name());
            }
            assert!(!theme.code.is_empty());
        }
    }

    #[test]
    fn token_names_round_trip() {
        for &t in ColorToken::ALL {
            assert_eq!(ColorToken::from_name(t.name()), Some(t));
        }
        assert_eq!(Rgba::parse("#D9542B"), Some(Rgba::hex(0xD9542B)));
        assert_eq!(Rgba::parse("#1B1A191A"), Some(Rgba { r: 0x1B, g: 0x1A, b: 0x19, a: 0x1A }));
        assert_eq!(Rgba::parse("#+12345"), None);
        assert_eq!(Rgba::parse("D9542B"), None);
    }

    #[test]
    fn built_ins_match_design_system() {
        let light = Theme::ember_light();
        assert_eq!(light.kind, ThemeKind::Light);
        assert_eq!(light.color(ColorToken::Accent), Rgba::hex(0xD9542B));
        assert_eq!(light.color(ColorToken::SyntaxLink), light.color(ColorToken::AccentText));
        assert_eq!(light.color(ColorToken::CommentHighlight), light.color(ColorToken::WarningBg));
        assert_eq!(light.shadow, Rgba { r: 27, g: 26, b: 25, a: 26 });
        let dark = Theme::ember_dark();
        assert_eq!(dark.kind, ThemeKind::Dark);
        assert_eq!(dark.color(ColorToken::SyntaxCode), Rgba::hex(0xD4D2CF));
        assert_eq!(dark.color(ColorToken::SyntaxCodeBg), Rgba::hex(0x161514));
        assert_eq!(dark.shadow, Rgba { r: 0, g: 0, b: 0, a: 115 });
    }

    #[test]
    fn contrast_ratio_matches_wcag() {
        assert!((Rgba::hex(0x000000).contrast(Rgba::hex(0xFFFFFF)) - 21.0).abs() < 0.01);
        assert!((Rgba::hex(0x777777).contrast(Rgba::hex(0xFFFFFF)) - 4.48).abs() < 0.01);
    }

    /// DESIGN_SYSTEM 8.7: every built-in theme meets WCAG AA.
    #[test]
    fn built_ins_meet_wcag_aa() {
        for theme in [Theme::ember_light(), Theme::ember_dark()] {
            let failures = theme.contrast_failures();
            assert!(failures.is_empty(), "{}: {failures:#?}", theme.name);
        }
    }
}
