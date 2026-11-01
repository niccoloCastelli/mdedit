# mdedit specification

A cross-platform, fast and lightweight markdown editor written in Rust.

Status: draft v0.1 (2026-11-01). Visual and interaction rules are in
[DESIGN_SYSTEM.md](DESIGN_SYSTEM.md); code structure is in
[ARCHITECTURE.md](ARCHITECTURE.md). Items marked **Open** need a decision.

## 1. Goals and non-goals

**Goals**

- Low memory use and instant startup. Targets: cold start under 300 ms to
  first frame, under 120 MB RSS with a 1 MB document open (most of that is the
  GPU renderer baseline), typing latency under 8 ms at p99 on a 10 MB file.
- Only the visible lines are laid out and drawn; parsing is incremental.
- Linux, macOS and Windows as first-class targets. Linux is the first test
  platform.
- Pure Rust dependencies where possible (no pandoc, no libgit2 if avoidable,
  no external binaries at runtime).

**Non-goals for v1**

- Collaborative editing and the MCP server. They are the next phase (section
  11), but v1 must not block them.
- Plugins, multi-cursor, spell check (spell check is a strong v1.x candidate).

## 2. Releases

| Milestone | Scope |
|---|---|
| **v1** | Everything in sections 3-10 unless marked *later* |
| **v1.x** | Math (`$...$`, `$$...$$`), Mermaid diagrams, branch merge and conflict resolution |
| **v2** | Collaborative mode, MCP server |

## 3. Documents and modes

- **Edit** mode: markdown source with syntax styling.
- **Split** mode: source and a read-only rendered preview side by side, with
  synced scroll.
- **View** mode: WYSIWYG. The document is rendered and directly editable; the
  block under the cursor reveals its markdown markers. Saving always writes
  plain markdown; View mode never stores anything the source can't express
  (comments go to the sidecar, section 7).
- Cursor and selection are kept across mode switches.
- Multiple documents open in tabs. Files can be opened outside a project
  ("loose files"); project features (git, assets, sidecars, export templates
  from the project file) need project mode, except comments, which work on
  loose files too.
- Encoding: UTF-8 (with or without BOM, preserved). Line endings are detected
  and preserved (LF or CRLF). Other encodings open read-only with a banner.
- External changes: files are watched (`notify` crate). A clean buffer
  reloads silently; a dirty buffer shows a banner offering reload / keep mine /
  diff.

## 4. Markdown support

- **Base:** CommonMark 0.31 via `pulldown-cmark`.
- **Enabled extensions in v1:** tables, task lists, strikethrough, footnotes,
  heading attributes (`{#id}`). Tables are not CommonMark (they come from
  GitHub Flavored Markdown), but table editing needs them, so they are
  enabled along with the other GFM features `pulldown-cmark` supports
  natively.
- **Later (v1.x), designed for now:** math (`pulldown-cmark`'s
  `ENABLE_MATH` gives inline/display math events; rendering via a pure-Rust
  TeX-to-shapes layer, see ARCHITECTURE 4.4) and Mermaid (fenced
  ```` ```mermaid ```` blocks). Until supported, both render as code blocks.
  The block renderer is a trait so these plug in without touching the editor.
- Raw HTML is shown as source in View mode (not rendered as a web page).
  Inline `<br>`, `<sup>`, `<sub>`, `<kbd>` are rendered.
- Round-trip rule: opening and saving a file without edits must produce a
  byte-identical file. Edits only rewrite the bytes they touch (no
  reformatting of the rest of the document).

## 5. Editing features

- Menu bar, floating selection toolbar, slash menu and command palette; one
  command registry backs all of them (DESIGN_SYSTEM 8.2).
- Tables: insert with size picker, grid editing in View mode, auto-aligned
  source in Edit mode, row/column operations, alignment, CSV/TSV paste.
- Undo/redo with grouping (consecutive typing groups by word / 1 s pause),
  unlimited within memory budget, survives mode switches. Not persisted
  across restarts in v1.
- Find and replace (Ctrl+F / Ctrl+H) with plain text, case, whole word and
  regex options; project-wide search in the sidebar (Ctrl+Shift+F).
- Outline from headings, click to jump.
- Word / character count, reading time.
- Session restore: open tabs, cursor positions and scroll on restart.

## 6. Project mode

A **project** is a directory containing a `.mdedit/` folder. Opening any
directory offers to make it a project.

### 6.1 Layout on disk

```
my-notes/
├── .mdedit/
│   ├── project.toml        # project settings (6.2)
│   └── templates/          # export templates (10)
├── .git/
├── intro.md
├── .intro.md.toml          # sidecar for intro.md (7)
├── assets/                 # images for files in this directory (6.4)
│   └── diagram-20261101-1530.png
└── chapter-1/
    ├── draft.md
    ├── .draft.md.toml
    └── assets/
```

Project settings live in `.mdedit/project.toml` (decided 2026-11-01), so
templates and future project data have a home next to it.

### 6.2 project.toml

```toml
[project]
name = "My notes"
default_branch = "main"

[git]
autocommit = true
autocommit_debounce_secs = 60       # commit after this long with no new save
author_name = "Niccolo"             # falls back to git config
author_email = ""
remote = "origin"
pull_on_open = true

[files]
large_file_threshold_mb = 20        # overrides the app-wide default
assets_dir = "assets"

[export]
default_template = "report"
```

### 6.3 Git versioning

- **Autocommit on save, debounced.** Every save restarts a timer
  (`autocommit_debounce_secs`, default 60 s); when it fires, all changed files
  are committed in one commit `auto: <files>`. Quitting the app flushes
  pending autocommits.
- **Checkpoints.** A named checkpoint (Ctrl+Alt+S or Git panel) squashes every
  autocommit since the previous checkpoint into one commit with the given
  message. Checkpoints can optionally carry a **version tag** (annotated tag,
  e.g. `v1.2` or `draft-3`).
- **Autocommits are local until checkpointed.** Squashing rewrites history, so
  autocommits are never pushed. Sync pushes up to the last checkpoint only; if
  there are autocommits when you press Sync, the app asks for a checkpoint
  name (with a default `Sync <date time>`) first. (Decided 2026-11-01.)
- **Branches (v1):** create from current, switch, rename, delete (with
  confirmation). Switching with unsaved changes saves and autocommits first.
  The branch name is always visible in the status bar.
- **Later (v1.x):** merge, conflict resolution with the diff view, cherry-pick
  of checkpoints.
- **History:** the Git panel lists checkpoints (and autocommits collapsed under
  them), with diff view and "restore this version" (which creates a new
  commit, never rewrites).
- Ignored by default: nothing is added to `.gitignore` automatically except
  `.mdedit/cache/`. Sidecars and assets are versioned.

### 6.4 Assets

- Inserting an image (dialog, drag and drop, or paste) in project mode copies
  it into `assets/` next to the markdown file and inserts a relative link
  `![](assets/name.png)`.
- Names: the original file name, sanitised, with `-2`, `-3` on collision;
  pasted images get `pasted-YYYYMMDD-HHMMSS.png`.
- Identical files (same SHA-256) are reused instead of duplicated.
- Renaming or moving a markdown file inside the app moves its referenced
  assets and sidecar and rewrites the links.
- Outside project mode, images are linked where they are (absolute or
  relative path), not copied.

### 6.5 Remote projects

- "Open remote project" clones a URL (HTTPS or SSH) into a chosen folder.
- On open: fetch + fast-forward pull if the working tree is clean; otherwise a
  banner asks to sync manually.
- Manual **Sync** button: checkpoint if needed, pull (fast-forward or, in
  v1, refuse and explain when it would need a merge), push.
- Push needs the libgit2 backend (ARCHITECTURE 7). When it is not available
  the editor works normally and Sync only pulls; the push action is shown
  disabled with a tooltip explaining why.
- Credentials: SSH agent and SSH keys in `~/.ssh`; HTTPS via the git
  credential helper the system already has, and the OS keychain (`keyring`
  crate) for tokens entered in the app. The app never stores credentials in
  project files.

## 7. Sidecar file (comments and extensions)

Each markdown file `name.md` may have a hidden sidecar `.name.md.toml` in the
same directory, created only when needed and deleted when empty. It holds
non-standard data so the markdown stays portable.

```toml
version = 1

[[comment]]
id = "c_01JC8Z3Q"
author = "Niccolo"
created = 2026-11-01T15:30:00Z
resolved = false
body = "Is this figure still right after the October change?"

  [comment.anchor]
  quote = "3–5 business days"         # exact text the comment is attached to
  prefix = "In the recipient's currency, "   # up to 32 chars before
  suffix = "\n"                       # up to 32 chars after
  hint = 4812                         # last known byte offset, speeds lookup

  [[comment.reply]]
  author = "Niccolo"
  created = 2026-11-01T15:42:00Z
  body = "Yes, checked with ops."
```

**Re-anchoring** (on open and after external changes): exact match of
`prefix + quote + suffix` nearest to `hint`; then exact `quote` nearest to
`hint`; then fuzzy match (bitap / Levenshtein ≤ 10 % of the quote length)
scored by context similarity. If nothing scores above the threshold the
comment becomes **detached** and is shown in a separate group; it is never
silently deleted. While the document is open, anchors move with edits
(positions are updated by the edit log), so re-anchoring is only needed for
changes made outside the app.

Future non-standard features (per-document export overrides, block IDs for
collaboration, custom attributes) go in new top-level tables in the same
file, guarded by `version`.

## 8. Large files

- Files above the threshold (default **20 MB**, configurable app-wide and per
  project) open in **large-file mode**: the file is memory-mapped, a line
  index is built in the background, and only the visible region is read.
  Typing is possible immediately; the scrollbar becomes exact once indexing
  finishes (it shows progress).
- Full editing is supported. Edits are stored as a piece table over the
  mapped file; saving streams to a temp file and atomically replaces the
  original.
- Preview, Split and View modes are disabled; syntax styling is limited to the
  visible lines (line-local styling, no full parse). Outline is built lazily
  from a background scan for headings.
- Search: plain text and regex over the whole file, streamed in chunks on a
  background thread with results appearing as they are found; replace-all is
  supported and recorded as one undo step.
- Memory target: under 50 MB of overhead for a 1 GB file before edits.
- If the file changes on disk while mapped, the app detects it (size / mtime /
  watcher), stops reading the map, and offers reload. **Open:** on Windows the
  mapped file can't be replaced by other programs while open; acceptable?

## 9. Settings

App settings in `settings.toml` in the platform config dir: appearance
(system/light/dark), light and dark theme, fonts and sizes, editor defaults
(mode, line numbers, measure, monospace edit), autosave, large-file
threshold, keymap overrides, default author. A settings dialog edits the same
file; changes apply live.

## 10. Export

- Formats: **DOCX** and **PDF**, from the menu (File → Export) or the palette.
  Export runs in the background and never blocks editing.
- **Pure Rust pipeline:**
  - PDF: markdown AST → Typst markup → `typst` compiler used as a library →
    `typst-pdf`. Fonts are the bundled ones plus any the template names.
  - DOCX: markdown AST → `docx-rs` document with named styles.
- **Templates** live in `.mdedit/templates/<name>/` (project) or the app config
  dir (global). A template is:
  - `template.toml`: page size, margins, fonts, sizes, colours, heading
    numbering, header/footer text (with `{title}`, `{page}`, `{date}`
    placeholders), title page on/off, table of contents on/off, code theme.
    These options map to both outputs.
  - optional `template.typ`: full Typst template for PDF for complete control.
  - optional `reference.docx`: styles copied into the generated DOCX.
    **Open:** `docx-rs` can read documents, but copying styles from a reference
    file needs some work on our side; v1 may ship with TOML-driven DOCX styles
    only.
- **Formatting options** in the export dialog override the template for one
  export (page size, margins, TOC, numbering, include comments as
  footnotes/margin notes or not).
- Built-in templates: **Guide** (the look of the reference PDF: eyebrow,
  numbered sections, rounded tables, callouts), **Report**, **Plain**.
- Images, tables, footnotes, task lists and code highlighting are exported;
  math and Mermaid once supported.

## 11. Next phase (v2), constraints for v1

- **Collaborative mode.** Requirements TBD. v1 keeps all edits as explicit
  operations (insert/delete at a char offset, with a transaction id) through
  one edit log, so a CRDT (e.g. `loro` or `yrs`) can sit behind the same
  interface later. Comment anchors already survive concurrent edits by quote +
  context.
- **MCP server.** Requirements TBD. The core (documents, project, git,
  sidecar, export) is a UI-free library crate with a command API, so an MCP
  server (stdio and/or HTTP) can expose the same commands an agent would need:
  read, search, edit ranges, comment, checkpoint, export.

## 12. Open questions summary

Decided on 2026-11-01: `.mdedit/project.toml`, local-only autocommits with
checkpoint-only push, GFM tables, Hanken Grotesk + JetBrains Mono, libgit2
only for push and optional.

1. Windows: large files are locked while mapped; acceptable?
2. DOCX templates: TOML styles only in v1, `reference.docx` later?
3. Design system questions in DESIGN_SYSTEM section 9.
