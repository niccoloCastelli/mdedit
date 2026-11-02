//! Template editor (SPEC 10): the export dialog's left pane turns into a
//! form over a working copy of the selected template. Every change goes to
//! the live preview; "Save" writes `<dir>/<slug>/template.toml` into the
//! project's or the global templates folder, copying logos next to it, and
//! "Save as…" writes a copy under a new name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use iced::widget::{Space, button, checkbox, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Background, Border, Element, Length, Padding};
use mdedit_export::template::{HAlign, NumberStyle, Running, Style};
use mdedit_export::{ExportError, Template};
use mdedit_theme::space::*;
use mdedit_theme::{ColorToken as T, Theme};

use super::{EYEBROW, ExportMsg, UI, UI_SMALL, checkbox_style};
use crate::app::Message;
use crate::style::{self, Btn, c};
use crate::view::bold;

/// Families bundled with the app (and so with every export).
pub const FONTS: [&str; 3] = ["Hanken Grotesk", "Source Serif 4", "JetBrains Mono"];
const PAGE_SIZES: [(&str, &str); 6] =
    [("a4", "A4"), ("us-letter", "Letter"), ("a5", "A5"), ("a3", "A3"), ("b5", "B5"), ("us-legal", "Legal")];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Header,
    Footer,
}

/// Every editable template option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Name,
    Style,
    PageSize,
    Margin(usize),
    FontBody,
    FontHeading,
    FontMono,
    BaseSize,
    LineHeight,
    Accent,
    Color(usize),
    HeadingNumbers,
    NumberStyle,
    TitlePage,
    Toc,
    Lead,
    Eyebrow,
    CodeTheme,
    Text(Part),
    Logo(Part),
    LogoPosition(Part),
    LogoHeight(Part),
    RunColor(Part),
    RunSize(Part),
    RunFont(Part),
    Bold(Part),
    Rule(Part),
}

/// Colour fields in `Colors`, in form order.
const COLORS: [&str; 7] =
    ["Text", "Secondary text", "Muted text", "Accent text", "Accent tint", "Border", "Subtle fill"];

#[derive(Debug, Clone)]
pub enum EditorMsg {
    Text(Field, String),
    Toggle(Field, bool),
    Choose(Field, &'static str),
    BrowseLogo(Part),
    LogoPicked(Part, Option<PathBuf>),
    SaveTo(Scope),
    Save,
    /// Opens the name prompt for a copy.
    SaveAs,
    SaveAsName(String),
    SaveAsCancel,
    SaveAsConfirm,
}

/// The name prompt's input, focused when it opens.
pub const SAVE_AS_INPUT: &str = "export-save-as";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    Global,
}

#[derive(Debug, Clone)]
pub struct TemplateEditor {
    pub draft: Template,
    /// Raw text of number and colour inputs while they don't parse.
    inputs: HashMap<Field, String>,
    pub scope: Scope,
    pub error: Option<String>,
    /// The new name while "Save as…" is open.
    pub save_as: Option<String>,
}

impl TemplateEditor {
    pub fn new(template: &Template, project: bool) -> Self {
        Self {
            draft: template.clone(),
            inputs: HashMap::new(),
            scope: if project { Scope::Project } else { Scope::Global },
            error: None,
            save_as: None,
        }
    }

    /// The draft if every input parses and the template validates.
    pub fn valid(&self) -> Result<&Template, String> {
        if let Some((field, _)) = self.inputs.iter().find(|(f, v)| !self.parses(**f, v)) {
            return Err(format!("{} is not valid", field_label(*field)));
        }
        self.draft.to_toml().and_then(|src| Template::from_toml(&src)).map(|_| &self.draft).map_err(|e| e.to_string())
    }

    fn parses(&self, field: Field, value: &str) -> bool {
        match field {
            f if is_color(f) => value.trim().is_empty() && matches!(f, Field::RunColor(_)) || hex(value).is_some(),
            Field::Margin(_) | Field::BaseSize | Field::LineHeight | Field::LogoHeight(_) | Field::RunSize(_) => {
                number(value).is_some()
            }
            _ => true,
        }
    }

    fn running(&mut self, part: Part) -> &mut Running {
        match part {
            Part::Header => &mut self.draft.header,
            Part::Footer => &mut self.draft.footer,
        }
    }

    fn color_mut(&mut self, i: usize) -> &mut String {
        let c = &mut self.draft.colors;
        [
            &mut c.text,
            &mut c.text_secondary,
            &mut c.text_muted,
            &mut c.accent_text,
            &mut c.accent_soft,
            &mut c.border,
            &mut c.bg_subtle,
        ]
        .into_iter()
        .nth(i)
        .expect("colour index")
    }

    /// Apply one message; true when the draft changed (the preview must
    /// follow).
    pub fn update(&mut self, msg: &EditorMsg) -> bool {
        self.error = None;
        match msg {
            EditorMsg::Text(field, value) => {
                if self.parses(*field, value) {
                    self.inputs.remove(field);
                } else {
                    self.inputs.insert(*field, value.clone());
                    return false;
                }
                // Keep what was typed ("1." while typing 1.5) as the shown
                // text; the draft gets the parsed value.
                if matches!(
                    field,
                    Field::Margin(_) | Field::BaseSize | Field::LineHeight | Field::LogoHeight(_) | Field::RunSize(_)
                ) {
                    self.inputs.insert(*field, value.clone());
                }
                self.set_text(*field, value);
                true
            }
            EditorMsg::Toggle(field, on) => {
                let on = *on;
                match field {
                    Field::HeadingNumbers => self.draft.heading_numbers = on,
                    Field::TitlePage => self.draft.title_page = on,
                    Field::Toc => self.draft.toc = on,
                    Field::Lead => self.draft.lead = on,
                    Field::Bold(p) => self.running(*p).bold = on,
                    Field::Rule(p) => self.running(*p).rule = on,
                    _ => return false,
                }
                true
            }
            EditorMsg::Choose(field, value) => {
                let value = *value;
                match field {
                    Field::Style => {
                        self.draft.style = match value {
                            "guide" => Style::Guide,
                            "report" => Style::Report,
                            _ => Style::Plain,
                        }
                    }
                    Field::PageSize => self.draft.page_size = value.into(),
                    Field::FontBody => self.draft.font_body = value.into(),
                    Field::FontHeading => self.draft.font_heading = value.into(),
                    Field::FontMono => self.draft.font_mono = value.into(),
                    Field::NumberStyle => {
                        self.draft.number_style =
                            if value == "guide" { NumberStyle::Guide } else { NumberStyle::Decimal }
                    }
                    Field::CodeTheme => self.draft.code_theme = value.into(),
                    Field::LogoPosition(p) => {
                        self.running(*p).logo_position =
                            HAlign::ALL.into_iter().find(|a| a.as_str() == value).unwrap_or_default()
                    }
                    Field::RunFont(p) => self.running(*p).font = value.into(),
                    _ => return false,
                }
                true
            }
            EditorMsg::LogoPicked(part, Some(path)) => {
                self.inputs.remove(&Field::Logo(*part));
                self.running(*part).logo = path.to_string_lossy().into_owned();
                true
            }
            EditorMsg::SaveTo(scope) => {
                self.scope = *scope;
                false
            }
            EditorMsg::SaveAs => {
                self.save_as = Some(format!("{} copy", self.draft.name));
                self.error = None;
                false
            }
            EditorMsg::SaveAsName(name) => {
                self.save_as = Some(name.clone());
                self.error = None;
                false
            }
            EditorMsg::SaveAsCancel => {
                self.save_as = None;
                self.error = None;
                false
            }
            EditorMsg::LogoPicked(_, None) | EditorMsg::BrowseLogo(_) | EditorMsg::Save | EditorMsg::SaveAsConfirm => {
                false
            }
        }
    }

    fn set_text(&mut self, field: Field, value: &str) {
        let num = || number(value).unwrap_or(0.0);
        let color = || hex(value).unwrap_or_default();
        match field {
            Field::Name => self.draft.name = value.into(),
            Field::Margin(i) => self.draft.margins_mm[i] = num(),
            Field::BaseSize => self.draft.base_size_pt = num(),
            Field::LineHeight => self.draft.line_height = num(),
            Field::Accent => self.draft.accent = color(),
            Field::Color(i) => *self.color_mut(i) = color(),
            Field::Eyebrow => self.draft.eyebrow = value.into(),
            Field::Text(p) => self.running(p).text = value.into(),
            Field::Logo(p) => self.running(p).logo = value.trim().into(),
            Field::LogoHeight(p) => self.running(p).logo_height_mm = num(),
            Field::RunColor(p) => self.running(p).color = if value.trim().is_empty() { String::new() } else { color() },
            Field::RunSize(p) => self.running(p).size_pt = num(),
            _ => {}
        }
    }

    /// The text an input shows: what was typed, or the draft's value.
    fn shown(&self, field: Field) -> String {
        if let Some(v) = self.inputs.get(&field) {
            return v.clone();
        }
        let d = &self.draft;
        let r = |p: Part| match p {
            Part::Header => &d.header,
            Part::Footer => &d.footer,
        };
        match field {
            Field::Name => d.name.clone(),
            Field::Margin(i) => fmt(d.margins_mm[i]),
            Field::BaseSize => fmt(d.base_size_pt),
            Field::LineHeight => fmt(d.line_height),
            Field::Accent => d.accent.clone(),
            Field::Color(i) => {
                let c = &d.colors;
                [&c.text, &c.text_secondary, &c.text_muted, &c.accent_text, &c.accent_soft, &c.border, &c.bg_subtle][i]
                    .clone()
            }
            Field::Eyebrow => d.eyebrow.clone(),
            Field::Text(p) => r(p).text.clone(),
            Field::Logo(p) => r(p).logo.clone(),
            Field::LogoHeight(p) => fmt(r(p).logo_height_mm),
            Field::RunColor(p) => r(p).color.clone(),
            Field::RunSize(p) => {
                if r(p).size_pt > 0.0 {
                    fmt(r(p).size_pt)
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        }
    }

    fn invalid(&self, field: Field) -> bool {
        self.inputs.get(&field).is_some_and(|v| !self.parses(field, v))
    }
}

fn is_color(field: Field) -> bool {
    matches!(field, Field::Accent | Field::Color(_) | Field::RunColor(_))
}

fn number(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

/// `#RRGGBB` or `RRGGBB` → `#RRGGBB`.
fn hex(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    (h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| format!("#{}", h.to_ascii_uppercase()))
}

fn fmt(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn field_label(field: Field) -> String {
    let part = |p: Part| if p == Part::Header { "Header" } else { "Footer" };
    match field {
        Field::Margin(i) => format!("{} margin", ["Top", "Right", "Bottom", "Left"][i]),
        Field::BaseSize => "Text size".into(),
        Field::LineHeight => "Line height".into(),
        Field::Accent => "Accent".into(),
        Field::Color(i) => COLORS[i].into(),
        Field::LogoHeight(p) => format!("{} logo height", part(p)),
        Field::RunColor(p) => format!("{} colour", part(p)),
        Field::RunSize(p) => format!("{} size", part(p)),
        _ => format!("{field:?}"),
    }
}

/// Lowercase letters, digits and dashes, for the template's folder name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "template".into() } else { out }
}

/// Write `template` to `<templates_dir>/<slug of its name>/`: the TOML,
/// its logos (copied in when they live elsewhere), and `template.typ` and
/// `fonts/` when the template came from another folder. Returns the
/// folder.
pub fn save(template: &Template, templates_dir: &Path) -> Result<PathBuf, ExportError> {
    let dir = templates_dir.join(slug(&template.name));
    std::fs::create_dir_all(&dir)?;
    let mut out = template.clone();
    for part in [Part::Header, Part::Footer] {
        let r = match part {
            Part::Header => &template.header,
            Part::Footer => &template.footer,
        };
        let Some(src) = template.logo_path(r) else { continue };
        let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let dest = dir.join(&name);
        if src != dest {
            std::fs::copy(&src, &dest).map_err(|e| ExportError::Template(format!("logo {}: {e}", src.display())))?;
        }
        match part {
            Part::Header => out.header.logo = name,
            Part::Footer => out.footer.logo = name,
        }
    }
    if template.dir.as_deref() != Some(dir.as_path()) {
        if let Some(typ) = &template.typst_source {
            std::fs::write(dir.join("template.typ"), typ)?;
        }
        if let Some(fonts) = template.dir.as_ref().map(|d| d.join("fonts"))
            && let Ok(entries) = std::fs::read_dir(&fonts)
        {
            std::fs::create_dir_all(dir.join("fonts"))?;
            for entry in entries.flatten() {
                std::fs::copy(entry.path(), dir.join("fonts").join(entry.file_name()))?;
            }
        }
    }
    let tmp = dir.join("template.toml.part");
    std::fs::write(&tmp, out.to_toml()?)?;
    std::fs::rename(&tmp, dir.join("template.toml"))?;
    Ok(dir)
}

fn msg(m: EditorMsg) -> Message {
    Message::Export(ExportMsg::Editor(m))
}

/// The form. `project` enables the Project save target; `targets` are the
/// two folders shown under the save button.
pub fn view<'a>(t: &'a Theme, e: &'a TemplateEditor, project: Option<&'a Path>) -> Element<'a, Message> {
    let eyebrow = |s: &'a str| text(s).size(EYEBROW).font(bold()).color(c(t, T::TextMuted));
    let label = |s: &str| text(s.to_string()).size(UI_SMALL).color(c(t, T::TextSecondary));
    let input = |field: Field, placeholder: &'a str| -> Element<'a, Message> {
        let invalid = e.invalid(field);
        text_input(placeholder, &e.shown(field))
            .on_input(move |v| msg(EditorMsg::Text(field, v)))
            .size(UI)
            .padding(Padding::from([SPACE_1, SPACE_2]))
            .style(move |th, st| {
                let mut s = style::input(t)(th, st);
                if invalid {
                    s.border.color = c(t, T::Danger);
                }
                s
            })
            .into()
    };
    let labelled = |name: &str, w: Element<'a, Message>| -> Element<'a, Message> {
        column![label(name), w].spacing(SPACE_1).width(Length::Fill).into()
    };
    let segments = |field: Field, options: &[(&'static str, &'static str)], current: &str| -> Element<'a, Message> {
        let mut r = row![].spacing(2);
        for &(value, name) in options {
            let on = current.eq_ignore_ascii_case(value);
            r = r.push(
                button(text(name).size(UI_SMALL))
                    .padding(Padding::from([SPACE_1, SPACE_2]))
                    .style(style::btn(t, if on { Btn::Toggled } else { Btn::Secondary }))
                    .on_press(msg(EditorMsg::Choose(field, value))),
            );
        }
        r.wrap().into()
    };
    let check = |field: Field, on: bool, name: &str| -> Element<'a, Message> {
        checkbox(on)
            .label(name.to_string())
            .size(16)
            .text_size(UI)
            .spacing(SPACE_2)
            .style(checkbox_style(t))
            .on_toggle(move |v| msg(EditorMsg::Toggle(field, v)))
            .into()
    };
    // An empty colour shows what it falls back to.
    let swatch = |value: String, fallback: Option<iced::Color>| -> Element<'a, Message> {
        let color = hex(&value)
            .and_then(|h| {
                let v = u32::from_str_radix(&h[1..], 16).ok()?;
                Some(iced::Color::from_rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8))
            })
            .or(if value.trim().is_empty() { fallback } else { None });
        container(Space::new())
            .width(24)
            .height(24)
            .style(move |_| container::Style {
                background: color.map(Background::Color),
                border: Border { color: c(t, T::BorderStrong), width: 1.0, radius: RADIUS_SM.into() },
                ..container::Style::default()
            })
            .into()
    };
    let color_row = |field: Field, name: &str, placeholder: &'a str| -> Element<'a, Message> {
        labelled(
            name,
            row![
                swatch(e.shown(field), matches!(field, Field::RunColor(_)).then(|| c(t, T::TextMuted))),
                input(field, placeholder)
            ]
            .spacing(SPACE_2)
            .align_y(Alignment::Center)
            .into(),
        )
    };
    let d = &e.draft;
    let fonts: Vec<(&'static str, &'static str)> = FONTS.iter().map(|f| (*f, *f)).collect();

    let style_name = match d.style {
        Style::Guide => "guide",
        Style::Report => "report",
        Style::Plain => "plain",
    };
    let general = column![
        eyebrow("TEMPLATE"),
        labelled("Name", input(Field::Name, "Template name")),
        labelled(
            "Look",
            segments(Field::Style, &[("guide", "Guide"), ("report", "Report"), ("plain", "Plain")], style_name)
        ),
        labelled("Page size", segments(Field::PageSize, &PAGE_SIZES, &d.page_size)),
        labelled(
            "Margins (mm): top, right, bottom, left",
            row![
                input(Field::Margin(0), "top"),
                input(Field::Margin(1), "right"),
                input(Field::Margin(2), "bottom"),
                input(Field::Margin(3), "left"),
            ]
            .spacing(SPACE_1)
            .into(),
        ),
    ]
    .spacing(SPACE_2);

    let mut heading_fonts = vec![("", "Body")];
    heading_fonts.extend(fonts.iter().copied());
    let type_ = column![
        eyebrow("TYPE"),
        labelled("Body font", segments(Field::FontBody, &fonts, &d.font_body)),
        labelled("Heading font", segments(Field::FontHeading, &heading_fonts, &d.font_heading)),
        labelled("Code font", segments(Field::FontMono, &fonts, &d.font_mono)),
        row![
            labelled("Text size (pt)", input(Field::BaseSize, "11")),
            labelled("Line height", input(Field::LineHeight, "1.5")),
        ]
        .spacing(SPACE_2),
        labelled(
            "Code colours",
            segments(Field::CodeTheme, &[("ember-light", "Light"), ("ember-dark", "Dark")], &d.code_theme)
        ),
    ]
    .spacing(SPACE_2);

    let mut colors = column![eyebrow("COLOURS"), color_row(Field::Accent, "Accent", "#D9542B")].spacing(SPACE_2);
    for (i, name) in COLORS.iter().enumerate() {
        colors = colors.push(color_row(Field::Color(i), name, "#RRGGBB"));
    }

    let number_style = if d.number_style == NumberStyle::Guide { "guide" } else { "decimal" };
    let headings = column![
        eyebrow("TITLE AND HEADINGS"),
        check(Field::HeadingNumbers, d.heading_numbers, "Number headings"),
        labelled("Numbers", segments(Field::NumberStyle, &[("decimal", "1.1"), ("guide", "01")], number_style)),
        check(Field::TitlePage, d.title_page, "Title page"),
        check(Field::Toc, d.toc, "Table of contents"),
        check(Field::Lead, d.lead, "First paragraph as lead"),
        labelled("Eyebrow above the title", input(Field::Eyebrow, "e.g. {date}")),
    ]
    .spacing(SPACE_2);

    let running = |part: Part, title: &'a str| -> Element<'a, Message> {
        let r = if part == Part::Header { &d.header } else { &d.footer };
        let mut run_fonts = vec![("", "Body")];
        run_fonts.extend(fonts.iter().copied());
        let mut logo = column![labelled(
            "Logo",
            row![
                input(Field::Logo(part), "PNG, JPEG or SVG file"),
                button(text("Browse…").size(UI_SMALL))
                    .padding(Padding::from([SPACE_1, SPACE_2]))
                    .style(style::btn(t, Btn::Secondary))
                    .on_press(msg(EditorMsg::BrowseLogo(part))),
            ]
            .spacing(SPACE_1)
            .align_y(Alignment::Center)
            .into(),
        )]
        .spacing(SPACE_1);
        if d.logo_path(r).is_some_and(|p| !p.is_file()) {
            logo = logo.push(text("File not found; the logo is left out").size(UI_SMALL).color(c(t, T::Danger)));
        }
        column![
            eyebrow(title),
            labelled("Text: left | centre | right", input(Field::Text(part), "{title} | | {page} / {pages}")),
            text("Placeholders: {title} {date} {page} {pages}").size(UI_SMALL).color(c(t, T::TextMuted)),
            logo,
            row![
                labelled(
                    "Logo position",
                    segments(
                        Field::LogoPosition(part),
                        &[("left", "Left"), ("center", "Centre"), ("right", "Right")],
                        r.logo_position.as_str()
                    )
                ),
                labelled("Logo height (mm)", input(Field::LogoHeight(part), "8")),
            ]
            .spacing(SPACE_2),
            row![
                color_row(Field::RunColor(part), "Colour", "muted text"),
                labelled("Size (pt)", input(Field::RunSize(part), "auto")),
            ]
            .spacing(SPACE_2),
            labelled("Font", segments(Field::RunFont(part), &run_fonts, &r.font)),
            row![check(Field::Bold(part), r.bold, "Bold"), check(Field::Rule(part), r.rule, "Rule")].spacing(SPACE_4),
        ]
        .spacing(SPACE_2)
        .into()
    };

    let mut save = column![eyebrow("SAVE")].spacing(SPACE_2);
    let scopes = row![
        button(text("Project").size(UI_SMALL))
            .padding(Padding::from([SPACE_1, SPACE_2]))
            .style(style::btn(t, if e.scope == Scope::Project { Btn::Toggled } else { Btn::Secondary }))
            .on_press_maybe(project.map(|_| msg(EditorMsg::SaveTo(Scope::Project)))),
        button(text("All documents").size(UI_SMALL))
            .padding(Padding::from([SPACE_1, SPACE_2]))
            .style(style::btn(t, if e.scope == Scope::Global { Btn::Toggled } else { Btn::Secondary }))
            .on_press(msg(EditorMsg::SaveTo(Scope::Global))),
    ]
    .spacing(2);
    save = save.push(scopes);
    if project.is_none() {
        save = save.push(text("Open a project to save templates with it.").size(UI_SMALL).color(c(t, T::TextMuted)));
    }
    if let Some(err) = &e.error {
        save = save.push(text(err.clone()).size(UI_SMALL).color(c(t, T::Danger)));
    }

    scrollable(
        column![
            general,
            type_,
            colors,
            headings,
            running(Part::Header, "HEADER"),
            running(Part::Footer, "FOOTER"),
            save
        ]
        .spacing(SPACE_5)
        .padding(Padding { top: 0.0, right: SPACE_3, bottom: SPACE_2, left: 0.0 }),
    )
    .height(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_as_prompt() {
        let mut e = TemplateEditor::new(&Template::builtin("guide").unwrap(), false);
        assert!(!e.update(&EditorMsg::SaveAs));
        assert_eq!(e.save_as.as_deref(), Some("Guide copy"));
        e.update(&EditorMsg::SaveAsName("Acme".into()));
        assert_eq!(e.save_as.as_deref(), Some("Acme"));
        e.update(&EditorMsg::SaveAsCancel);
        assert_eq!(e.save_as, None);
    }

    #[test]
    fn edits_apply_when_valid() {
        let mut e = TemplateEditor::new(&Template::builtin("guide").unwrap(), false);
        assert!(e.update(&EditorMsg::Text(Field::BaseSize, "12.5".into())));
        assert_eq!(e.draft.base_size_pt, 12.5);
        // Half-typed numbers and colours stay in the input only.
        assert!(!e.update(&EditorMsg::Text(Field::Accent, "#12".into())));
        assert_eq!(e.draft.accent, "#D9542B");
        assert!(e.valid().is_err());
        assert!(e.update(&EditorMsg::Text(Field::Accent, "112233".into())));
        assert_eq!(e.draft.accent, "#112233");
        assert!(e.valid().is_ok());
        assert!(e.update(&EditorMsg::Choose(Field::LogoPosition(Part::Footer), "right")));
        assert_eq!(e.draft.footer.logo_position, HAlign::Right);
        assert!(e.update(&EditorMsg::Text(Field::RunColor(Part::Header), "".into())));
        assert_eq!(e.draft.header.color, "");
        assert!(e.update(&EditorMsg::Toggle(Field::Rule(Part::Header), true)));
        assert!(e.draft.header.rule);
    }

    #[test]
    fn save_copies_logo_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let logo = tmp.path().join("brand.svg");
        std::fs::write(&logo, "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>").unwrap();
        let mut t = Template::builtin("report").unwrap();
        t.name = "Acme Report!".into();
        t.header.logo = logo.to_string_lossy().into_owned();
        let dir = save(&t, &tmp.path().join("templates")).unwrap();
        assert_eq!(dir.file_name().unwrap(), "acme-report");
        assert!(dir.join("brand.svg").is_file());
        let back = Template::load_dir(&dir).unwrap();
        assert_eq!(back.header.logo, "brand.svg");
        assert_eq!(back.logo_path(&back.header), Some(dir.join("brand.svg")));
        assert_eq!(back.name, "Acme Report!");
        // Saving the loaded template again (same folder) keeps working.
        assert_eq!(save(&back, &tmp.path().join("templates")).unwrap(), dir);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Guide"), "guide");
        assert_eq!(slug("  Acme  Report (v2) "), "acme-report-v2");
        assert_eq!(slug("!!!"), "template");
    }
}
