# mdedit performance numbers

Measured numbers for the targets in SPEC 1 and SPEC 8. Each section says
how to reproduce it. Machine unless noted: Intel i7-6700K (4 cores / 8
threads, 4.0 GHz), 31 GB RAM, NVMe SSD, Linux 6.5, rustc 1.99.1, release
profile (thin LTO).

## Large files (W2, 2026-11-01)

```bash
cargo run --release -p mdedit-core --example large_file -- /path/to/big.md
```

The example generates a 1 GB markdown-like file (16.2 M lines, headings,
lists, some non-ASCII words) if the path does not exist, then opens it with
`io::load` (so it goes through the large-file switch). The file was just
written, so it was in the page cache; a cold open reads it from disk during
indexing, which is in the background either way. Memory is the change in
resident set from before the open (`/proc/self/status`): *anon* is private
memory (heap), *file-backed* is pages of the mapped file, which belong to
the OS page cache and are dropped under memory pressure without any I/O.

| Step (1 GB file) | Result | Target |
|---|---|---|
| Open (map + first piece) | 0.14 ms | typing possible immediately |
| First 60 lines readable | 0.17 ms after open | |
| Background line index (`memchr`, UTF-8 check) | 2.2 s, progress reported | `len_lines` is `None` until then |
| Memory after open + full index | **+1.2 MB** (anon +0.1 MB) | under 50 MB overhead before edits |
| Line index size | 15.8 k checkpoints, 126 KB | ~8 bytes per 1024 lines |
| `line(i)` at a random line | 8.7 µs | |
| One screen (60 lines) mid-file | 0.63 ms | well inside an 8 ms frame |
| `line_to_byte` + `byte_to_line`, random | 60 µs per pair | |
| Typing (one char) | 0.6 µs | |
| 10 k scattered edits | 20 k pieces, anon +5 MB | edits proportional to edit size |
| Literal search, unedited, in place | 287 ms (3.7 GB/s), 161 matches | |
| Literal search, streamed, after 10 k edits | first result 5 ms, all 507 ms (2.1 GB/s) | results appear as found |
| Regex `^#{2,3} \w+ \d+$` | first result 8 ms, all 0.78 s (1.4 GB/s), 268 k matches | regex over the whole file, streamed |
| Case-insensitive whole word `Quantum` | 12.8 M matches, 4.2 s (0.26 GB/s) | |
| Memory during streamed search | anon +8-10 MB, file-backed ~+50 MB | |
| Replace-all (161 matches, one undo step) | 0.66 s; undo 0.35 ms | one undo step |
| Save (stream, fsync, rename, remap) | 2.85 s, back to 1 piece | atomic replace |

Notes:

- The indexer and search release the mapped pages behind them
  (`MADV_DONTNEED` on the shared read-only map), which is why the resident
  set stays flat while they stream the whole gigabyte. Without it the
  file-backed part grows to the file size (still page cache, but it shows
  up in RSS and worries users).
- Many scattered edits make line lookups count newlines inside the
  checkpoint blocks around each edit; those reads stay resident as
  file-backed pages (+850 MB after 10 k edits spread over the whole file,
  anon only +5 MB). They are page cache and are reclaimed by the OS; if it
  ever matters, the lookups can release pages the way search does.
- Search speed with many matches is bounded by the per-match cost of the
  regex engine finding each start (about 0.3 µs per match); with few
  matches it is bounded by reading the map (page faults), close to a bare
  `memmem` over the same map (230 ms).
- Search windows are 1 MB and look 256 KB past their end, so a single
  match longer than 256 KB is cut short. Plain text is searched as an
  escaped regex.
