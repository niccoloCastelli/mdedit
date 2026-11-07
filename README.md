<p align="center">
  <img src="assets/icons/png/mdedit-128.png" width="96" height="96" alt="mdedit icon">
</p>

<h1 align="center">mdedit</h1>

<p align="center">
  A fast, lightweight, cross-platform Markdown editor written in Rust on <a href="https://iced.rs">iced</a>.
</p>

mdedit opens a 1 GB file in under a millisecond, types at one frame of
latency on a 10 MB document, and saves plain Markdown that round-trips
byte for byte. It has four modes (source, split, WYSIWYG and read-only),
comments that live outside the Markdown, git-backed projects, Mermaid
diagrams and PDF/DOCX export, all in pure Rust with no external binaries
at run time.

Current version: **0.3.0** (see [CHANGELOG.md](CHANGELOG.md)). Linux is the
only platform built and tested so far.

## Features

- **Four modes:** Edit (Markdown source with syntax styling), Split (source
  and preview with synced scroll), View (WYSIWYG in the Typora style: the
  block under the caret reveals its markers) and Read (no editing, but you
  can still select and copy). Cursor and selection survive mode switches.
- **Fast core:** rope buffer, virtualised editor and view widgets that lay
  out only what is visible, and incremental block parsing (about 12 µs per
  keystroke on a 10 MB file).
- **Large files:** above 20 MB (configurable) a file is memory-mapped with a
  background line index, streaming save, and streamed text/regex search and
  replace-all. Numbers are in [docs/PERF.md](docs/PERF.md).
- **Markdown:** CommonMark 0.31.2 (all 652 spec examples pass) plus GFM
  tables, task lists, strikethrough, footnotes, heading attributes and
  GitHub alert callouts. Images render in View mode. Opening and saving
  without edits produces an identical file.
- **Mermaid:** ```` ```mermaid ```` blocks render as diagrams in View, Split
  and Read modes and in exports. Hover a diagram and click the pencil to
  edit it in its own window; flowcharts get a visual canvas (drag nodes,
  connect, rename, 14 node shapes) that rewrites only the statements you
  touch, so your comments and formatting in the source survive.
- **Editing:** menu bar, command palette, slash menu (`/` at line start),
  floating format toolbar and right-click menu, all backed by one command
  registry. Smart Enter/Tab for lists, quotes and tables; table insert,
  row/column operations and auto-alignment; CSV/TSV paste as a table; a URL
  pasted onto a selection becomes a link.
- **Find and replace:** a find bar with match case, whole word and regex
  (with `$1` groups), plus project-wide search in the sidebar.
- **Comments:** comment on any selection, in any mode. Comments are stored in
  a hidden TOML sidecar next to the file (`.name.md.toml`), anchored by quote
  and context, and re-anchored after outside edits. The Markdown stays
  clean.
- **Projects and git:** open a folder as a project for a file tree, link-safe
  moves and renames, images copied to `assets/` (insert, drop or paste),
  debounced autocommit on save, named checkpoints that squash autocommits,
  branches, file history with diffs, and sync (fast-forward pull; push with
  the optional `push` build feature).
- **Export:** PDF (Typst as a library, fonts embedded) and DOCX, with the
  Guide, Report and Plain templates or your own. Live preview, a template
  editor, header/footer logos, table of contents, heading numbers,
  syntax-highlighted code and comments as footnotes.
- **Look:** Ember Light and Ember Dark themes plus your own TOML themes,
  bundled Hanken Grotesk, JetBrains Mono and Source Serif 4 fonts, and
  Lucide icons.
- **Everyday:** tabs with a tab menu, outline, word count and reading time,
  session restore, and external change detection.

## Build and run

You need a recent stable Rust toolchain (the workspace uses edition 2024).
On Linux the app talks to the GPU through Vulkan (or GL) and uses the XDG
desktop portal for file dialogs, so no GTK headers are needed.

```bash
cargo run --release
```

To open files directly, pass them as arguments:

```bash
cargo run --release -- notes.md
```

The `Makefile` wraps the usual tasks:

| Command | What it does |
|---|---|
| `make` / `make build` | Debug build of the app |
| `make release` | Release build (`target/release/mdedit`) |
| `make check` | The merge gate: `fmt`, `clippy -D warnings` and `test --workspace` |
| `make deb` | Release build plus `target/debian/mdedit_<version>-1_<arch>.deb` (needs `cargo install cargo-deb`) |
| `make clean` | `cargo clean` |

The `.deb` installs the binary, a desktop entry, the app and Markdown file
icons and a `text/markdown` MIME definition. It is built without the `push`
feature. To enable pushing from the Git panel, build with libgit2:

```bash
cargo build --release -p mdedit-app --features push
```

Useful environment variables:

| Variable | Effect |
|---|---|
| `MDEDIT_CONFIG_DIR` | Keep settings, keymap, session and themes in one directory (handy for testing) |
| `ICED_BACKEND=tiny-skia` | Software renderer; starts faster than Vulkan |
| `MDEDIT_TRACE=1` | Print start-up and frame timings |
| `MDEDIT_DEBUG_KEYS=1` | Print every key chord the app receives |

## Keyboard shortcuts

`Ctrl` is `Cmd` on macOS. Every shortcut can be changed in `keymap.toml`.

| Shortcut | Action |
|---|---|
| `Ctrl+1` / `2` / `3` / `4` | Edit / Split / View / Read mode |
| `Ctrl+E` | Cycle modes |
| `Ctrl+N`, `Ctrl+O`, `Ctrl+S`, `Ctrl+Shift+S` | New, open, save, save as |
| `Ctrl+W`, `Ctrl+Tab`, `Ctrl+Shift+Tab` | Close tab, next tab, previous tab |
| `Ctrl+P` | Go to file |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+F`, `Ctrl+H`, `F3` / `Shift+F3` | Find, replace, next / previous match |
| `Alt+C` / `Alt+W` / `Alt+X` | Match case / whole word / regex |
| `Ctrl+Shift+F` | Find in files |
| `Ctrl+Z`, `Ctrl+Shift+Z` | Undo, redo |
| `Ctrl+B`, `Ctrl+I`, `Ctrl+Shift+X`, `` Ctrl+` `` | Bold, italic, strikethrough, inline code |
| `Ctrl+K` | Link |
| `Ctrl+Alt+0` … `Ctrl+Alt+6` | Paragraph, heading 1 to 6 |
| `Ctrl+Shift+8` / `7` / `9` | Bullet, numbered, task list |
| `Ctrl+Shift+.` | Quote |
| `Ctrl+Alt+C`, `Ctrl+Alt+T`, `Ctrl+Alt+I` | Code block, table, image |
| `Ctrl+Shift+V` | Paste as table |
| `Ctrl+Alt+M`, `Ctrl+Shift+M` | Add comment, toggle comments panel |
| `Ctrl+Shift+B`, `Ctrl+Shift+O`, `Ctrl+Shift+G` | Toggle sidebar, outline, Git panel |
| `Ctrl+Alt+S`, `Ctrl+Alt+H` | Git checkpoint, file history |
| `Ctrl+Shift+E` | Export as PDF |
| `Ctrl+=`, `Ctrl+-`, `Ctrl+0`, `Ctrl`+wheel | Zoom in, out, reset; zoom with the mouse wheel |
| `Ctrl+,` | Open the settings file |

In the Mermaid editor window, `Ctrl+Enter` saves the diagram back into the
document and closes the window, and `Ctrl+S` saves and keeps editing.

## Configuration

Settings, keymap, session and user themes live in the platform config
directory (`~/.config/mdedit` on Linux), as TOML files. There is no settings
dialog yet; `Ctrl+,` opens `settings.toml` in the editor.
[docs/example-theme.toml](docs/example-theme.toml) is a starting point for a
theme.

A project is any folder with a `.mdedit/` directory. Its settings are in
`.mdedit/project.toml`, and its export templates in `.mdedit/templates/`.
See [docs/SPEC.md](docs/SPEC.md) section 6 for the layout and options.

## Project layout

```
crates/
  mdedit-core/     buffer, edit log, document, block index, parser, search, commands
  mdedit-render/   layout model shared by the app and export (no GPU code)
  mdedit-mermaid/  Mermaid to themed SVG/PNG, flowchart source model
  mdedit-project/  project.toml, file tree, assets, watcher
  mdedit-sidecar/  sidecar TOML, comments, anchoring
  mdedit-git/      autocommit, checkpoints, branches, sync (gix; libgit2 only for push)
  mdedit-export/   AST to Typst to PDF, AST to DOCX, templates
  mdedit-theme/    themes, design tokens, bundled fonts and icons
  mdedit-editor/   Edit-mode widget (iced)
  mdedit-view/     View/Split widget (iced)
  mdedit-app/      the application shell (binary: mdedit)
assets/            fonts, icons and built-in themes, compiled into the binary
docs/              spec, architecture, design system, performance, plan
packaging/         Linux desktop/MIME/icon files, macOS Info.plist stub
scripts/           icon generation
```

Only `mdedit-app`, `mdedit-editor`, `mdedit-view` and `mdedit-theme` depend
on iced; the core, project, git, sidecar and export crates are UI-free.

## Documentation

- [docs/SPEC.md](docs/SPEC.md): what the editor does and the decisions behind it
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): how the code is structured
- [docs/DESIGN_SYSTEM.md](docs/DESIGN_SYSTEM.md): visual and interaction rules
- [docs/PERF.md](docs/PERF.md): measured performance and how to reproduce it
- [docs/PLAN.md](docs/PLAN.md): workstreams and merge rules
- [packaging/README.md](packaging/README.md): icons and per-platform packaging
- [assets/README.md](assets/README.md): bundled fonts and icons with their licences

## Status and known gaps

mdedit is in active development. The main gaps in 0.3.1:

- Only Linux is built and tested. macOS and Windows have icon and bundle
  stubs but no release builds.
- Math (`$...$`, `$$...$$`) is not rendered yet; it shows as code.
- Git has no branch merge or conflict resolution, pull is fast-forward
  only, and push needs a `push` feature build.
- No settings dialog: settings, keymap and themes are TOML files.
- The first frame on a GPU takes about 400 ms (Vulkan start-up), above the
  300 ms target; `ICED_BACKEND=tiny-skia` starts faster.
- On the Mermaid canvas, node positions are for editing only (Mermaid has no
  syntax for them), and subgraphs are kept but not edited.
- Expanding a folder in the file tree still reads it on the UI thread, so a
  very slow folder can pause the window briefly.

Next up are math, branch merging, and the other platforms. Collaborative
editing and an MCP server are planned for v2 (SPEC section 11).

## License

Licensed under either of MIT or Apache-2.0, at your option. Bundled fonts
and icons keep their own licences (OFL-1.1 and ISC), listed in
[assets/README.md](assets/README.md).
