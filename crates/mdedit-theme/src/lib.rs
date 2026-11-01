//! Design tokens and themes (DESIGN_SYSTEM 2-4).
//!
//! Widgets never hard-code a colour, size or duration: they read a [`Theme`]
//! and the scales in [`typography`] and [`space`]. This crate has no GUI
//! dependency; enable the `iced` feature for `From` conversions.
//!
//! Theming workstream: TOML theme files with `extends`, user theme dir,
//! settings-driven light/dark selection, the WCAG contrast test over every
//! built-in theme, bundled fonts (Hanken Grotesk, JetBrains Mono, Source
//! Serif 4) and Lucide icons.

pub mod space;
pub mod typography;

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
        let v = u32::from_str_radix(s, 16).ok()?;
        match s.len() {
            6 => Some(Self::hex(v)),
            8 => Some(Self { a: v as u8, ..Self::hex(v >> 8) }),
            _ => None,
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl Theme {
    pub fn color(&self, token: ColorToken) -> Rgba {
        self.colors[token as usize]
    }

    pub fn set_color(&mut self, token: ColorToken, value: Rgba) {
        self.colors[token as usize] = value;
    }

    fn from_pairs(name: &str, kind: ThemeKind, shadow: Rgba, pairs: &[(ColorToken, u32)]) -> Self {
        let mut colors = vec![Rgba::hex(0xFF00FF); ColorToken::ALL.len()];
        for &(token, hex) in pairs {
            colors[token as usize] = Rgba::hex(hex);
        }
        Self { name: name.into(), kind, colors, fonts: Fonts::default(), shadow }
    }

    /// Ember Light (DESIGN_SYSTEM 2.2).
    pub fn ember_light() -> Self {
        use ColorToken::*;
        Self::from_pairs(
            "Ember Light",
            ThemeKind::Light,
            Rgba { r: 27, g: 26, b: 25, a: 26 },
            &[
                (Bg, 0xFFFFFF),
                (BgSubtle, 0xF5F3F1),
                (Surface, 0xFFFFFF),
                (SurfaceRaised, 0xFBFAFA),
                (Border, 0xE3DFDA),
                (BorderStrong, 0xC6C5C3),
                (Text, 0x1B1A19),
                (TextSecondary, 0x5F5B57),
                (TextMuted, 0x78746F),
                (Accent, 0xD9542B),
                (AccentText, 0xB8431F),
                (AccentSoft, 0xFBE9E2),
                (OnAccent, 0xFFFFFF),
                (WarningBg, 0xFBF0D9),
                (WarningText, 0x8A5A00),
                (Danger, 0xB42335),
                (Success, 0x3F7D4E),
                (Info, 0x2F6FAE),
                (CommentHighlight, 0xFBF0D9),
                (SyntaxMarker, 0x78746F),
                (SyntaxHeading, 0x1B1A19),
                (SyntaxEmphasis, 0x1B1A19),
                (SyntaxLink, 0xB8431F),
                (SyntaxUrl, 0x78746F),
                (SyntaxCode, 0x5F5B57),
                (SyntaxCodeBg, 0xF5F3F1),
                (SyntaxHtml, 0x2F6FAE),
                (SyntaxMath, 0x2F6FAE),
            ],
        )
    }

    /// Ember Dark (DESIGN_SYSTEM 2.3).
    pub fn ember_dark() -> Self {
        use ColorToken::*;
        Self::from_pairs(
            "Ember Dark",
            ThemeKind::Dark,
            Rgba { r: 0, g: 0, b: 0, a: 115 },
            &[
                (Bg, 0x111111),
                (BgSubtle, 0x161514),
                (Surface, 0x1B1A19),
                (SurfaceRaised, 0x1E1E1E),
                (Border, 0x302F2E),
                (BorderStrong, 0x444342),
                (Text, 0xEAEAE9),
                (TextSecondary, 0xAEAEAD),
                (TextMuted, 0x8E8D8D),
                (Accent, 0xE86A43),
                (AccentText, 0xFF8A5C),
                (AccentSoft, 0x3A2118),
                (OnAccent, 0x111111),
                (WarningBg, 0x2E2412),
                (WarningText, 0xD6A24A),
                (Danger, 0xF0707F),
                (Success, 0x6FBF86),
                (Info, 0x7FB2E5),
                (CommentHighlight, 0x3A2E14),
                (SyntaxMarker, 0x8E8D8D),
                (SyntaxHeading, 0xEAEAE9),
                (SyntaxEmphasis, 0xEAEAE9),
                (SyntaxLink, 0xFF8A5C),
                (SyntaxUrl, 0x8E8D8D),
                (SyntaxCode, 0xD4D2CF),
                (SyntaxCodeBg, 0x161514),
                (SyntaxHtml, 0x7FB2E5),
                (SyntaxMath, 0x7FB2E5),
            ],
        )
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
        }
    }

    #[test]
    fn token_names_round_trip() {
        for &t in ColorToken::ALL {
            assert_eq!(ColorToken::from_name(t.name()), Some(t));
        }
        assert_eq!(Rgba::parse("#D9542B"), Some(Rgba::hex(0xD9542B)));
    }
}
