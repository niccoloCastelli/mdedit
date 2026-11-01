//! Theme tokens resolved for drawing (DESIGN_SYSTEM 2, 3, 7.1).

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use iced::font::{Style as FontStyle, Weight};
use iced::{Color, Font};
use mdedit_theme::typography::{self, TextStyle};
use mdedit_theme::{ColorToken, Theme, ThemeKind};

/// Font family names must be `&'static str` in iced; each distinct theme
/// font name is leaked once.
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct Colors {
    pub bg: Color,
    pub bg_subtle: Color,
    pub surface: Color,
    pub border: Color,
    pub border_strong: Color,
    pub text: Color,
    pub secondary: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub accent_soft: Color,
    pub selection: Color,
    pub on_accent: Color,
    pub warning_bg: Color,
    pub warning_text: Color,
    pub danger: Color,
    pub success: Color,
    pub info: Color,
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

/// Everything about the look that layout depends on.
#[derive(Debug, Clone)]
pub(crate) struct Look {
    pub prose: Font,
    pub mono: Font,
    pub ui: Font,
    pub colors: Colors,
    pub dark: bool,
    /// Hash of the above; part of every layout cache key.
    pub generation: u64,
}

impl Look {
    pub fn new(theme: &Theme) -> Self {
        let c = |t| Color::from(theme.color(t));
        let colors = Colors {
            bg: c(ColorToken::Bg),
            bg_subtle: c(ColorToken::BgSubtle),
            surface: c(ColorToken::Surface),
            border: c(ColorToken::Border),
            border_strong: c(ColorToken::BorderStrong),
            text: c(ColorToken::Text),
            secondary: c(ColorToken::TextSecondary),
            muted: c(ColorToken::TextMuted),
            accent: c(ColorToken::Accent),
            accent_text: c(ColorToken::AccentText),
            accent_soft: c(ColorToken::AccentSoft),
            selection: c(ColorToken::Selection),
            on_accent: c(ColorToken::OnAccent),
            warning_bg: c(ColorToken::WarningBg),
            warning_text: c(ColorToken::WarningText),
            danger: c(ColorToken::Danger),
            success: c(ColorToken::Success),
            info: c(ColorToken::Info),
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
        (&theme.fonts.prose, &theme.fonts.mono, &theme.fonts.ui).hash(&mut h);
        Self {
            prose: Font::with_name(intern(&theme.fonts.prose)),
            mono: Font::with_name(intern(&theme.fonts.mono)),
            ui: Font::with_name(intern(&theme.fonts.ui)),
            colors,
            dark: theme.kind == ThemeKind::Dark,
            generation: h.finish(),
        }
    }

    pub fn font(&self, style: &TextStyle) -> Font {
        let base = match style.role {
            typography::FontRole::Ui => self.ui,
            typography::FontRole::Prose => self.prose,
            typography::FontRole::Mono => self.mono,
        };
        Font { weight: weight(style.weight), ..base }
    }
}

pub(crate) fn weight(w: u16) -> Weight {
    match w {
        0..=150 => Weight::Thin,
        151..=250 => Weight::ExtraLight,
        251..=350 => Weight::Light,
        351..=450 => Weight::Normal,
        451..=550 => Weight::Medium,
        551..=650 => Weight::Semibold,
        651..=750 => Weight::Bold,
        751..=850 => Weight::ExtraBold,
        _ => Weight::Black,
    }
}

pub(crate) fn italic(font: Font) -> Font {
    Font { style: FontStyle::Italic, ..font }
}

pub(crate) fn bold(font: Font) -> Font {
    Font { weight: Weight::Bold, ..font }
}
