# Changelog

## v0.3.1 (2026-11-07)

### Features

- **Ctrl+mouse wheel zooms** the document: one notch is 10 %, from 50 %
  to 300 %, the same steps as Ctrl+= and Ctrl+-. Touchpad scrolling with
  Ctrl held zooms too. Ctrl+0 resets. The zoom level is saved and shown in
  the status bar when it is not 100 %. In the Mermaid editor window
  Ctrl+wheel keeps zooming the canvas.

### Fixes

- **Edit mode follows the zoom:** Ctrl+= and Ctrl+- used to scale only
  View, Read and the Split preview; the Edit source text, code and line
  numbers now scale with them.

## v0.3.0 (2026-11-03)

### Features

- **Visual Mermaid editor:** hover a rendered diagram in View, Split or
  Read mode and click the pencil button to edit it in its own window.
  Flowcharts get a canvas: drag nodes, double-click empty space to add a
  node, drag a node's dot (or Shift+drag) to connect it to another node or
  to a new one, double-click to rename, Delete to remove. The side panel
  sets the label, one of 14 node shapes, the flowchart direction, and a
  connection's label, line (solid, dotted, thick, hidden) and ends. A live
  preview and the editable source sit next to the canvas, with undo and
  redo inside the window.
- **Your source stays yours:** each change rewrites only the statements
  it touches, so comments, `classDef`/`style` lines, spacing and the way
  you write arrows survive. Save (Ctrl+Enter) puts the code back between
  the fences as one step that Ctrl+Z undoes in the document; Ctrl+S saves
  and keeps editing. Closing with unsaved changes asks first.
- Other diagram types (sequence, class, state, pie, ...) open as source
  with a live preview.

### Known limitations

- Node positions on the canvas are only for editing: Mermaid has no
  syntax for them, so the rendered diagram is always laid out
  automatically, and positions are not kept after the window closes.
- Subgraphs are shown and kept but not edited on the canvas (new nodes go
  outside them); `linkStyle` indexes are not renumbered when an edge is
  deleted; `A@{ shape: ... }` nodes are kept but shown as rectangles.
- Changing one edge of a grouped statement (`A & B --> C`) splits that
  statement into one line per edge.
- Expanding a folder in the file tree still reads it on the UI thread, so a
  very slow folder can pause the window briefly.
- Only Linux is built and tested; macOS and Windows have no release builds.
- Math is not rendered yet.
- Git: no branch merge or conflict resolution, pull is fast-forward only,
  and push needs a `--features push` build (the .deb is built without it).
- No settings dialog: settings, keymap and themes are TOML files.

## v0.2.3 (2026-11-02)

### Features

- **Export preview:** the export dialog shows the rendered PDF pages next
  to the options and updates them as you change the template or options.
  DOCX exports show the same layout, with a note that Word may differ
  slightly.
- **Header and footer styling:** a template's header and footer can now
  carry a logo (PNG, JPEG or SVG; left, centre or right; height in mm), a
  colour, a font size, a font, bold text and a thin rule. This works in
  both PDF and DOCX. The plain `header = "left | centre | right"` string
  still works.
- **Template editor:** "Edit template…" in the export dialog edits page
  size, margins, fonts, colours, heading options, header and footer with a
  live preview. Save writes the template to the project or to all
  documents and copies its logo next to it; Save as… writes a copy under
  a new name.

### Known limitations

- The preview always shows the PDF layout, also for DOCX exports.
- In DOCX the header logo sits on the text baseline rather than centred.
- Expanding a folder in the file tree still reads it on the UI thread, so a
  very slow folder can pause the window briefly.
- Only Linux is built and tested; macOS and Windows have no release builds.
- Math is not rendered yet.
- Git: no branch merge or conflict resolution, pull is fast-forward only,
  and push needs a `--features push` build (the .deb is built without it).
- No settings dialog: settings, keymap and themes are TOML files.

## v0.2.2 (2026-11-02)

### Features

- **Tab menu:** right-click a tab for Close, Close Others, Close All and
  Close Tabs to the Left / Right. Saved tabs close at once; each tab with
  unsaved changes asks in turn, and Cancel stops the rest.
- **All-tabs list:** when the tabs don't fit, a chevron at the right end of
  the tab bar lists every tab with its folder (the active one ticked,
  unsaved ones dotted). Picking one, or switching to a tab that is scrolled
  out of view, scrolls the tab bar to it.

### Known limitations

- Tab widths are estimated from the title length, so the all-tabs button
  can appear slightly before the tabs actually overflow.
- Expanding a folder in the file tree still reads it on the UI thread, so a
  very slow folder can pause the window briefly.
- Only Linux is built and tested; macOS and Windows have no release builds.
- Math is not rendered yet.
- Git: no branch merge or conflict resolution, pull is fast-forward only,
  and push needs a `--features push` build (the .deb is built without it).
- No settings dialog: settings, keymap and themes are TOML files.

## v0.2.1 (2026-11-02)

### Fixes

- **Slow folders no longer freeze the window:** opening a project (from the
  menu, Recent projects or at start-up) reads it and sets up the folder
  watch in the background. When that takes more than a second, for example
  on a Google Drive or network mount, the Files panel shows a spinner with
  Cancel and the status bar says "opening project…"; you can keep editing
  meanwhile. Recent-project lists no longer touch the disk while drawing.

### Known limitations

- Expanding a folder in the file tree still reads it on the UI thread, so a
  very slow folder can pause the window briefly.
- Only Linux is built and tested; macOS and Windows have no release builds.
- Math is not rendered yet.
- Git: no branch merge or conflict resolution, pull is fast-forward only,
  and push needs a `--features push` build (the .deb is built without it).
- No settings dialog: settings, keymap and themes are TOML files.

## v0.2.0 (2026-11-02)

### Features

- **Mermaid diagrams:** ```` ```mermaid ```` blocks render as diagrams in
  View, Split and Read modes, drawn in the theme's colours and UI font by a
  pure-Rust renderer on a background worker. While you edit a diagram block
  you see its source and the diagram together, and the last good diagram
  stays up while a half-typed edit is invalid. PDF and DOCX exports include
  the diagrams (sharp 2x PNG in DOCX); a source that doesn't render stays a
  code block. Insert > Mermaid Diagram and `/mermaid` add a starter block.
- **Find and replace bar:** Ctrl+F opens a find bar above the document,
  prefilled from a single-line selection; Ctrl+H adds a replace row. Matches
  are highlighted in every mode with an "n of N" count. Enter / Shift+Enter
  and F3 / Shift+F3 step through matches, Replace goes one match at a time,
  and Replace All is a single undo step (with `$1` groups in regex mode).
  Match case, whole word and regex toggles (Alt+C / Alt+W / Alt+X) are shared
  with the sidebar search.

### Known limitations

- Only Linux is built and tested; macOS and Windows have no release builds.
- Math is not rendered yet.
- Mermaid covers the diagram types the bundled renderer supports; others
  stay as code blocks. The first render seeds a font cache in
  `~/.cache/mmdr`.
- The first F3 right after switching mode can select match 1 again.
- Git: no branch merge or conflict resolution, pull is fast-forward only,
  and push needs a `--features push` build (the .deb is built without it).
- No settings dialog: settings, keymap and themes are TOML files.
- First frame on a GPU takes about 400 ms (Vulkan start-up).
- Installing the .deb sets the markdown file icon for the whole system.

## v0.1.2 (2026-11-02)

### Features

- **Close Project:** File > Close Project (also the x in the project header
  and the palette) saves and closes the project's tabs, commits pending
  changes and leaves project mode, so mdedit goes back to editing standalone
  files.
- **Recent projects:** the last 10 project folders are listed in the empty
  Files panel and under File > Open Recent Project…. Opening another project
  closes the current one first.
- **Files from outside a project:** starting mdedit with files that are not
  inside the last project (for example opening an `.md` from the file
  manager) opens them on their own instead of restoring that project.

### Known limitations

- Same as v0.1.1.

## v0.1.1 (2026-11-01)

Packaging fix release.

### Fixes

- **Markdown file icon on Linux:** the .deb now installs the file icon as
  `mdedit-markdown` and its MIME definition points `text/markdown` at that
  name. Before, it used `text-markdown`, which icon themes like Ubuntu's Yaru
  already provide, so their icon hid ours and `.md` files kept the theme's
  icon even though they opened with mdedit.

### Known limitations

- Same as v0.1.0. Also: installing mdedit replaces the markdown file icon for
  every app on the system, since file icons belong to the MIME type, not to
  the default application. File managers that are already running (Nautilus)
  may need a restart to pick it up.

## v0.1.0 (2026-11-01)

First release of mdedit, a fast, lightweight markdown editor written in Rust
on iced. Linux is the only platform built and tested so far; `make deb`
produces a Debian package.

### Features

- **Four modes:** Edit (source with markdown highlighting), Split (source and
  preview with scroll sync and a draggable divider), View (WYSIWYG, Typora
  style: the block under the caret reveals its source) and Read (read-only
  view that still selects and copies). Ctrl+1..4 switch modes.
- **Fast core:** rope buffer with undo/redo and a capped edit log; virtualised
  editor and view widgets that lay out only visible lines and blocks;
  incremental block parsing (window reparse, about 12 µs per keystroke on a
  10 MB file) with large windows offloaded to a worker.
- **Large files:** memory-mapped piece tree above 20 MB (configurable) with a
  background line indexer, streaming save and streamed literal/regex search
  and replace-all. A 1 GB file opens in under a millisecond and searches in
  about half a second (docs/PERF.md).
- **Markdown:** CommonMark 0.31.2 (all 652 spec examples) plus GFM tables,
  task lists, strikethrough, footnotes (numbered in definition order) and
  GitHub alert callouts. Images render in View mode.
- **Editing commands:** formatting toggles, headings, lists, quotes, code,
  links, smart Enter/Tab list and quote continuation, pair wrapping, URL paste
  onto a selection as a link, CSV/TSV paste as a table, table row/column
  operations with auto-alignment.
- **App shell:** menu bar, tabs, sidebar with outline and file tree, status
  bar, command palette, slash menu (`/` at line start), floating format
  toolbar on selection, right-click menu, custom title bar, configurable
  keymap (keymap.toml), settings and session restore, external change
  detection.
- **Comments:** comment on a selection in any mode (Ctrl+Alt+M), anchored by
  quote and context in a hidden per-file TOML sidecar, re-anchored after
  external edits, undo restores detached comments; comments panel
  (Ctrl+Shift+M).
- **Projects and git:** open a folder as a project; file tree with moves and
  renames that rewrite links (including raw HTML `<img>`); images inserted,
  dropped or pasted go to `assets/`; debounced autocommit on save, named
  checkpoints that squash autocommits, branch create/switch, file history
  with diffs, sync (fast-forward pull; push with the optional `push` build
  feature).
- **Export:** PDF (Typst, embedded fonts) and DOCX with the Guide, Report and
  Plain templates or custom ones; table of contents, heading numbers, page
  size, margins, syntax-highlighted code and comments as footnotes
  (Ctrl+Shift+E).
- **Look:** Ember Light and Ember Dark themes plus user TOML themes, bundled
  Hanken Grotesk, JetBrains Mono and Source Serif 4 fonts, Lucide icons, app
  and markdown file icons.
- **Linux packaging:** `.desktop` entry, hicolor icons, `text/markdown` MIME
  definition; `make deb` builds `target/debian/mdedit_<version>-1_<arch>.deb`.

### Known limitations

- Only Linux is built and tested. macOS and Windows have icon and bundle
  stubs but no release builds yet.
- Math and Mermaid diagrams are not rendered yet (planned for v1.x).
- Git: no branch merge or conflict resolution (v1.x); pull is fast-forward
  only. Push needs a build with `--features push` (the .deb is built without
  it, so Sync only pulls).
- No settings dialog: settings, keymap and themes are edited as TOML files.
- First frame on a GPU takes about 400 ms (Vulkan device start-up), above the
  300 ms open target; the tiny-skia backend (`ICED_BACKEND=tiny-skia`) starts
  faster.
- Edit mode can show the wrong weight when bold starts inside an ASCII word
  (a cosmic-text shaping quirk already worked around in View mode).
- The .deb declares only libc as a hard dependency; the Vulkan, EGL and
  xkbcommon libraries winit and wgpu load at run time are Recommends.
