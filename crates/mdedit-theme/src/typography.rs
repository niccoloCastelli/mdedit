//! Type scale (DESIGN_SYSTEM 3.2). Sizes and line heights in logical px at
//! zoom 1.0; the app multiplies by the user zoom.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    Ui,
    Prose,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub line_height: f32,
    pub weight: u16,
    /// Letter spacing in em.
    pub tracking: f32,
    pub uppercase: bool,
    pub role: FontRole,
}

const fn style(size: f32, line_height: f32, weight: u16, tracking: f32, role: FontRole) -> TextStyle {
    TextStyle { size, line_height, weight, tracking, uppercase: false, role }
}

pub const DISPLAY: TextStyle = style(32.0, 38.0, 700, -0.02, FontRole::Prose);
pub const H2: TextStyle = style(26.0, 32.0, 700, -0.015, FontRole::Prose);
pub const H3: TextStyle = style(21.0, 28.0, 600, -0.01, FontRole::Prose);
pub const H4: TextStyle = style(18.0, 26.0, 600, 0.0, FontRole::Prose);
pub const H5: TextStyle = style(16.0, 24.0, 600, 0.0, FontRole::Prose);
pub const H6: TextStyle = H5;
pub const BODY: TextStyle = style(16.0, 27.0, 400, 0.0, FontRole::Prose);
pub const BODY_LEAD: TextStyle = style(19.0, 30.0, 400, 0.0, FontRole::Prose);
pub const CODE: TextStyle = style(14.0, 22.0, 400, 0.0, FontRole::Mono);
pub const UI: TextStyle = style(13.0, 18.0, 400, 0.0, FontRole::Ui);
pub const UI_SMALL: TextStyle = style(12.0, 16.0, 400, 0.0, FontRole::Ui);
pub const EYEBROW: TextStyle = TextStyle { uppercase: true, ..style(11.0, 16.0, 600, 0.08, FontRole::Ui) };
pub const NUMBER: TextStyle = style(13.0, 18.0, 400, 0.0, FontRole::Mono);

/// Heading style by level 1-6.
pub const fn heading(level: u8) -> TextStyle {
    match level {
        1 => DISPLAY,
        2 => H2,
        3 => H3,
        4 => H4,
        5 => H5,
        _ => H6,
    }
}

/// Document column width (DESIGN_SYSTEM 3.3).
pub const MEASURE: f32 = 720.0;
/// Wide elements (tables, images, code) may extend to this.
pub const MEASURE_WIDE: f32 = 960.0;
