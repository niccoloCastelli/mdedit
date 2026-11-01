//! Export templates (SPEC 10): `template.toml` (+ optional `template.typ`
//! for PDF). Built-ins: Guide, Report, Plain. Placeholder fields only; the
//! export workstream defines the full schema and documents it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Template {
    pub name: String,
    pub page_size: String,
    /// Margins in mm: top, right, bottom, left.
    pub margins_mm: [f32; 4],
    pub font_body: String,
    pub font_mono: String,
    pub base_size_pt: f32,
    pub accent: String,
    pub heading_numbers: bool,
    pub title_page: bool,
    pub toc: bool,
    /// Placeholders: `{title}`, `{page}`, `{date}`.
    pub header: String,
    pub footer: String,
}

impl Default for Template {
    fn default() -> Self {
        Self {
            name: "Plain".into(),
            page_size: "a4".into(),
            margins_mm: [25.0, 20.0, 25.0, 20.0],
            font_body: "Hanken Grotesk".into(),
            font_mono: "JetBrains Mono".into(),
            base_size_pt: 11.0,
            accent: "#D9542B".into(),
            heading_numbers: false,
            title_page: false,
            toc: false,
            header: String::new(),
            footer: "{page}".into(),
        }
    }
}
