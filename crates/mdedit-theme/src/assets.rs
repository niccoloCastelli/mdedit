//! Bundled fonts and icons (DESIGN_SYSTEM 3.1 and 5), compiled into the
//! binary so rendering is identical everywhere and exports can embed them.
//!
//! Files live in the repo's `assets/` dir with their licence texts; see
//! `assets/README.md` for versions and sources.

#[path = "icon_list.rs"]
mod icon_list;

/// One bundled font file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontFace {
    /// Family name as stored in the font, e.g. `"Hanken Grotesk"`.
    pub family: &'static str,
    /// CSS weight, 400 regular to 700 bold.
    pub weight: u16,
    pub italic: bool,
    pub bytes: &'static [u8],
}

macro_rules! faces {
    ($($family:literal, $dir:literal, $file:literal, $weight:literal, $italic:literal;)*) => {
        &[$(FontFace {
            family: $family,
            weight: $weight,
            italic: $italic,
            bytes: include_bytes!(concat!("../../../assets/fonts/", $dir, "/", $file)),
        }),*]
    };
}

/// Every bundled face. Weights match the type scale (DESIGN_SYSTEM 3.2):
/// 400, 500, 600 and 700 for the UI/prose family, fewer for the others.
pub const FONT_FACES: &[FontFace] = faces! {
    "Hanken Grotesk", "hanken-grotesk", "HankenGrotesk-Regular.ttf", 400, false;
    "Hanken Grotesk", "hanken-grotesk", "HankenGrotesk-Italic.ttf", 400, true;
    "Hanken Grotesk", "hanken-grotesk", "HankenGrotesk-Medium.ttf", 500, false;
    "Hanken Grotesk", "hanken-grotesk", "HankenGrotesk-SemiBold.ttf", 600, false;
    "Hanken Grotesk", "hanken-grotesk", "HankenGrotesk-Bold.ttf", 700, false;
    "Hanken Grotesk", "hanken-grotesk", "HankenGrotesk-BoldItalic.ttf", 700, true;
    "JetBrains Mono", "jetbrains-mono", "JetBrainsMono-Regular.ttf", 400, false;
    "JetBrains Mono", "jetbrains-mono", "JetBrainsMono-Italic.ttf", 400, true;
    "JetBrains Mono", "jetbrains-mono", "JetBrainsMono-Bold.ttf", 700, false;
    "JetBrains Mono", "jetbrains-mono", "JetBrainsMono-BoldItalic.ttf", 700, true;
    "Source Serif 4", "source-serif-4", "SourceSerif4-Regular.ttf", 400, false;
    "Source Serif 4", "source-serif-4", "SourceSerif4-It.ttf", 400, true;
    "Source Serif 4", "source-serif-4", "SourceSerif4-Semibold.ttf", 600, false;
    "Source Serif 4", "source-serif-4", "SourceSerif4-Bold.ttf", 700, false;
    "Source Serif 4", "source-serif-4", "SourceSerif4-BoldIt.ttf", 700, true;
};

/// The bytes of every bundled font, to load at startup (iced:
/// `Settings::fonts` or `iced::font::load`; Typst: its font book).
pub fn fonts() -> impl Iterator<Item = &'static [u8]> {
    FONT_FACES.iter().map(|f| f.bytes)
}

/// A Lucide icon as SVG bytes (24×24 viewBox, `currentColor` stroke at
/// 1.5 px), by its Lucide name or alias, e.g. `"bold"`, `"git-branch"`.
/// `None` if the icon is not bundled; add it in `assets/icons/update.py`.
pub fn icon(name: &str) -> Option<&'static [u8]> {
    let icons = icon_list::ICONS;
    icons.binary_search_by(|(n, _)| (*n).cmp(name)).ok().map(|i| icons[i].1)
}

/// Names [`icon`] accepts, sorted.
pub fn icon_names() -> impl Iterator<Item = &'static str> {
    icon_list::ICONS.iter().map(|(n, _)| *n)
}

/// A bundled third-party component and its licence, for an About dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct License {
    pub component: &'static str,
    /// SPDX identifier.
    pub spdx: &'static str,
    pub text: &'static str,
}

pub const LICENSES: &[License] = &[
    License {
        component: "Hanken Grotesk",
        spdx: "OFL-1.1",
        text: include_str!("../../../assets/fonts/hanken-grotesk/OFL.txt"),
    },
    License {
        component: "JetBrains Mono",
        spdx: "OFL-1.1",
        text: include_str!("../../../assets/fonts/jetbrains-mono/OFL.txt"),
    },
    License {
        component: "Source Serif 4",
        spdx: "OFL-1.1",
        text: include_str!("../../../assets/fonts/source-serif-4/LICENSE.md"),
    },
    License { component: "Lucide icons", spdx: "ISC", text: include_str!("../../../assets/icons/LICENSE") },
];

#[cfg(feature = "iced")]
mod iced_support {
    use std::borrow::Cow;

    /// [`super::fonts`] in the form `iced::Settings::fonts` takes.
    pub fn iced_fonts() -> Vec<Cow<'static, [u8]>> {
        super::fonts().map(Cow::Borrowed).collect()
    }

    /// An iced SVG handle for [`super::icon`]. Tint it with the `svg`
    /// widget's style (`text-secondary`, `text` on hover, `accent` active).
    pub fn icon_handle(name: &str) -> Option<iced::widget::svg::Handle> {
        super::icon(name).map(iced::widget::svg::Handle::from_memory)
    }
}

#[cfg(feature = "iced")]
pub use iced_support::{iced_fonts, icon_handle};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_are_truetype_with_expected_names() {
        for face in FONT_FACES {
            assert_eq!(&face.bytes[..4], &[0, 1, 0, 0], "{} {} is not a TrueType font", face.family, face.weight);
            // The family name is stored in the `name` table as UTF-16BE.
            let utf16: Vec<u8> = face.family.encode_utf16().flat_map(u16::to_be_bytes).collect();
            assert!(face.bytes.windows(utf16.len()).any(|w| w == utf16.as_slice()), "{} not named", face.family);
        }
        assert_eq!(fonts().count(), FONT_FACES.len());
    }

    #[test]
    fn icons_resolve_by_name_and_alias() {
        let names: Vec<_> = icon_names().collect();
        assert!(names.windows(2).all(|w| w[0] < w[1]), "icon list must be sorted for binary search");
        for name in ["bold", "italic", "git-branch", "history", "trash-2", "settings", "x"] {
            let svg = std::str::from_utf8(icon(name).unwrap_or_else(|| panic!("missing {name}"))).unwrap();
            assert!(svg.contains("<svg") && svg.contains(r#"stroke-width="1.5""#), "{name}");
        }
        assert_eq!(icon("no-such-icon"), None);
    }

    #[test]
    fn licences_are_bundled() {
        assert!(LICENSES.iter().filter(|l| l.spdx == "OFL-1.1").all(|l| l.text.contains("SIL Open Font License")));
        assert!(LICENSES.iter().any(|l| l.spdx == "ISC" && l.text.contains("ISC License")));
    }
}
