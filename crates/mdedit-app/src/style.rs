//! iced styles built from theme tokens (DESIGN_SYSTEM 2, 4, 7). Widgets in
//! the shell never use a literal colour; they go through these helpers.

use std::collections::HashMap;
use std::sync::Mutex;

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Font, Shadow, Vector, border};
use mdedit_theme::space::{RADIUS_LG, RADIUS_MD};
use mdedit_theme::{ColorToken as T, Theme};

pub fn c(theme: &Theme, token: T) -> Color {
    theme.color(token).into()
}

fn with_alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// The iced theme behind built-in widgets we don't restyle (scrollbars).
pub fn iced_theme(theme: &Theme) -> iced::Theme {
    iced::Theme::custom(
        theme.name.clone(),
        iced::theme::Palette {
            background: c(theme, T::Bg),
            text: c(theme, T::Text),
            primary: c(theme, T::Accent),
            success: c(theme, T::Success),
            warning: c(theme, T::WarningText),
            danger: c(theme, T::Danger),
        },
    )
}

/// A font by family name. iced wants `&'static str`; the few names a
/// session uses are interned once.
pub fn font(name: &str) -> Font {
    static NAMES: Mutex<Option<HashMap<String, &'static str>>> = Mutex::new(None);
    if name.is_empty() {
        return Font::DEFAULT;
    }
    let mut guard = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    let name: &'static str =
        map.entry(name.to_string()).or_insert_with(|| Box::leak(name.to_string().into_boxed_str()));
    Font::with_name(name)
}

fn shadow(theme: &Theme) -> Shadow {
    Shadow { color: theme.shadow.into(), offset: Vector::new(0.0, 4.0), blur_radius: 16.0 }
}

/// Window background.
pub fn app_bg(theme: &Theme) -> impl Fn(&iced::Theme) -> container::Style + '_ {
    move |_| container::Style { background: Some(c(theme, T::Bg).into()), ..Default::default() }
}

/// Flat chrome (menu bar, sidebar, status bar): `bg-subtle`, no shadow.
pub fn chrome(theme: &Theme) -> impl Fn(&iced::Theme) -> container::Style + '_ {
    move |_| container::Style {
        background: Some(c(theme, T::BgSubtle).into()),
        text_color: Some(c(theme, T::TextSecondary)),
        ..Default::default()
    }
}

/// 1 px divider.
pub fn divider(theme: &Theme) -> impl Fn(&iced::Theme) -> container::Style + '_ {
    move |_| container::Style { background: Some(c(theme, T::Border).into()), ..Default::default() }
}

/// Floating surfaces: menus, palette, dialogs.
pub fn popover(theme: &Theme, radius: f32) -> impl Fn(&iced::Theme) -> container::Style + '_ {
    move |_| container::Style {
        background: Some(c(theme, T::Surface).into()),
        text_color: Some(c(theme, T::Text)),
        border: Border { color: c(theme, T::Border), width: 1.0, radius: radius.into() },
        shadow: shadow(theme),
        snap: true,
    }
}

/// Dimmed backdrop behind modal dialogs and the palette.
pub fn scrim(theme: &Theme) -> impl Fn(&iced::Theme) -> container::Style + '_ {
    move |_| {
        let base = c(theme, T::Text);
        container::Style { background: Some(with_alpha(base, 0.12).into()), ..Default::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Banner {
    Info,
    Warning,
}

pub fn banner(theme: &Theme, kind: Banner) -> impl Fn(&iced::Theme) -> container::Style + '_ {
    move |_| {
        let (bg, fg) = match kind {
            Banner::Info => (c(theme, T::BgSubtle), c(theme, T::Text)),
            Banner::Warning => (c(theme, T::WarningBg), c(theme, T::WarningText)),
        };
        container::Style {
            background: Some(bg.into()),
            text_color: Some(fg),
            border: Border { color: c(theme, T::Border), width: 1.0, radius: RADIUS_LG.into() },
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Primary,
    Secondary,
    Ghost,
    Danger,
    /// Menu bar title / menu row: ghost, square, `surface-raised` on hover.
    MenuItem,
    /// Selected palette row or open menu title.
    Selected,
    /// Icon button in the toggled state.
    Toggled,
}

pub fn btn(theme: &Theme, kind: Btn) -> impl Fn(&iced::Theme, button::Status) -> button::Style + '_ {
    move |_, status| {
        use button::Status::*;
        let hovered = matches!(status, Hovered | Pressed);
        let radius = match kind {
            Btn::MenuItem | Btn::Selected => 4.0,
            _ => RADIUS_MD,
        };
        let (bg, fg, border_color) = match kind {
            Btn::Primary => (Some(c(theme, T::Accent)), c(theme, T::OnAccent), Color::TRANSPARENT),
            Btn::Secondary => (
                Some(if hovered { c(theme, T::SurfaceRaised) } else { c(theme, T::Surface) }),
                c(theme, T::Text),
                c(theme, T::Border),
            ),
            Btn::Danger => (Some(c(theme, T::Danger)), c(theme, T::OnAccent), Color::TRANSPARENT),
            Btn::Ghost | Btn::MenuItem => {
                (hovered.then(|| c(theme, T::SurfaceRaised)), c(theme, T::TextSecondary), Color::TRANSPARENT)
            }
            Btn::Selected => (Some(c(theme, T::AccentSoft)), c(theme, T::Text), Color::TRANSPARENT),
            Btn::Toggled => (Some(c(theme, T::AccentSoft)), c(theme, T::Accent), Color::TRANSPARENT),
        };
        let mut style = button::Style {
            background: bg.map(Background::Color),
            text_color: if hovered && matches!(kind, Btn::Ghost | Btn::MenuItem) { c(theme, T::Text) } else { fg },
            border: Border { color: border_color, width: 1.0, radius: radius.into() },
            shadow: Shadow::default(),
            snap: true,
        };
        if status == Disabled {
            style.text_color = with_alpha(style.text_color, 0.4);
            style.background = style.background.map(|b| b.scale_alpha(0.4));
        }
        style
    }
}

/// Document tabs (DESIGN_SYSTEM 7): active has `bg` and a 2 px accent
/// bottom line, drawn by the caller under the button.
pub fn tab(theme: &Theme, active: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style + '_ {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let bg = if active {
            c(theme, T::Bg)
        } else if hovered {
            c(theme, T::SurfaceRaised)
        } else {
            c(theme, T::BgSubtle)
        };
        button::Style {
            background: Some(bg.into()),
            text_color: if active { c(theme, T::Text) } else { c(theme, T::TextSecondary) },
            border: border::rounded(0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

pub fn accent_line(theme: &Theme, on: bool) -> impl Fn(&iced::Theme) -> container::Style + '_ {
    move |_| container::Style {
        background: Some(if on { c(theme, T::Accent) } else { Color::TRANSPARENT }.into()),
        ..Default::default()
    }
}

pub fn input(theme: &Theme) -> impl Fn(&iced::Theme, text_input::Status) -> text_input::Style + '_ {
    move |_, status| {
        let focused = matches!(status, text_input::Status::Focused { .. });
        text_input::Style {
            background: c(theme, T::Surface).into(),
            border: Border {
                color: if focused { c(theme, T::Accent) } else { c(theme, T::BorderStrong) },
                width: if focused { 2.0 } else { 1.0 },
                radius: RADIUS_MD.into(),
            },
            icon: c(theme, T::TextMuted),
            placeholder: c(theme, T::TextMuted),
            value: c(theme, T::Text),
            selection: c(theme, T::AccentSoft),
        }
    }
}
