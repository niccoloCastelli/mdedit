# mdedit design system

This document defines how mdedit looks and behaves: visual tokens, themes,
typography, components, and interaction rules. The code architecture lives in
[ARCHITECTURE.md](ARCHITECTURE.md) and the feature scope in [SPEC.md](SPEC.md).

Status: draft v0.1 (2026-11-01). Anything marked **Open** needs a decision
before the matching piece is implemented.

## 1. Principles

1. **The document is the interface.** Chrome (menus, panels, status bar) is
   quiet and low contrast; the text is the highest-contrast thing on screen.
2. **Legible first.** Reading comfort beats density: generous line height, a
   capped measure (line length), warm neutrals instead of pure black on white.
   The reference points are Typora's legible themes and the Magma guide PDF
   (`assets/offramp-guide.pdf`).
3. **One accent.** A single warm orange marks focus, selection handles, the
   primary action and section numbers. Everything else is neutral.
4. **Everything is a token.** No colour, size or duration is hard-coded in a
   widget. Widgets read from the active `Theme`, and themes are TOML files.
5. **Fast feels calm.** No animation longer than 150 ms, nothing animates
   while typing, and no layout shift when panels open or the parser catches up.
6. **Keyboard complete.** Every action is reachable from the keyboard; the
   mouse is a convenience.

## 2. Colour

Values were sampled from the reference PDF and then adjusted where a sampled
value failed WCAG AA contrast for body-size text (4.5:1). Contrast ratios are
against the theme's `bg` unless noted.

### 2.1 Semantic tokens

Widgets only ever use these names. Themes assign values to them.

| Token | Purpose |
|---|---|
| `bg` | Window and document background |
| `bg-subtle` | Sidebar, table header, inline code background |
| `surface` | Cards, popovers, menus, floating toolbar |
| `surface-raised` | Hovered menu item, active tab |
| `border` | 1 px dividers, card outlines, table rules |
| `border-strong` | Input outlines, focused-but-not-accented edges |
| `text` | Body text and headings |
| `text-secondary` | Lead paragraphs, UI labels, sidebar items |
| `text-muted` | Placeholders, line numbers, timestamps, captions |
| `accent` | Primary buttons, focus rings, cursor, section numbers |
| `accent-text` | Accent used as small text (links, eyebrows) |
| `accent-soft` | Selection background, badges, hovered accent items |
| `on-accent` | Text on an `accent` fill |
| `warning-bg` / `warning-text` | Callout boxes, large-file banner, unsynced state |
| `danger` | Destructive actions, merge conflicts, errors |
| `success` | Synced, saved, committed indicators |
| `info` | Neutral notices, links in dark theme if needed |
| `comment-highlight` | Background of text anchored by a comment |
| `syntax-*` | Markdown source styling (see 2.4) |

### 2.2 Ember Light (default light theme)

| Token | Value | Source / note |
|---|---|---|
| `bg` | `#FFFFFF` | PDF page |
| `bg-subtle` | `#F5F3F1` | PDF warm grey fills |
| `surface` | `#FFFFFF` | |
| `surface-raised` | `#FBFAFA` | PDF table rows |
| `border` | `#E3DFDA` | PDF card and table outlines |
| `border-strong` | `#C6C5C3` | |
| `text` | `#1B1A19` | PDF headings and body, 17.4:1 |
| `text-secondary` | `#5F5B57` | PDF lead paragraph, 6.7:1 |
| `text-muted` | `#78746F` | PDF uses `#969491` (3.0:1), darkened to 4.6:1 |
| `accent` | `#D9542B` | PDF eyebrow, numbers, primary button |
| `accent-text` | `#B8431F` | accent darkened for small text, 5.4:1 |
| `accent-soft` | `#FBE9E2` | PDF step badges |
| `on-accent` | `#FFFFFF` | 4.0:1, use only at ≥14 px semibold (UI buttons) |
| `warning-bg` | `#FBF0D9` | PDF callout |
| `warning-text` | `#8A5A00` | PDF callout title, 5.2:1 on `warning-bg` |
| `danger` | `#B42335` | kept distinct in hue from the orange accent |
| `success` | `#3F7D4E` | 4.9:1 |
| `info` | `#2F6FAE` | 5.2:1 |
| `comment-highlight` | `#FBF0D9` | same as warning-bg; the margin marker uses `warning-text` |

### 2.3 Ember Dark (default dark theme)

Sampled from the dark console screenshots inside the PDF.

| Token | Value | Note |
|---|---|---|
| `bg` | `#111111` | |
| `bg-subtle` | `#161514` | |
| `surface` | `#1B1A19` | |
| `surface-raised` | `#1E1E1E` | |
| `border` | `#302F2E` | |
| `border-strong` | `#444342` | |
| `text` | `#EAEAE9` | 15.7:1 |
| `text-secondary` | `#AEAEAD` | 8.5:1 |
| `text-muted` | `#8E8D8D` | 5.7:1 |
| `accent` | `#E86A43` | slightly lifted from `#D9542B` for dark bg, 5.9:1 |
| `accent-text` | `#FF8A5C` | 8.1:1 |
| `accent-soft` | `#3A2118` | |
| `on-accent` | `#111111` | 5.9:1 |
| `warning-bg` | `#2E2412` | |
| `warning-text` | `#D6A24A` | |
| `danger` | `#F0707F` | |
| `success` | `#6FBF86` | |
| `info` | `#7FB2E5` | |
| `comment-highlight` | `#3A2E14` | |

### 2.4 Markdown source styling (`syntax-*`)

Used in Edit mode and in the revealed source of the block under the cursor in
View mode. Markers are de-emphasised so the prose stays readable.

| Token | Applies to | Light | Dark |
|---|---|---|---|
| `syntax-marker` | `#`, `*`, `_`, `` ` ``, `>`, list bullets, fences, `|` | `text-muted` | `text-muted` |
| `syntax-heading` | heading text | `text`, bold | `text`, bold |
| `syntax-emphasis` | emphasis / strong text | `text` + style | `text` + style |
| `syntax-link` | link text | `accent-text` | `accent-text` |
| `syntax-url` | link / image destination | `text-muted` | `text-muted` |
| `syntax-code` | inline code and code blocks | `#5F5B57` on `bg-subtle` | `#D4D2CF` on `bg-subtle` |
| `syntax-html` | raw HTML | `info` | `info` |
| `syntax-math` | `$...$` (later) | `info` | `info` |

Code block highlighting inside fences uses `syntect` (or `tree-sitter`
grammars later) with a token map per theme; that map is part of the theme file.

### 2.5 Theme files

Themes are TOML files. Built-ins (Ember Light, Ember Dark) are compiled in;
user themes go in the platform config dir (`~/.config/mdedit/themes/` on Linux,
via the `directories` crate). A theme can `extends = "ember-light"` and
override only some tokens. Light/dark follows the OS by default
(`appearance = "system" | "light" | "dark"`), with one light and one dark theme
selected in settings.

```toml
name = "Ember Light"
kind = "light"            # light | dark

[color]
bg = "#FFFFFF"
text = "#1B1A19"
accent = "#D9542B"
# ...every token from 2.1

[font]
ui = "Hanken Grotesk"
prose = "Hanken Grotesk"  # a theme may switch prose to a serif
mono = "JetBrains Mono"

[syntax]
marker = "text-muted"     # may reference another token or a hex value
```

## 3. Typography

### 3.1 Families

These are the fonts the reference PDF uses (confirmed by Niccolo). All are
open-licence (OFL) and bundled in the binary, so rendering is identical on
every platform and in exports.

| Role | Family | Why |
|---|---|---|
| UI and prose (default) | **Hanken Grotesk** | Reference PDF font: tight headings, open body text |
| Monospace | **JetBrains Mono** | Reference PDF font for section numbers; good code legibility |
| Optional prose serif | **Source Serif 4** | For a Typora-style "book" reading theme |

### 3.2 Scale

Base size 16 px, ratio about 1.25, rounded to whole pixels. The user zoom
(Ctrl +/-) multiplies the whole scale; the UI chrome scale is separate.

| Token | Size / line height | Weight | Tracking | Use |
|---|---|---|---|---|
| `display` | 32 / 38 | 700 | -0.02em | H1 in View mode |
| `h2` | 26 / 32 | 700 | -0.015em | |
| `h3` | 21 / 28 | 600 | -0.01em | |
| `h4` | 18 / 26 | 600 | 0 | |
| `h5`, `h6` | 16 / 24 | 600 | 0 | H6 uses `text-secondary` |
| `body` | 16 / 27 (1.7) | 400 | 0 | Prose |
| `body-lead` | 19 / 30 | 400 | 0 | Optional first paragraph style (`text-secondary`) |
| `code` | 14 / 22 | 400 | 0 | Inline and block code, Edit mode source |
| `ui` | 13 / 18 | 400/500 | 0 | Menus, sidebar, buttons |
| `ui-small` | 12 / 16 | 400 | 0 | Status bar, captions |
| `eyebrow` | 11 / 16 | 600 | +0.08em, uppercase | Panel titles, table headers, callout titles |
| `number` | 13 / 18 | 400 mono | 0 | Section numbers, line numbers |

Edit mode uses `code` for the whole buffer when the monospace option is on and
`body` otherwise (default: proportional prose with monospace code blocks, like
iA Writer).

### 3.3 Measure

The document column has a max width of **720 px** (about 70 characters of
body text) and is centred. Tables, images and code blocks may extend to
**960 px** when the window allows. In Split mode each pane gets its own measure.
"Full width" is a toggle.

## 4. Space, shape, elevation

### 4.1 Spacing

4 px base. Tokens: `space-1` 4, `space-2` 8, `space-3` 12, `space-4` 16,
`space-5` 24, `space-6` 32, `space-7` 48, `space-8` 64.

| Where | Value |
|---|---|
| Paragraph gap | `space-4` (16) |
| Before H1/H2 | `space-7` (48); after: `space-3` |
| Before H3-H6 | `space-5` (24); after: `space-2` |
| List item gap | `space-1` |
| Card / callout padding | `space-4` × `space-5` |
| Document side padding | `space-6`, min `space-4` on narrow windows |
| Menu item | height 28, padding `space-2` × `space-3` |

### 4.2 Radius and borders

`radius-sm` 4 (inline code, badges), `radius-md` 8 (inputs, buttons, menus),
`radius-lg` 12 (cards, callouts, dialogs, table frame, images), `radius-full`
for step badges and avatars. Borders are 1 px `border`. The PDF style for
tables (outer rounded frame, horizontal rules only, uppercase `eyebrow`
header) is the default table style.

### 4.3 Elevation

Only floating things get shadows: menus, floating toolbar, command palette,
dialogs. One shadow token per theme: light `0 4 16 rgba(27,26,25,0.10)`, dark
`0 4 16 rgba(0,0,0,0.45)` plus a 1 px `border`. Panels and sidebars are flat
and separated by borders.

### 4.4 Motion

`motion-fast` 80 ms (hover, press), `motion-base` 150 ms (popover open,
panel slide). Ease-out. The cursor blinks at 530 ms and stops blinking while
typing. All motion is disabled when the OS asks for reduced motion.

## 5. Iconography

**Lucide** icons (ISC licence), drawn from SVG at 16 px in the UI and 14 px in
dense toolbars, stroke 1.5 px, coloured `text-secondary`, `text` on hover,
`accent` when active. Icons are bundled and rendered through iced's `svg`
widget. Every icon button has a tooltip with its shortcut.

## 6. Layout

```
┌───────────────────────────────────────────────────────────────────┐
│ Menu bar: File Edit Format Insert View Project Help               │
├────────┬──────────────────────────────────────────────┬───────────┤
│Sidebar │  Tab bar: doc.md ● | notes.md                │ Comments  │
│ Files  │──────────────────────────────────────────────│ panel     │
│ Outline│                                              │ (toggle)  │
│ Search │         document column (max 720)           │           │
│ Git    │                                              │           │
├────────┴──────────────────────────────────────────────┴───────────┤
│ Status: main ● saved · autocommit in 42s   Ln 12, Col 4   Edit ▾  │
└───────────────────────────────────────────────────────────────────┘
```

- **Menu bar.** iced has no native menu bar, so it is an in-window bar built
  from our own components (on macOS the first version also uses an in-window
  bar; a native menu via `muda` is a later option).
- **Sidebar** (240 px, resizable 180-400, collapsible with Ctrl+B) with tabs:
  Files (project mode), Outline (headings), Search, Git (branches, history,
  checkpoints).
- **Comments panel** (300 px, right, Ctrl+Alt+M) lists threads in document
  order; each card aligns with its anchor when there is room.
- **Status bar** (24 px): branch, save/sync state, autocommit countdown, cursor
  position, word count, mode switcher, large-file indicator.
- **Split mode** divides the editor area 50/50 (draggable) with synced scroll
  driven by the block index.

## 7. Components

Each component lists its states. All states map to tokens; none introduces a
new colour.

| Component | Anatomy and states |
|---|---|
| **Button** | Variants: primary (`accent` fill, `on-accent`), secondary (`surface`, 1 px `border`), ghost (no border, `surface-raised` on hover), danger. Height 28 (UI) / 32 (dialogs), `radius-md`. States: default, hover, pressed, focused (2 px `accent` ring offset 2), disabled (40 % opacity). |
| **Icon button** | 28×28 ghost button, tooltip after 500 ms. Toggled state uses `accent-soft` background and `accent` icon. |
| **Text input** | 1 px `border-strong`, `radius-md`, focus ring `accent`. Error: `danger` border + helper text. |
| **Menu / dropdown** | `surface`, shadow, `radius-md`, items 28 px, shortcut right-aligned in `text-muted`, separators `border`. Keyboard: arrows, type-ahead, Enter, Esc. |
| **Floating toolbar** | Appears 150 ms after a non-empty selection settles, 8 px above it (below if no room). Bold, italic, strike, code, link, heading ▾, comment. Hides on typing or Esc. |
| **Slash menu** | Opens on `/` at line start or after whitespace in View and Edit modes. Filterable list with icon, name, hint; max 8 visible rows. |
| **Command palette** | Ctrl+Shift+P. Centred, 560 px wide, fuzzy search over every command, shows shortcuts. Ctrl+P is the same palette scoped to files. |
| **Tabs** | Document tabs: 32 px, active has `bg` and a 2 px `accent` bottom line (as in the PDF's console tabs), dirty dot `●`. Middle click closes. |
| **Tree (files)** | 24 px rows, 12 px indent per level, chevron, hover `surface-raised`, selected `accent-soft`. Hidden sidecar files never shown. |
| **Callout / banner** | `radius-lg`, `eyebrow` title. Variants: info (`bg-subtle`), warning (`warning-bg`), danger. Used for the large-file banner, merge notices, sync errors. |
| **Toast** | Bottom-right, `surface` + shadow, auto-dismiss 4 s unless it has an action. |
| **Dialog** | Modal, 480 px, `radius-lg`, title + body + right-aligned actions (primary last). Esc cancels. |
| **Badge** | `radius-full` or `radius-sm`, `accent-soft` / `accent-text`; step numbers in the PDF style. |
| **Comment card** | Author, relative time (`text-muted`), body, reply box, resolve. Orphaned comments (anchor lost) are shown in a separate "Detached" group with a warning badge. |
| **Table editor** | See 8.4. |
| **Diff view** | Two-column or inline, `success`/`danger` tinted line backgrounds at 12 % alpha. Used for history and, later, conflicts. |

### 7.1 Rendered markdown (View mode, export defaults)

| Element | Style |
|---|---|
| Headings | Section 3.2 scale; an optional theme setting shows H2 numbers in `number` style and `accent` like the PDF (`01`, `02`) |
| Paragraph | `body`, `text` |
| Links | `accent-text`, underline on hover only |
| Blockquote | 3 px left bar `border-strong`, `text-secondary`, italic off |
| Inline code | `code`, `bg-subtle`, `radius-sm`, padding 2×4 |
| Code block | `bg-subtle`, `radius-lg`, padding `space-4`, language label `eyebrow` top-right, copy button on hover |
| Table | Rounded frame, horizontal rules, `eyebrow` header in `text-secondary`, cell padding 12×16 |
| Lists | Bullet `•` in `text-muted`; ordered numbers in `text-secondary`; task list checkbox 16 px `radius-sm` outlined in `accent` (PDF checklist style) |
| Horizontal rule | 1 px `border`, `space-6` vertical margin |
| Image | `radius-lg`, max width = measure, caption from title in `ui-small` `text-muted` |
| Footnotes | Superscript `accent-text`, list at the end in `ui` size |

## 8. Interaction rules

### 8.1 Modes

| Mode | What it is | Shortcut |
|---|---|---|
| **Edit** | Markdown source with syntax styling (2.4), line numbers optional | Ctrl+1 |
| **Split** | Source left, rendered preview right, synced scroll, preview read-only | Ctrl+2 |
| **View** | WYSIWYG: the document is rendered and editable. The block containing the cursor reveals its markdown markers (Typora's model), all other blocks stay rendered | Ctrl+3 |

Ctrl+E cycles modes. The cursor position and selection are preserved across
mode switches (mapped through the block index). Large files (above the
threshold) only allow Edit mode; the other options are disabled with a tooltip
explaining why.

### 8.2 Formatting commands

Every formatting action exists in three places with the same name and icon:
menu bar, floating toolbar / slash menu, keyboard shortcut. Commands are
toggles where it makes sense (Ctrl+B on bold text removes bold).

| Action | Shortcut | Slash |
|---|---|---|
| Bold / Italic / Strike / Inline code | Ctrl+B / Ctrl+I / Ctrl+Shift+X / Ctrl+` | |
| Link | Ctrl+K | `/link` |
| Heading 1-6 / Paragraph | Ctrl+Alt+1..6 / Ctrl+Alt+0 | `/h1`..`/h6` |
| Bullet / Numbered / Task list | Ctrl+Shift+8 / 7 / 9 | `/list`, `/numbered`, `/todo` |
| Quote | Ctrl+Shift+. | `/quote` |
| Code block | Ctrl+Alt+C | `/code` |
| Table | Ctrl+Alt+T | `/table` |
| Image | Ctrl+Alt+I | `/image` |
| Horizontal rule | | `/hr` |
| Comment | Ctrl+Alt+M | `/comment` |
| Math, Mermaid (later) | | `/math`, `/mermaid` |

On macOS Ctrl is Cmd and Alt is Option. Shortcuts are defined in one keymap
table and can be overridden in a `keymap.toml`.

### 8.3 Smart editing

- Enter in a list continues it; Enter on an empty item ends the list.
- Tab / Shift+Tab indent and outdent list items and table cells.
- Typing `**`, `` ` ``, `[` auto-pairs only when there is a selection (wraps it);
  no auto-pairing on bare typing, which annoys prose writers.
- Pasting a URL onto a selection makes a link. Pasting an image in project
  mode saves it to `assets/` (see SPEC).
- Typing markdown syntax in View mode converts the block live (`# ` makes a
  heading, `- ` a list, `> ` a quote, `| a | b |` + Enter a table).

### 8.4 Tables

- `/table` or Ctrl+Alt+T opens a size picker (grid hover, up to 10×10).
- In View mode a table is a grid: Tab / Shift+Tab move between cells, Enter in
  the last row adds a row, a hover toolbar on rows/columns offers insert
  above/below/left/right, delete, move, and column alignment.
- In Edit mode the source is **auto-aligned**: pipes are padded so columns line
  up (width measured in grapheme clusters, CJK counted as two columns) when
  leaving the table or on Ctrl+Alt+T inside it. Ctrl+Shift+V pastes CSV/TSV
  as a table.

### 8.5 Focus, selection, cursor

- Focus ring: 2 px `accent`, offset 2, on every focusable control.
- Cursor: 2 px wide bar, `accent`. Selection: `accent-soft`. Multi-cursor is a
  later option, not in v1.
- The search match currently selected uses `accent-soft` with a 1 px `accent`
  outline; other matches use `warning-bg`.

### 8.6 Feedback and state

- Save is automatic for project files (on idle, 1 s) and explicit (Ctrl+S)
  for loose files. The status bar shows `●` unsaved, `saved`, `committed`.
- Long operations (open large file, export, sync) show progress in the status
  bar and never block typing.
- Errors are toasts with a "Details" action; conflicts and sync failures are
  banners because they need action.

### 8.7 Accessibility

- All text tokens meet WCAG AA (4.5:1) against their intended background; this
  is checked by a unit test over every built-in theme.
- Full keyboard navigation, visible focus, reduced-motion support.
- Screen reader support depends on iced's accessibility work (AccessKit); we
  track it and expose names for every control so it can be wired when iced
  ships it.

## 9. Open questions

1. Should H2 section numbering in the PDF style (`01`, `02` in accent mono) be
   on by default in View mode and exports, or opt-in?
2. Edit mode default font: proportional (iA Writer style, proposed) or
   monospace?
