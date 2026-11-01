#!/usr/bin/env bash
# Regenerates every raster app and file icon from the two SVG sources in
# assets/icons/: the PNG set, the .ico and .icns files, and the Linux hicolor
# tree under packaging/linux/icons. Needs python3 and one SVG rasteriser
# (rsvg-convert, resvg or inkscape, tried in that order). Run from anywhere:
#   scripts/gen-icons.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
icons="$root/assets/icons"
png="$icons/png"
hicolor="$root/packaging/linux/icons/hicolor"
sizes=(16 24 32 48 64 128 256 512)
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

render() { # svg size out
    if command -v rsvg-convert >/dev/null; then
        rsvg-convert -w "$2" -h "$2" -o "$3" "$1"
    elif command -v resvg >/dev/null; then
        resvg -w "$2" -h "$2" "$1" "$3"
    elif command -v inkscape >/dev/null; then
        inkscape --export-type=png --export-width="$2" --export-height="$2" \
            --export-filename="$3" "$1" >/dev/null 2>&1
    else
        echo "gen-icons: install rsvg-convert, resvg or inkscape" >&2
        exit 1
    fi
}

mkdir -p "$png"
for name in mdedit mdedit-file-md; do
    for s in "${sizes[@]}"; do
        render "$icons/$name.svg" "$s" "$png/$name-$s.png"
    done
    render "$icons/$name.svg" 1024 "$tmp/$name-1024.png"
done

# .ico (16-256, PNG entries) and .icns, written by a small packer so the
# output only depends on the PNGs.
python3 -I - "$png" "$tmp" "$icons" <<'PY'
import struct, sys
from pathlib import Path

png, tmp, out = map(Path, sys.argv[1:])

def ico(name):
    sizes = [16, 24, 32, 48, 64, 128, 256]
    data = [(png / f"{name}-{s}.png").read_bytes() for s in sizes]
    head = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries = b""
    for s, d in zip(sizes, data):
        dim = 0 if s == 256 else s
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(d), offset)
        offset += len(d)
    (out / f"{name}.ico").write_bytes(head + entries + b"".join(data))

def icns(name):
    # OSType -> pixel size; every entry holds PNG data.
    types = [("icp4", 16), ("icp5", 32), ("icp6", 64), ("ic07", 128), ("ic08", 256),
             ("ic09", 512), ("ic10", 1024), ("ic11", 32), ("ic12", 64), ("ic13", 256),
             ("ic14", 512)]
    body = b""
    for t, s in types:
        src = tmp / f"{name}-1024.png" if s == 1024 else png / f"{name}-{s}.png"
        d = src.read_bytes()
        body += t.encode() + struct.pack(">I", 8 + len(d)) + d
    (out / f"{name}.icns").write_bytes(b"icns" + struct.pack(">I", 8 + len(body)) + body)

for name in ("mdedit", "mdedit-file-md"):
    ico(name)
    icns(name)
PY

# Linux hicolor tree: the app icon and the text/markdown file type icon.
for s in "${sizes[@]}"; do
    mkdir -p "$hicolor/${s}x$s/apps" "$hicolor/${s}x$s/mimetypes"
    cp "$png/mdedit-$s.png" "$hicolor/${s}x$s/apps/mdedit.png"
    cp "$png/mdedit-file-md-$s.png" "$hicolor/${s}x$s/mimetypes/text-markdown.png"
done
mkdir -p "$hicolor/scalable/apps" "$hicolor/scalable/mimetypes"
cp "$icons/mdedit.svg" "$hicolor/scalable/apps/mdedit.svg"
cp "$icons/mdedit-file-md.svg" "$hicolor/scalable/mimetypes/text-markdown.svg"
