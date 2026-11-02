//! Mermaid diagrams (```` ```mermaid ```` fenced blocks, SPEC 4) as SVG, in
//! pure Rust (ARCHITECTURE 4.4).
//!
//! [`render`] lays a diagram out with `mermaid-rs-renderer`, coloured from an
//! mdedit [`Theme`] ([`Style`]), then runs the SVG through `usvg` with the
//! bundled fonts so every label becomes a path. The result draws the same in
//! iced's SVG widget, Typst (PDF export) and [`to_png`] (DOCX export), with
//! no font lookups left to the consumer.
//!
//! Rendering is synchronous and takes about 0.1-2 ms per diagram once warm;
//! the first call loads font metadata (tens of ms). Callers on a UI thread
//! run it on a worker and cache the result (mdedit-view does).

pub mod flowchart;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, OnceLock};

use mdedit_theme::{ColorToken, Rgba, Theme};
use mermaid_rs_renderer::{RenderOptions, Theme as MmdTheme};
use resvg::usvg::{self, fontdb};

/// Larger sources are not rendered (shown as code instead): layout cost
/// grows faster than linearly with the number of nodes.
pub const MAX_SOURCE: usize = 64 * 1024;

/// Font size of diagram labels (px), the `ui` size of DESIGN_SYSTEM 3.2.
const FONT_SIZE: f32 = 14.0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("diagram source is over {} KB", MAX_SOURCE / 1024)]
    TooLarge,
    #[error("{0}")]
    Syntax(String),
    #[error("could not draw the diagram: {0}")]
    Svg(String),
}

/// Whether a fenced code block's language is Mermaid.
pub fn is_mermaid(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("mermaid")
}

/// Diagram colours and font, from an mdedit theme.
#[derive(Debug, Clone)]
pub struct Style {
    theme: MmdTheme,
    key: u64,
}

impl Style {
    pub fn from_theme(theme: &Theme) -> Self {
        let c = |t: ColorToken| css(theme.color(t));
        let text = c(ColorToken::Text);
        let mut m = MmdTheme::modern();
        m.font_family = format!("{}, sans-serif", theme.fonts.ui);
        m.font_size = FONT_SIZE;
        // Transparent: the diagram sits on the page (or paper) colour.
        m.background = "none".into();
        m.primary_color = c(ColorToken::BgSubtle);
        m.primary_text_color = text.clone();
        m.primary_border_color = c(ColorToken::BorderStrong);
        m.line_color = c(ColorToken::TextSecondary);
        m.secondary_color = c(ColorToken::AccentSoft);
        m.tertiary_color = c(ColorToken::Surface);
        m.edge_label_background = c(ColorToken::Bg);
        m.cluster_background = c(ColorToken::Surface);
        m.cluster_border = c(ColorToken::Border);
        m.sequence_actor_fill = c(ColorToken::BgSubtle);
        m.sequence_actor_border = c(ColorToken::BorderStrong);
        m.sequence_actor_line = c(ColorToken::BorderStrong);
        m.sequence_note_fill = c(ColorToken::WarningBg);
        m.sequence_note_border = c(ColorToken::WarningText);
        m.sequence_activation_fill = c(ColorToken::AccentSoft);
        m.sequence_activation_border = c(ColorToken::Accent);
        m.text_color = text.clone();
        m.git_commit_label_color = text.clone();
        m.git_commit_label_background = c(ColorToken::BgSubtle);
        m.git_tag_label_color = text.clone();
        m.git_tag_label_background = c(ColorToken::AccentSoft);
        m.git_tag_label_border = c(ColorToken::Accent);
        let series = [
            ColorToken::Accent,
            ColorToken::Info,
            ColorToken::Success,
            ColorToken::WarningText,
            ColorToken::Danger,
            ColorToken::TextSecondary,
        ];
        for (i, slot) in m.pie_colors.iter_mut().enumerate() {
            let rgba = theme.color(series[i % series.len()]);
            // The second round is lighter so neighbours stay apart.
            *slot = if i < series.len() { css(rgba) } else { css(Rgba { a: 0x99, ..rgba }) };
        }
        m.pie_title_text_size = 18.0;
        m.pie_title_text_color = text.clone();
        m.pie_section_text_size = FONT_SIZE;
        // Small slices put their label outside, on the page.
        m.pie_section_text_color = text.clone();
        m.pie_legend_text_size = FONT_SIZE;
        m.pie_legend_text_color = text;
        m.pie_stroke_color = c(ColorToken::Bg);
        m.pie_outer_stroke_color = c(ColorToken::Border);
        m.pie_opacity = 1.0;

        let mut h = DefaultHasher::new();
        for t in ColorToken::ALL {
            theme.color(*t).hash(&mut h);
        }
        theme.fonts.ui.hash(&mut h);
        Self { theme: m, key: h.finish() }
    }

    /// Changes whenever the rendered output would.
    pub fn key(&self) -> u64 {
        self.key
    }
}

fn css(c: Rgba) -> String {
    if c.a == 255 {
        format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
    } else {
        format!("rgba({},{},{},{:.3})", c.r, c.g, c.b, c.a as f32 / 255.0)
    }
}

/// A rendered diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagram {
    /// SVG with text already converted to paths.
    pub svg: Vec<u8>,
    /// Natural size in CSS px.
    pub width: f32,
    pub height: f32,
}

/// Render Mermaid source to an SVG.
pub fn render(src: &str, style: &Style) -> Result<Diagram, Error> {
    if src.len() > MAX_SOURCE {
        return Err(Error::TooLarge);
    }
    if src.trim().is_empty() {
        return Err(Error::Syntax("empty diagram".into()));
    }
    seed_measure_font(&style.theme.font_family);
    let options = RenderOptions { theme: style.theme.clone(), ..RenderOptions::default() };
    let svg = mermaid_rs_renderer::render_with_options(src, options).map_err(|e| Error::Syntax(first_line(&e)))?;
    let tree = parse(svg.as_bytes())?;
    let size = tree.size();
    let flat = tree.to_string(&usvg::WriteOptions::default());
    Ok(Diagram { svg: flat.into_bytes(), width: size.width(), height: size.height() })
}

/// A node's box in a rendered diagram, top-left corner and size in px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Where the renderer puts each node of `src` (the visual editor starts
/// from the same layout as the rendered diagram, and sizes boxes to fit
/// their labels). `None` when the source doesn't parse.
pub fn node_boxes(src: &str, style: &Style) -> Option<HashMap<String, NodeBox>> {
    if src.len() > MAX_SOURCE || src.trim().is_empty() {
        return None;
    }
    seed_measure_font(&style.theme.font_family);
    let parsed = mermaid_rs_renderer::parse_mermaid_strict(src).ok()?;
    let options = RenderOptions { theme: style.theme.clone(), ..RenderOptions::default() };
    let layout = mermaid_rs_renderer::compute_layout(&parsed.graph, &options.theme, &options.layout);
    Some(
        layout
            .nodes
            .into_iter()
            .filter(|(_, n)| !n.hidden)
            .map(|(id, n)| (id, NodeBox { x: n.x, y: n.y, width: n.width, height: n.height }))
            .collect(),
    )
}

fn first_line(e: &dyn std::fmt::Display) -> String {
    let s = e.to_string();
    s.lines().next().unwrap_or_default().trim().to_string()
}

/// The diagram as a PNG at `scale` × its natural size (for formats without
/// SVG support).
pub fn to_png(diagram: &Diagram, scale: f32) -> Result<Vec<u8>, Error> {
    let tree = parse(&diagram.svg)?;
    let size = tree.size().to_int_size().scale_by(scale).ok_or_else(|| Error::Svg("empty size".into()))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| Error::Svg("diagram too large".into()))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| Error::Svg(e.to_string()))
}

/// Any SVG (a template logo) rasterised so its larger side is `max_px`;
/// returns the PNG and its pixel size.
pub fn svg_to_png(svg: &[u8], max_px: u32) -> Result<(Vec<u8>, u32, u32), Error> {
    let tree = parse(svg)?;
    let size = tree.size();
    let scale = max_px as f32 / size.width().max(size.height()).max(1.0);
    let px = size.to_int_size().scale_by(scale).ok_or_else(|| Error::Svg("empty size".into()))?;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(px.width(), px.height()).ok_or_else(|| Error::Svg("image too large".into()))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let png = pixmap.encode_png().map_err(|e| Error::Svg(e.to_string()))?;
    Ok((png, px.width(), px.height()))
}

fn parse(svg: &[u8]) -> Result<usvg::Tree, Error> {
    let options = usvg::Options { fontdb: fonts(), ..usvg::Options::default() };
    usvg::Tree::from_data(svg, &options).map_err(|e| Error::Svg(e.to_string()))
}

/// Bundled fonts first, then the system's for scripts they don't cover.
fn fonts() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    Arc::clone(DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        for bytes in mdedit_theme::assets::fonts() {
            db.load_font_data(bytes.to_vec());
        }
        let ui = Theme::default_for(mdedit_theme::ThemeKind::Light).fonts;
        db.set_sans_serif_family(ui.ui);
        db.set_monospace_family(ui.mono);
        Arc::new(db)
    }))
}

/// `mermaid-rs-renderer` measures labels with a font it finds among the
/// system fonts, and keeps a copy per family string in
/// `$XDG_CACHE_HOME/mmdr/font-cache` (read before searching). The bundled
/// UI font is not a system font, so put it in that cache: boxes then fit the
/// labels we draw. Mirrors the renderer's private `cache_paths` (0.3.1); if
/// that changes, labels are measured with a system sans-serif instead, which
/// only makes boxes a little loose.
fn seed_measure_font(family: &str) {
    static SEEDED: OnceLock<()> = OnceLock::new();
    SEEDED.get_or_init(|| {
        let Some(base) = std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        else {
            return;
        };
        let name = family.split(',').next().unwrap_or_default().trim();
        let Some(face) =
            mdedit_theme::assets::FONT_FACES.iter().find(|f| f.family == name && f.weight == 400 && !f.italic)
        else {
            return;
        };
        let key = format!("v2-font-family-case:{}", family.trim());
        let mut h = DefaultHasher::new();
        key.as_str().hash(&mut h);
        let hash = h.finish();
        let dir = base.join("mmdr").join("font-cache");
        let font = dir.join(format!("{hash:x}.font"));
        if std::fs::metadata(&font).is_ok_and(|m| m.len() == face.bytes.len() as u64) {
            return;
        }
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&font, face.bytes);
        let _ = std::fs::write(dir.join(format!("{hash:x}.meta")), "0");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> Style {
        Style::from_theme(&Theme::ember_light())
    }

    #[test]
    fn renders_common_diagrams_with_text_as_paths() {
        let samples = [
            "flowchart LR\n  A[Start] --> B{Is it?}\n  B -->|Yes| C[OK]\n  B -->|No| D[Rethink]",
            "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi\n  Note right of Bob: thinks",
            "classDiagram\n  Animal <|-- Duck\n  Animal : +int age",
            "stateDiagram-v2\n  [*] --> Still\n  Still --> [*]",
            "pie title Pets\n  \"Dogs\" : 386\n  \"Cats\" : 85",
            "erDiagram\n  CUSTOMER ||--o{ ORDER : places",
            "gantt\n  dateFormat YYYY-MM-DD\n  section A\n  Task :a1, 2024-01-01, 30d",
        ];
        for src in samples {
            let d = render(src, &style()).unwrap_or_else(|e| panic!("{src}: {e}"));
            let svg = String::from_utf8(d.svg.clone()).unwrap();
            assert!(!svg.contains("<text"), "{src}: text left in the SVG");
            assert!(d.width > 10.0 && d.height > 10.0, "{src}: {}x{}", d.width, d.height);
            let lower = svg.to_ascii_lowercase();
            assert!(lower.contains("#d9542b") || lower.contains("#1b1a19"), "{src}: theme colours missing");
        }
    }

    #[test]
    fn bad_source_is_a_one_line_error() {
        let Err(Error::Syntax(e)) = render("this is not mermaid", &style()) else { panic!() };
        assert!(!e.is_empty() && !e.contains('\n'));
        assert_eq!(render("  \n", &style()), Err(Error::Syntax("empty diagram".into())));
        assert_eq!(render(&"a".repeat(MAX_SOURCE + 1), &style()), Err(Error::TooLarge));
    }

    #[test]
    fn png_has_the_scaled_size() {
        let d = render("flowchart TD\n A --> B", &style()).unwrap();
        let png = to_png(&d, 2.0).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        assert_eq!(w, (d.width.ceil() as u32) * 2);
    }

    #[test]
    fn node_boxes_follow_the_layout() {
        let boxes = node_boxes("flowchart LR\n A[Start] --> B[A much longer label]", &style()).unwrap();
        let (a, b) = (boxes["A"], boxes["B"]);
        assert!(b.x > a.x + a.width - 1.0, "LR puts B right of A");
        assert!(b.width > a.width);
        assert!(node_boxes("not a diagram", &style()).is_none());
    }

    #[test]
    fn style_key_follows_the_theme() {
        assert_eq!(style().key(), style().key());
        assert_ne!(style().key(), Style::from_theme(&Theme::ember_dark()).key());
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(400))]
        /// The release profile aborts on panic, so the renderer must never
        /// panic on half-typed input.
        #[test]
        fn never_panics_on_edited_sources(
            base in 0usize..5,
            cut in 0usize..200,
            insert in "[-A-Za-z0-9 >|{}\\[\\]():;\"\n%-]{0,12}",
        ) {
            let sources = [
                "flowchart LR\n  A[Start] --> B{Is it?}\n  B -->|Yes| C[OK]\n  subgraph S\n  C --> D\n  end",
                "sequenceDiagram\n  participant A\n  A->>B: Hello\n  loop Every minute\n  B-->>A: Hi\n  end",
                "classDiagram\n  class Animal {\n    +int age\n    +eat()\n  }\n  Animal <|-- Duck",
                "stateDiagram-v2\n  [*] --> Still\n  state Moving {\n    [*] --> Fast\n  }",
                "pie title Pets\n  \"Dogs\" : 386\n  \"Cats\" : 85",
            ];
            let src = sources[base];
            let mut at = cut.min(src.len());
            while !src.is_char_boundary(at) {
                at -= 1;
            }
            let edited = format!("{}{}{}", &src[..at], insert, &src[at..]);
            let _ = render(&edited, &style());
            let _ = render(&src[..at], &style());
        }
    }
}
