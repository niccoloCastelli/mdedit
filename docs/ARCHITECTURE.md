# mdedit architecture

How the code is organised to meet the goals in [SPEC.md](SPEC.md): low
memory, a rope buffer, only visible lines rendered, incremental parsing, and a
UI-free core that collaboration and MCP can reuse later.

Status: draft v0.1 (2026-11-01).

## 1. Workspace layout

The repo becomes a Cargo workspace. Only `mdedit-app` depends on iced.

```
crates/
  mdedit-core/      buffer, edit log, document, block index, parser, search, commands
  mdedit-render/    layout model shared by the app and export (no GPU code)
  mdedit-mermaid/   Mermaid → themed SVG (text as paths) / PNG, for view and export
  mdedit-project/   project.toml, file tree, assets, watcher
  mdedit-sidecar/   sidecar TOML, comments, anchoring (also for loose files)
  mdedit-git/       versioning: autocommit, checkpoints, branches, sync
  mdedit-export/    AST → Typst → PDF, AST → DOCX, templates
  mdedit-theme/     theme/token loading, built-in themes, contrast tests
  mdedit-editor/    Edit-mode widget (iced)
  mdedit-view/      View/Split widget (iced)
  mdedit-app/       iced application shell, keymap, menus (binary: mdedit)
  (later) mdedit-mcp/  MCP server over the core command API
```

Dependency rule: `app → {editor, view, render, project, sidecar, git, export, theme} → core`;
only `editor`, `view` and `app` depend on iced. (Sidecar, editor and view were
split out in the M0 skeleton so workstreams can run in parallel; see
[PLAN.md](PLAN.md).) Core
depends on nothing UI-related, so it is fully unit-testable and usable from a
headless MCP server.

## 2. Text buffer (`mdedit-core::buffer`)

```rust
pub trait TextBuffer: Send + Sync {
    fn len_bytes(&self) -> usize;
    fn len_lines(&self) -> Option<usize>;          // None while a large file is indexing
    fn line(&self, idx: usize) -> Cow<'_, str>;
    fn slice(&self, range: Range<usize>) -> Cow<'_, str>;
    fn byte_to_line(&self, byte: usize) -> usize;
    fn line_to_byte(&self, line: usize) -> usize;
    fn chunks(&self, range: Range<usize>) -> Box<dyn Iterator<Item = &[u8]> + '_>;
    fn apply(&mut self, edit: &Edit) -> AppliedEdit; // returns the inverse for undo
}
```

Two implementations, chosen at open time by file size:

| | `RopeBuffer` (≤ threshold) | `MappedBuffer` (> threshold) |
|---|---|---|
| Storage | `ropey::Rope` (1.6), whole file in memory | `memmap2` map of the original + append-only "add" buffer |
| Structure | rope (B-tree of chunks) | piece table in a balanced tree (pieces point into map or add buffer) |
| Line index | built into ropey | background scan with `memchr`, stored as a sparse checkpoint every 1024 lines (8 bytes per checkpoint), refined on demand |
| Save | write rope chunks to temp file, fsync, rename | stream pieces to temp file, fsync, rename, then remap |

All mutations go through `Edit { range, text, origin, txn }` and are appended
to an **edit log**. Undo/redo, comment anchors, the block index, syntax caches
and (later) collaboration all subscribe to the edit log instead of watching
the buffer directly.

## 3. Document model and incremental parsing

### 3.1 Block index

`Document` owns the buffer and a **block index**: an ordered list of top-level
blocks (`kind`, byte range, a parse generation), stored so it can be shifted
by edits in O(log n): leaves of about 64 blocks with leaf-relative offsets,
and Fenwick trees over the leaves' byte spans and block counts (lookup by
byte or by block number is O(log n) too). Line ranges are not stored; the
buffer maps bytes to lines.

Because of this storage, `BlockIndex::blocks()` returns a `Blocks` view
(`len`, `get(i)`, `iter()`) that yields `Block`s **by value** with absolute
ranges, and `blocks_in(range)` returns an iterator; there is no `&[Block]`
slice. A block that a reparse reproduces unchanged keeps its generation, so
render caches survive edits elsewhere. `last_change()` names the reparsed
block range and the link labels whose definition changed.

### 3.2 Incremental reparse

`pulldown-cmark` is not incremental, so incrementality is done at block
granularity:

1. An edit marks the top-level blocks it touches as dirty and shifts the
   offsets of all later blocks.
2. The reparse window starts at the beginning of the block before the first
   dirty one (a change can merge a paragraph into the previous one or turn it
   into a setext heading) and ends at the end of the block after the last dirty
   one.
3. `pulldown-cmark` parses just that window (with `into_offset_iter()` for
   source ranges). If the last produced block ends exactly on an old block
   boundary, the result is spliced in. If not (an unclosed code fence or HTML
   block swallowed text), the window is extended to the next boundary and the
   step repeats. Worst case is a full reparse, which is still bounded by the
   document size and only happens for edits like opening a code fence.
4. Link reference definitions are document-global: they are collected in a
   side table; a change to one invalidates only the blocks that use that label.

As implemented: the window grows by doubling the number of old blocks it
covers, so opening a fence near the top of a big file costs O(log n) parse
attempts, not one per block. The window also steps back over link reference
definitions on the lines just before it, since those make the next lines a
paragraph continuation rather than a fresh start. A proptest of random edit
sequences and every CommonMark 0.31.2 spec example (typed, pasted, deleted)
check the result against a full parse.

A typical keystroke reparses one paragraph (about 12 µs on a 10 MB file,
`cargo bench -p mdedit-core --bench parse`). Parsing runs on the UI thread
for small windows and on a worker when the window exceeds 64 KB, with the
previous result shown until the new one arrives: the index keeps the old
blocks shifted and marks the region pending; the app takes the job with
`Document::take_parse_job()`, runs `ParseJob::run()` on
`tokio::task::spawn_blocking`, and hands the result to
`Document::finish_parse()` (a result for an older version is dropped and the
next job covers the newer edits). `BlockIndex::deferred(buf)` opens a
document with the whole parse as such a job (a full parse of 10 MB takes a
few hundred milliseconds).

**Fallback plan:** if block-level reparse proves fragile, switch to
`tree-sitter-md` (true incremental parsing) for structure and keep
`pulldown-cmark` for rendering events. The block index API is designed so this
swap is internal to `mdedit-core::parse`.

### 3.3 Large files

Large files have no block index. Edit mode styles each visible line with a
line-local tokenizer (headings, emphasis, code spans, fence state carried from
the previous visible line on a best-effort basis): `highlight_line(line,
&mut LineState)`. Parsed documents use `highlight_lines(index, buf, lines)`,
the same tokenizer with block context from the index (code, HTML, setext
headings, tables), so a viewport can start mid-block. `scan_outline(buf)`
builds the large-file outline from a line scan.

## 4. Rendering

### 4.1 Edit mode: virtualised lines

A custom iced widget (`iced::advanced::Widget`) instead of `text_editor`,
because `text_editor` lays out the whole document.

- A **height map** (Fenwick tree of line heights) maps scroll offset to line
  index in O(log n). Unmeasured lines use an estimate (font line height ×
  estimated wrap count from byte length); real heights replace estimates as
  lines become visible, with a scroll anchor so the view doesn't jump.
- Only lines in the viewport plus one screen of overscan are shaped. Shaped
  lines (`iced::advanced::text::Paragraph`, cosmic-text underneath) live in an
  LRU cache keyed by `(line content hash, width, style generation)`, capped at
  about 2000 entries.
- Cursor, selection, search highlights and comment highlights are drawn as
  quads behind the text; the gutter is drawn in the same widget.
- Input handling (IME, clipboard, keyboard) lives in the widget and produces
  `Edit`s; the widget never mutates the buffer itself.

### 4.2 View mode: virtualised blocks

- `mdedit-render` turns each block's events into a **layout tree** (styled
  spans, list markers, table grid, image boxes) with no GPU dependency, so the
  same model drives the preview and export.
- The view widget virtualises by **block** with the same height-map approach;
  each block is laid out lazily when it scrolls into view and cached until its
  parse generation changes.
- The block under the cursor is shown as styled source (Typora model); leaving
  it re-renders it. Mapping between rendered positions and source bytes uses
  the source ranges kept on every span.
- Images are decoded off-thread, downscaled to display size, and evicted from
  an LRU cache by memory budget (default 64 MB).

### 4.3 Split mode

Edit widget + read-only View widget on the same `Document`. Scroll sync maps
the top visible line to its block and aligns the other pane to the same block
plus the proportional offset inside it.

### 4.4 Math and Mermaid (v1.x hooks)

`BlockRenderer` trait: `fn layout(&self, block: &Block, ctx) -> LayoutNode`.
Built-ins handle CommonMark blocks; math and Mermaid register their own.

- Math: render with `typst` (already a dependency for PDF export): compile a
  tiny `$ ... $` document and draw the result as vector paths (`typst-svg` →
  iced `svg`), cached by formula hash. Same engine for screen and PDF, pure
  Rust.
- Mermaid (`mdedit-mermaid`): `mermaid-rs-renderer` (pure Rust, ~0.1-2 ms
  per diagram) lays the diagram out with colours from the theme tokens and
  the UI font; `usvg` then turns every label into paths using the bundled
  fonts, so the SVG draws identically in iced's `svg` widget, in Typst and
  as a `resvg` PNG for DOCX. The renderer measures labels with a system font
  it caches under `$XDG_CACHE_HOME/mmdr/font-cache`; we seed that cache with
  the bundled UI font so boxes fit (harmless if it ever stops working: boxes
  get a little loose). The view does not use `BlockRenderer` for it: the
  View widget draws ```` ```mermaid ```` code blocks itself
  (`mdedit-view/src/diagrams.rs`, one worker thread started on first use, a
  16 MB LRU of SVGs keyed by source + theme, and the last good diagram per
  block shown while an edit is re-rendering). Sources over 64 KB stay
  code. The release profile aborts on panic, so a proptest checks the
  renderer on half-typed sources.

### 4.5 Renderer backend

iced with `wgpu` by default; the `tiny-skia` software renderer is enabled as a
fallback feature for machines without a usable GPU (and lower memory).

## 5. Commands, keymap, menus

- Every user action is a `Command` (id, title, icon, default shortcut,
  `fn run(&mut AppState, args) -> Task`) in one registry in `mdedit-core`
  (document commands) and `mdedit-app` (UI commands).
- Menu bar, floating toolbar, slash menu, command palette and keymap all read
  the registry, so a command is defined once.
- The same document command API is what the MCP server will expose.

## 6. Project, sidecars, assets (`mdedit-project`)

- `Project::open(dir)` reads `.mdedit/project.toml` (`serde` + `toml`),
  builds the file tree lazily, starts a `notify` watcher (debounced).
- Sidecars: `Sidecar::load(md_path)` / `save()`; comment anchors are kept as
  live offsets while the document is open (updated from the edit log) and
  written back as quote + prefix + suffix + hint on save. Re-anchoring
  algorithm per SPEC 7.
- Assets: content-addressed dedupe (SHA-256), collision-safe naming, link
  rewriting on rename/move.

## 7. Git (`mdedit-git`)

- Library: **`gix` (gitoxide)**, pure Rust, for status, commit, tree writing,
  branches, tags, clone and fetch.
- Push: `gix` push support has lagged behind the rest. If it is still
  missing when Sync is implemented, push goes through `git2` (libgit2), the
  only non-Rust dependency, and only for push. It is **optional**: behind a
  `push` cargo feature, isolated in `mdedit-git::push` behind a `PushBackend`
  trait. Builds without it (or where libgit2 can't be used) run normally with
  push disabled in the UI. Release builds enable the feature with libgit2
  vendored and statically linked, so users never need to install it.
- Autocommit: a debounced task per project; commits are written directly
  from the files on disk (no shelling out, no index lock held for long).
- Checkpoint squash: builds one commit whose tree is the current tree and
  whose parent is the previous checkpoint, then moves the branch ref. Since
  autocommits are never pushed (SPEC 6.3), this never rewrites shared history.
- All git work runs on a background worker; the UI receives status messages.

## 8. Export (`mdedit-export`)

- Input: the `mdedit-render` layout model (or parse events) + a resolved
  `Template`.
- PDF: generate Typst markup, compile with `typst` as a library through a
  minimal `World` implementation (bundled fonts, project files for images),
  produce PDF with `typst-pdf`.
- DOCX: `docx-rs` with paragraph/character/table styles created from the
  template's TOML.
- Runs on a blocking worker; progress and errors go back as messages.

## 9. Threads and async

- UI thread: iced event loop, input, layout of visible content, small
  reparses.
- `tokio` runtime (already enabled in iced): file I/O, watchers, git, export,
  large-file indexing and search (`spawn_blocking` for CPU-bound work).
- Communication only by messages (`Task` / `Subscription`); workers get
  immutable snapshots (cheap for ropey via `Rope::clone`; for `MappedBuffer` a
  snapshot of the piece tree).

## 10. Memory budget (targets)

| Item | Budget |
|---|---|
| Renderer baseline (wgpu) | ~60-80 MB (tiny-skia lower) |
| Rope buffer | ≈ file size + ~10 % |
| Block index | ~48 bytes per block |
| Shaped line cache | ≤ 2000 lines |
| Image cache | ≤ 64 MB, configurable |
| Large-file mode | map is OS page cache, index ~8 bytes per 1024 lines, edits proportional to edit size |

A `--trace-mem` debug flag logs RSS and cache sizes to check these.

## 11. Testing

- Core: unit tests plus property tests (`proptest`): random edit sequences
  must give the same block index as a full reparse, and the same text in
  `RopeBuffer` and `MappedBuffer`.
- Round-trip: open/save of the CommonMark spec examples is byte-identical.
- Comment anchoring: fuzz edits around anchors, check detach vs move.
- Themes: every built-in theme passes the contrast test.
- Benchmarks (`criterion`): keystroke-to-reparse, scroll frame layout, open
  1 GB file, regex search over 1 GB.
- Git: tests run against temp repos.

## 12. Key dependencies (planned)

`iced` 0.14, `ropey` 1.6, `pulldown-cmark` 0.13, `rfd` 0.17, `tokio`,
`memmap2`, `memchr`, `regex` / `regex-automata`, `notify`, `serde` + `toml`,
`gix`, `typst` + `typst-pdf` (+ `typst-svg` for math), `docx-rs`, `syntect`,
`image`, `directories`, `keyring`, `sha2`, `unicode-segmentation`,
`unicode-width`.
