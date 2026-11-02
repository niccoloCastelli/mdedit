# mdedit implementation plan

How v1 (SPEC sections 3-10) gets built: milestones, independent workstreams,
their dependencies, and the rules that let several threads work in parallel
without stepping on each other.

Status: v0.1 (2026-11-01). Read [SPEC.md](SPEC.md),
[DESIGN_SYSTEM.md](DESIGN_SYSTEM.md) and [ARCHITECTURE.md](ARCHITECTURE.md)
first; this file only says *who builds what, in which order*.

## 1. Workspace

The repo is a Cargo workspace (M0, done). Each workstream owns one crate or a
set of files, so parallel branches rarely touch the same code.

| Crate | Contents | Owner workstream |
|---|---|---|
| `mdedit-core` | `buffer.rs`, `edit.rs`, `edit_log.rs`, `undo.rs`, `io.rs`, `selection.rs`, `document.rs` | W1 core buffer |
| | `mapped.rs`, `search.rs` | W2 large files & search |
| | `parse.rs`, `highlight.rs` | W3 parsing |
| | `command.rs` | W12 commands & smart editing |
| `mdedit-editor` | Edit-mode widget (iced) | W4 editor widget |
| `mdedit-render` | GPU-free layout model | W5 render & View |
| `mdedit-view` | View/Split widget (iced) | W5 render & View |
| `mdedit-mermaid` | Mermaid → themed SVG / PNG (no iced) | ws/mermaid |
| `mdedit-theme` | tokens, themes, type scale, fonts, icons | W6 theme |
| `mdedit-app` | binary `mdedit`: shell, menus, tabs, panels, settings | W7 app shell |
| `mdedit-project` | project.toml, file tree, assets, watcher | W8 project |
| `mdedit-sidecar` | sidecar TOML, comments, anchoring | W9 sidecar |
| `mdedit-git` | autocommit, checkpoints, branches, sync | W10 git |
| `mdedit-export` | PDF (Typst) and DOCX (docx-rs), templates | W11 export |

Compared with ARCHITECTURE 1, three crates were split out for parallel work:
`mdedit-sidecar` (comments also work on loose files, so it does not belong to
project), `mdedit-editor` and `mdedit-view` (the two big widgets, so the app
shell crate is not a merge hotspot). Only `mdedit-editor`, `mdedit-view` and
`mdedit-app` depend on iced; `mdedit-theme` has an optional `iced` feature
for colour conversions.

Every module already exists as a placeholder with its intended public API
and doc comments pointing at the spec sections it implements. Placeholders
return `Unimplemented` errors or trivial results; nothing panics, so the app
runs at every step.

## 2. Shared interfaces

These are what other workstreams compile against. Their **owner** may
extend them freely; changing an existing signature means telling the
affected workstreams (through the coordinator) in the same change.

| Interface | Where | Owner | Used by |
|---|---|---|---|
| `TextBuffer`, `RopeBuffer` | core `buffer.rs` | W1 | everyone |
| `Edit`, `AppliedEdit`, `EditOrigin`, `TxnId`, `Selection` | core `edit.rs`, `selection.rs` | W1 | everyone |
| `EditLog`, `EditListener` | core `edit_log.rs` | W1 | W3, W9, W4/W5 caches |
| `Document` (`apply`, `blocks`, `buffer`) | core `document.rs` | W1 | W4, W5, W7, W12 |
| `BlockIndex`, `Block`, `BlockKind`, `markdown_options()`, `outline()` | core `parse.rs` | W3 | W5, W7, W11, W12 |
| `highlight_line`, `SyntaxToken` | core `highlight.rs` | W3 | W4 |
| `SearchQuery`, `search()` | core `search.rs` | W2 | W4, W7 |
| `CommandMeta`, `DocCommand`, `run()` | core `command.rs` | W12 | W7 |
| `ColorToken`, `Theme`, `typography`, `space` | `mdedit-theme` | W6 | W4, W5, W7 |
| `editor()`, `EditorState`, `Action` | `mdedit-editor` | W4 | W7 |
| `view()`, `ViewState`, `Action`, `LayoutNode`, `layout_block()` | `mdedit-view`, `mdedit-render` | W5 | W7, W11 (optional) |
| `Project`, `ProjectConfig` | `mdedit-project` | W8 | W7, W10 |
| `Sidecar`, `Comment`, `Anchor`, `reanchor`, `LiveAnchors` | `mdedit-sidecar` | W9 | W7, W11 |
| `Repo`, `PushBackend` | `mdedit-git` | W10 | W7 |
| `export()`, `Template` | `mdedit-export` | W11 | W7 |

Rule of thumb: the widget crates never mutate a `Document`; they return
actions and the app calls `Document::apply`. All text changes go through
`Document::apply`, so the edit log sees everything.

## 3. Workstreams

Each has a crate (or files), scope and done criteria. "Needs" lists hard
dependencies; everything else can be stubbed against the placeholders.

### W1 Core buffer and edit log (`mdedit-core`)
Scope: finish `RopeBuffer`; undo/redo with grouping (SPEC 5); `io::load`
with BOM and line-ending detection, non-UTF-8 read-only; atomic `io::save`
(temp, fsync, rename); dirty tracking; edit-log listener plumbing in
`Document`. Done: round-trip test (open/save unedited is byte-identical,
incl. CRLF and BOM files), undo grouping tests, proptest of random edits vs
a `String` model, criterion bench of insert on a 10 MB rope.
Needs: nothing.

### W2 Large files and search (`mdedit-core` `mapped.rs`, `search.rs`)
Scope: `MappedBuffer` (memmap2 + piece table + background `memchr` line
index, SPEC 8); streaming save; change-on-disk detection; search with case,
whole word and regex (`regex-automata` over chunks), streamed results,
replace-all as one transaction. Done: proptest "same edits give same text as
RopeBuffer"; open a generated 1 GB file in under 50 MB overhead; regex search
over 1 GB streams results; bench numbers recorded in `docs/PERF.md`.
Needs: nothing (hook into `io::load` by size is a three-line change done by
whichever of W1/W2 merges second).

### W3 Markdown parsing (`mdedit-core` `parse.rs`, `highlight.rs`)
Scope: block index in a structure that shifts in O(log n); incremental
window reparse (ARCHITECTURE 3.2) with extension on unclosed fences;
link-reference side table; worker offload for windows over 64 KB;
`highlight_line` for Edit-mode styling, both from the block index and the
line-local tokenizer for large files; outline. Done: proptest "random edit
sequences give the same block index as a full reparse"; CommonMark spec
examples parse to the same blocks as a full parse; keystroke reparse bench
in microseconds on a 10 MB file.
Needs: nothing (works on `RopeBuffer`).

### W4 Edit-mode widget (`mdedit-editor`)
Scope: custom `iced::advanced::Widget` per ARCHITECTURE 4.1: height map,
viewport-only shaping, LRU of shaped lines, cursor/selection/highlight
quads, gutter with line numbers, mouse selection, keyboard navigation (word,
line, page, document), IME, clipboard actions, syntax colours from
`highlight_line` + theme tokens, proportional vs monospace setting. A
standalone `examples/edit.rs` window for testing without the app. Done:
smooth scrolling and typing on a 10 MB file with the p99 under 8 ms per
keystroke (measured and noted), stable scroll anchor when line heights get
measured, all `Action`s emitted.
Needs: W1 for real undo/save (not for development), W3 for real colours
(placeholder returns none).

### W5 Render and View mode (`mdedit-render`, `mdedit-view`)
Scope, part 1 (can start now): `layout_block` for every CommonMark + GFM
block and inline, source ranges on every span, tests against the spec
examples. Part 2: View widget virtualised by block, Typora-style reveal of
the block under the cursor, cursor mapping through source ranges, images
(off-thread decode, LRU by memory), table grid editing, Split mode with
synced scroll. Done: part 1 fully tested; part 2 renders the design-system
styles (DESIGN_SYSTEM 7.1) and edits a document in View mode.
Needs: part 2 needs W6 fonts and benefits from W4's text-layout helpers.

### W6 Theme and design tokens (`mdedit-theme`, `assets/`)
Scope: TOML theme loading with `extends`, user theme dir (`directories`),
light/dark selection from settings and OS; WCAG contrast test over every
built-in theme; bundle Hanken Grotesk, JetBrains Mono, Source Serif 4 (OFL)
and Lucide icons (ISC) under `assets/` with licence files, and expose them
(`fonts()` returning bytes to load into iced, `icon(name)` returning SVG
bytes); syntect code-theme map per theme. Done: tests pass for both
built-ins, an example theme file in `docs/`, fonts load in the app.
Needs: nothing.

### W7 App shell (`mdedit-app`)
Scope: the layout of DESIGN_SYSTEM 6: in-window menu bar, tabs with dirty
dot, sidebar (Files, Outline, Search, Git placeholders), comments panel
placeholder, status bar, mode switcher (Ctrl+1/2/3, Ctrl+E), command
registry for UI commands + keymap table with `keymap.toml` overrides,
command palette, open/save/save-as flows, `settings.toml`, session restore,
external-change banner. Integrates each library as it lands. Done: M1 exit
criteria below met in the app.
Needs: W4 for real editing, W6 for fonts/tokens; starts on the placeholders.

### W8 Project and assets (`mdedit-project`)
Scope: `Project::init/open`, `project.toml` read/write, lazy file tree,
asset import with SHA-256 dedupe and collision-safe names, rename/move a
markdown file with its assets and sidecar and rewrite links, debounced
`notify` watcher emitting events. Done: tests on temp dirs for every
SPEC 6.4 rule.
Needs: nothing.

### W9 Sidecar comments (`mdedit-sidecar`)
Scope: load/save (atomic, delete when empty, keep unknown tables), anchor
creation with 32-char context, re-anchoring (exact with context, exact,
fuzzy bitap/Levenshtein, else detached), `LiveAnchors` moved by the edit
log. Done: SPEC 7 example round-trips; fuzz test of edits around anchors
(move or detach, never lost); external-change re-anchoring tests.
Needs: nothing.

### W10 Git (`mdedit-git`)
Scope: `Repo` on `gix`: init/open, status, autocommit `auto: <files>`,
debounced autocommit worker, checkpoint squash + annotated tags, branches
create/switch/rename/delete, history with autocommits folded, clone,
fetch + fast-forward pull, `PushBackend` with libgit2 behind the `push`
feature (vendored, static). Done: every operation tested against temp repos,
including "push never sends autocommits"; builds and passes with and
without `--features push`.
Needs: nothing.

### W11 Export (`mdedit-export`)
Scope: markdown → Typst markup, Typst `World` with bundled fonts and project
images, PDF via `typst-pdf`; DOCX via `docx-rs` with styles from the
template; template schema (`template.toml`) and the three built-ins (Guide,
Report, Plain); code highlighting, tables, footnotes, task lists, images.
Done: golden tests (Typst markup snapshots; DOCX XML snapshots), a sample
document exported to both formats by an example binary.
Needs: W6 fonts for the final look (can use system fonts meanwhile).

### W12 Commands and smart editing (`mdedit-core` `command.rs`)
Scope: every formatting command of DESIGN_SYSTEM 8.2 as edits (toggles),
command metadata registry, smart editing (8.3: list continuation, indent,
wrap-selection pairs), table operations and Edit-mode auto-alignment (8.4),
CSV/TSV paste. Done: unit tests per command on markdown fixtures.
Needs: W3 (uses the block index to know the context). Start after W3's
first merge.

### Later
- **Wiring panels** (Outline, Search, Git, Comments, Export dialog) is W7's
  job as each backend lands.
- **v1.x:** math (Typst → SVG), Mermaid (done: `mdedit-mermaid`, ws/mermaid), branch merge + conflict UI.
- **v2:** collaboration (CRDT behind the edit log), `mdedit-mcp`.

## 4. Dependency graph

```
W1 core buffer ──┬──> W2 large files (io hook only)
                 ├──> W4 editor widget ──┐
                 └──> W3 parsing ──┬─────┤
                                   ├──> W12 commands
                                   └──> W5 render/view ──┐
W6 theme ─────────────────────────────> W4, W5, W11      │
W8 project, W9 sidecar, W10 git, W11 export (independent)│
                                                         v
                        W7 app shell integrates everything
```

Solid dependencies are only for *finishing*; every workstream except W12
can start today against the placeholders.

## 5. Milestones

| Milestone | Content | Workstreams | Exit criteria |
|---|---|---|---|
| **M0** Skeleton | workspace, placeholders, plan | - | done (this commit) |
| **M1** Edits a file | open, type, select, undo, save; Edit mode with syntax colours; tokens and fonts | W1, W3 (highlight), W4, W6, W7 | 10 MB file: open < 300 ms to first frame, typing p99 < 8 ms, unedited save byte-identical |
| **M2** Markdown-aware | incremental parse, outline, find/replace, large files, formatting commands, palette, slash menu, toolbar | W2, W3, W12, W7 | 1 GB file opens and searches within SPEC 8 targets; every 8.2 command works from menu, shortcut and palette |
| **M3** View and Split | WYSIWYG, preview, tables, images | W5, W7 | edit a real document entirely in View mode; Split scroll sync |
| **M4** Projects | project mode, assets, comments, git (autocommit, checkpoints, branches, sync) | W8, W9, W10, W7 | a project survives a day of use: autocommits, checkpoint + tag, sync with a remote, comments re-anchor after external edits |
| **M5** Export and polish | PDF/DOCX with templates, settings dialog, session restore, macOS and Windows builds, perf and memory checks | W11, W6, W7 | all SPEC 1 targets measured; release builds on three platforms |

M2-M5 workstreams mostly start before their milestone; the milestone is
when they are integrated and accepted in the app.

## 6. Parallel work: worktrees and merging

- `master` in `/home/nicco/rust/mdedit` is the integration branch. Nobody
  edits files in that checkout directly; it is only used to merge.
- Each workstream gets a branch `ws/<name>` and a worktree
  `/home/nicco/rust/mdedit-wt/<name>`:

  ```bash
  git -C /home/nicco/rust/mdedit worktree add -b ws/<name> /home/nicco/rust/mdedit-wt/<name> master
  ```

- Stay current: `git merge master` into your branch whenever master moves
  (at least before merging back).
- Before merging back, in your worktree, all must pass on the merged result:

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

- Merge back (one merge at a time; if another merge is in progress, wait):

  ```bash
  git -C /home/nicco/rust/mdedit merge --no-ff ws/<name>
  ```

- Merge early and often: a placeholder replaced by a working subset is worth
  merging, so others build on the real thing sooner.
- Stay inside your files (table in section 1). If you need a small change
  elsewhere (a new method on a shared type, a dependency line), make it
  additive and mention it in the merge commit message; for anything
  breaking, ask the owner through the coordinator first.
- Dependencies: versions are declared once in the root `[workspace.dependencies]`;
  crates use `foo.workspace = true`. Add a new entry there only if it is
  missing (additive, one line). `Cargo.lock` conflicts: take master's
  version and run `cargo check --workspace` to regenerate.
- Builds: each worktree has its own `target/` (a few GB each, disk is fine).
  The machine has 8 cores, so test only your crate while iterating
  (`cargo test -p <crate>`) and run the workspace checks before merging.
- No pushes: there is no remote; everything stays local.
- Linux (X11, `DISPLAY=:1`) is the test platform. Rust is in `~/.cargo/bin`
  (not on `PATH` in non-login shells).
