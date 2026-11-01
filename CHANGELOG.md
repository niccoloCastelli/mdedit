# Changelog

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
