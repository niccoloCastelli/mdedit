//! Export templates (SPEC 10). A template is a directory with:
//!
//! - `template.toml`: the [`Template`] fields below. Every field is
//!   optional and defaults to the Plain look; the options map to both PDF
//!   and DOCX.
//! - `template.typ` (optional): a Typst template replacing the built-in
//!   one for PDF. It must define the functions the generated markup calls
//!   (see `templates/mdedit.typ`, the built-in one, which is the reference)
//!   and reads the template options from `/__mdedit/conf.typ`.
//! - `fonts/` (optional): extra `.ttf`/`.otf`/`.ttc` files for PDF export.
//!
//! Built-ins: Guide (the look of the reference PDF), Report, Plain. Lookup
//! order for a name: `<dir>/<name>/template.toml` in each search dir (the
//! project's `.mdedit/templates`, then the global config dir), then the
//! built-ins (case-insensitive).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ExportError;

/// The built-in Typst template, used when a template has no `template.typ`.
pub const BUILTIN_TYPST: &str = include_str!("../templates/mdedit.typ");

const BUILTINS: &[(&str, &str)] = &[
    ("guide", include_str!("../templates/guide/template.toml")),
    ("report", include_str!("../templates/report/template.toml")),
    ("plain", include_str!("../templates/plain/template.toml")),
];

/// The overall look; selects layout details the colour and size options
/// cannot express (section numbers, table frame, list markers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Eyebrow over the title, rule and `01` number over each section,
    /// rounded tables, callout boxes, numbered step badges.
    Guide,
    /// Classic report: ruled tables, quiet headings.
    Report,
    #[default]
    Plain,
}

/// Heading number format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NumberStyle {
    /// `1`, `1.1`, `1.1.1`.
    #[default]
    Decimal,
    /// Two-digit top-level numbers in the mono font and accent colour
    /// (`01`, `02`), as in the reference PDF; sub-levels are not numbered.
    Guide,
}

/// Horizontal slot of a header/footer element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl HAlign {
    pub const ALL: [HAlign; 3] = [HAlign::Left, HAlign::Center, HAlign::Right];

    pub fn as_str(self) -> &'static str {
        match self {
            HAlign::Left => "left",
            HAlign::Center => "center",
            HAlign::Right => "right",
        }
    }
}

/// A page header or footer. In `template.toml` it is either a plain
/// string (the `text`) or a table with these keys:
///
/// ```toml
/// [header]
/// text = "{title} | | {date}"
/// logo = "logo.png"          # relative to the template directory
/// logo_position = "left"     # left | center | right
/// logo_height_mm = 8.0
/// color = "#716D68"          # empty: colors.text_muted
/// size_pt = 8.0              # 0: 80 % of base_size_pt
/// font = ""                  # empty: font_body
/// bold = false
/// rule = true                # hairline between it and the body
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Running {
    /// Placeholders: `{title}`, `{page}`, `{pages}`, `{date}`. Split into
    /// left, centre and right parts with `|`; a single part is centred.
    pub text: String,
    /// Image file (PNG, JPEG, SVG); relative paths are resolved against
    /// the template directory. Empty: no logo.
    pub logo: String,
    pub logo_position: HAlign,
    pub logo_height_mm: f32,
    pub color: String,
    pub size_pt: f32,
    pub font: String,
    pub bold: bool,
    pub rule: bool,
}

impl Default for Running {
    fn default() -> Self {
        Self {
            text: String::new(),
            logo: String::new(),
            logo_position: HAlign::Left,
            logo_height_mm: 8.0,
            color: String::new(),
            size_pt: 0.0,
            font: String::new(),
            bold: false,
            rule: false,
        }
    }
}

impl Running {
    pub fn text(text: &str) -> Self {
        Self { text: text.into(), ..Self::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.logo.is_empty()
    }

    /// Font size in points for a template's base size.
    pub fn size(&self, base_pt: f32) -> f32 {
        if self.size_pt > 0.0 { self.size_pt } else { base_pt * 0.8 }
    }
}

/// `header = "text"` or a `[header]` table.
fn running<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Running, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Text(String),
        Full(Running),
    }
    Ok(match Repr::deserialize(d)? {
        Repr::Text(text) => Running::text(&text),
        Repr::Full(r) => r,
    })
}

/// Colours other than the accent, as `#RRGGBB`. Defaults are Ember Light
/// (DESIGN_SYSTEM 2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Colors {
    pub text: String,
    pub text_secondary: String,
    pub text_muted: String,
    pub accent_text: String,
    pub accent_soft: String,
    pub border: String,
    pub bg_subtle: String,
    pub warning_bg: String,
    pub warning_text: String,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            text: "#1B1A19".into(),
            text_secondary: "#5F5B57".into(),
            text_muted: "#716D68".into(),
            accent_text: "#B8431F".into(),
            accent_soft: "#FBE9E2".into(),
            border: "#E3DFDA".into(),
            bg_subtle: "#F5F3F1".into(),
            warning_bg: "#FBF0D9".into(),
            warning_text: "#8A5A00".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Template {
    pub name: String,
    pub style: Style,
    /// `a3`, `a4`, `a5`, `b5`, `us-letter`, `us-legal`.
    pub page_size: String,
    /// Margins in mm: top, right, bottom, left.
    pub margins_mm: [f32; 4],
    pub font_body: String,
    /// Headings; empty means `font_body`.
    pub font_heading: String,
    pub font_mono: String,
    pub base_size_pt: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    pub accent: String,
    pub colors: Colors,
    pub heading_numbers: bool,
    pub number_style: NumberStyle,
    pub title_page: bool,
    pub toc: bool,
    /// Small uppercase accent line above the title. Placeholders as in
    /// `header`.
    pub eyebrow: String,
    /// Style the first paragraph after the title as a lead paragraph.
    pub lead: bool,
    /// See [`Running`]; a plain string is the text.
    #[serde(deserialize_with = "running")]
    pub header: Running,
    #[serde(deserialize_with = "running")]
    pub footer: Running,
    /// Built-in mdedit theme whose `[code]` colours highlight code blocks
    /// (`ember-light`, `ember-dark`).
    pub code_theme: String,
    /// `template.typ` contents, if the template has one.
    #[serde(skip)]
    pub typst_source: Option<String>,
    /// Directory the template was loaded from (fonts are read from
    /// `fonts/` in it).
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

impl Default for Template {
    fn default() -> Self {
        Self {
            name: "Plain".into(),
            style: Style::Plain,
            page_size: "a4".into(),
            margins_mm: [25.0, 20.0, 25.0, 20.0],
            font_body: "Hanken Grotesk".into(),
            font_heading: String::new(),
            font_mono: "JetBrains Mono".into(),
            base_size_pt: 11.0,
            line_height: 1.5,
            accent: "#D9542B".into(),
            colors: Colors::default(),
            heading_numbers: false,
            number_style: NumberStyle::Decimal,
            title_page: false,
            toc: false,
            eyebrow: String::new(),
            lead: false,
            header: Running::default(),
            footer: Running::text("{page}"),
            code_theme: "ember-light".into(),
            typst_source: None,
            dir: None,
        }
    }
}

impl Template {
    pub fn from_toml(src: &str) -> Result<Self, ExportError> {
        let template: Template = toml::from_str(src).map_err(|e| ExportError::Template(e.to_string()))?;
        template.validate()?;
        Ok(template)
    }

    /// A built-in template by name, case-insensitive.
    pub fn builtin(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        BUILTINS
            .iter()
            .find(|(id, _)| *id == name)
            .map(|(_, src)| Self::from_toml(src).expect("built-in template parses"))
    }

    /// Guide, Report and Plain.
    pub fn builtins() -> Vec<Self> {
        BUILTINS.iter().map(|(id, _)| Self::builtin(id).expect("built-in")).collect()
    }

    /// Load `dir/template.toml`, plus `template.typ` if present. Without a
    /// `name` key the directory name is the template name.
    pub fn load_dir(dir: &Path) -> Result<Self, ExportError> {
        let src = std::fs::read_to_string(dir.join("template.toml"))?;
        let mut template = Self::from_toml(&src)?;
        let has_name = src.parse::<toml::Table>().is_ok_and(|t| t.contains_key("name"));
        if !has_name && let Some(dir_name) = dir.file_name() {
            template.name = dir_name.to_string_lossy().into_owned();
        }
        match std::fs::read_to_string(dir.join("template.typ")) {
            Ok(typ) => template.typst_source = Some(typ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        template.dir = Some(dir.to_path_buf());
        Ok(template)
    }

    /// Find `name` in `search_dirs` (each holding `<name>/template.toml`),
    /// then among the built-ins.
    pub fn resolve(name: &str, search_dirs: &[PathBuf]) -> Result<Self, ExportError> {
        for dir in search_dirs {
            let candidate = dir.join(name);
            if candidate.join("template.toml").is_file() {
                return Self::load_dir(&candidate);
            }
        }
        Self::builtin(name).ok_or_else(|| ExportError::Template(format!("no template named {name:?}")))
    }

    /// The Typst template source: `template.typ` or the built-in one.
    pub fn typst(&self) -> &str {
        self.typst_source.as_deref().unwrap_or(BUILTIN_TYPST)
    }

    /// The logo file of a header or footer: absolute, or relative to the
    /// template directory (built-ins have none, so relative logos there
    /// resolve against the current directory and usually don't exist).
    pub fn logo_path(&self, r: &Running) -> Option<PathBuf> {
        if r.logo.trim().is_empty() {
            return None;
        }
        let path = Path::new(r.logo.trim());
        Some(match &self.dir {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path.to_path_buf(),
        })
    }

    /// `template.toml` text for this template (used by the template
    /// editor).
    pub fn to_toml(&self) -> Result<String, ExportError> {
        toml::to_string_pretty(self).map_err(|e| ExportError::Template(e.to_string()))
    }

    pub fn heading_font(&self) -> &str {
        if self.font_heading.is_empty() { &self.font_body } else { &self.font_heading }
    }

    fn validate(&self) -> Result<(), ExportError> {
        let c = &self.colors;
        for (field, value) in [
            ("accent", &self.accent),
            ("colors.text", &c.text),
            ("colors.text_secondary", &c.text_secondary),
            ("colors.text_muted", &c.text_muted),
            ("colors.accent_text", &c.accent_text),
            ("colors.accent_soft", &c.accent_soft),
            ("colors.border", &c.border),
            ("colors.bg_subtle", &c.bg_subtle),
            ("colors.warning_bg", &c.warning_bg),
            ("colors.warning_text", &c.warning_text),
        ] {
            if parse_hex(value).is_none() {
                return Err(ExportError::Template(format!("{field}: {value:?} is not a #RRGGBB colour")));
            }
        }
        for (field, r) in [("header", &self.header), ("footer", &self.footer)] {
            if !r.color.is_empty() && parse_hex(&r.color).is_none() {
                return Err(ExportError::Template(format!("{field}.color: {:?} is not a #RRGGBB colour", r.color)));
            }
            if !(r.logo_height_mm > 0.0 && r.size_pt >= 0.0) {
                return Err(ExportError::Template(format!("{field}: logo_height_mm and size_pt must be positive")));
            }
        }
        if !(self.base_size_pt > 0.0 && self.line_height > 0.0) {
            return Err(ExportError::Template("base_size_pt and line_height must be positive".into()));
        }
        if page_size_mm(&self.page_size).is_none() {
            return Err(ExportError::Template(format!("unknown page_size {:?}", self.page_size)));
        }
        Ok(())
    }
}

/// `#RRGGBB` (or `RRGGBB`) to six lowercase hex digits.
pub(crate) fn parse_hex(s: &str) -> Option<String> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
}

/// Width and height in mm of the paper sizes both outputs support, with
/// the Typst paper name.
pub(crate) fn page_size_mm(name: &str) -> Option<(f32, f32, &'static str)> {
    Some(match name.to_ascii_lowercase().as_str() {
        "a3" => (297.0, 420.0, "a3"),
        "a4" => (210.0, 297.0, "a4"),
        "a5" => (148.0, 210.0, "a5"),
        "b5" => (176.0, 250.0, "iso-b5"),
        "us-letter" | "letter" => (215.9, 279.4, "us-letter"),
        "us-legal" | "legal" => (215.9, 355.6, "us-legal"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_and_differ() {
        let all = Template::builtins();
        let names: Vec<_> = all.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Guide", "Report", "Plain"]);
        let guide = Template::builtin("GUIDE").unwrap();
        assert_eq!(guide.style, Style::Guide);
        assert_eq!(guide.number_style, NumberStyle::Guide);
        assert_eq!(Template::builtin("plain").unwrap(), Template::default());
        assert!(Template::builtin("nope").is_none());
    }

    #[test]
    fn partial_toml_falls_back_to_defaults() {
        let t = Template::from_toml("name = \"Mine\"\naccent = \"#112233\"\n[colors]\ntext = \"#000000\"\n").unwrap();
        assert_eq!(t.accent, "#112233");
        assert_eq!(t.colors.text, "#000000");
        assert_eq!(t.colors.border, Colors::default().border);
        assert_eq!(t.page_size, "a4");
    }

    #[test]
    fn header_and_footer_as_string_or_table() {
        let t = Template::from_toml("header = \"{title}\"\n[footer]\ntext = \"{page}\"\nlogo = \"logo.png\"\nlogo_position = \"right\"\ncolor = \"#112233\"\nsize_pt = 7.5\nrule = true\n").unwrap();
        assert_eq!(t.header, Running::text("{title}"));
        assert_eq!(t.footer.logo_position, HAlign::Right);
        assert_eq!((t.footer.size(10.0), t.header.size(10.0)), (7.5, 8.0));
        assert!(t.footer.rule && !t.footer.bold);
        assert_eq!(t.logo_path(&t.footer), Some(PathBuf::from("logo.png")));
        assert!(Template::from_toml("[header]\ncolor = \"red\"").is_err());
        assert!(Template::from_toml("[header]\nlogo_height_mm = 0").is_err());
    }

    #[test]
    fn toml_round_trip() {
        for mut t in Template::builtins() {
            t.header = Running { logo: "logo.svg".into(), rule: true, ..Running::text("{title} | | {date}") };
            let back = Template::from_toml(&t.to_toml().unwrap()).unwrap();
            assert_eq!(back, t);
        }
    }

    #[test]
    fn invalid_values_are_errors() {
        assert!(Template::from_toml("accent = \"orange\"").is_err());
        assert!(Template::from_toml("page_size = \"tabloid-ish\"").is_err());
        assert!(Template::from_toml("style = \"fancy\"").is_err());
    }

    #[test]
    fn resolve_prefers_search_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("guide");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("template.toml"), "style = \"guide\"\n").unwrap();
        std::fs::write(dir.join("template.typ"), "// custom\n").unwrap();
        let t = Template::resolve("guide", &[tmp.path().to_path_buf()]).unwrap();
        assert_eq!(t.name, "guide");
        assert_eq!(t.typst(), "// custom\n");
        assert_eq!(Template::resolve("Report", &[tmp.path().to_path_buf()]).unwrap().name, "Report");
    }
}
