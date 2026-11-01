# Bundled assets

Compiled into the binary by `mdedit-theme` (`include_bytes!`), so rendering
is identical on every platform and exports can embed the same fonts. Each
component keeps its licence file next to it; `mdedit_theme::assets::LICENSES`
exposes the texts for an About dialog.

| Path | Component | Version | Source | Licence |
|---|---|---|---|---|
| `fonts/hanken-grotesk/` | Hanken Grotesk: Regular, Italic, Medium, SemiBold, Bold, Bold Italic | 3.014 | https://github.com/marcologous/hanken-grotesk (`fonts/ttf`) | OFL-1.1, `OFL.txt` |
| `fonts/jetbrains-mono/` | JetBrains Mono: Regular, Italic, Bold, Bold Italic | 2.304 | https://github.com/JetBrains/JetBrainsMono/releases/tag/v2.304 | OFL-1.1, `OFL.txt` |
| `fonts/source-serif-4/` | Source Serif 4 (text optical size): Regular, Italic, Semibold, Bold, Bold Italic | 4.005 | https://github.com/adobe-fonts/source-serif/releases/tag/4.005R (Desktop TTF) | OFL-1.1, `LICENSE.md` |
| `icons/lucide/` | Lucide icons, a subset (see `icons/update.py`) | 1.66.0 | https://github.com/lucide-icons/lucide/releases/tag/1.66.0 | ISC, `icons/LICENSE` |
| `themes/` | Built-in themes Ember Light and Ember Dark | - | this repo (DESIGN_SYSTEM 2.2-2.4) | project licence |

Static TTFs are used rather than variable fonts because iced's text stack
picks faces by weight from separate files most reliably.

## Changing assets

- **Icons:** add the Lucide name (or alias) to `ICONS` in `icons/update.py`,
  download `lucide-icons-<version>.zip` from the Lucide releases page, unzip
  it, and run `python3 -I assets/icons/update.py <unzipped dir>`. The script
  sets the stroke to 1.5 px (DESIGN_SYSTEM 5) and regenerates
  `crates/mdedit-theme/src/icon_list.rs`.
- **Fonts:** replace the files from the upstream release and update
  `FONT_FACES` in `crates/mdedit-theme/src/assets.rs` and the table above.
