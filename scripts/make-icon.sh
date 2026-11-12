#!/usr/bin/env bash
# Regenerates the icons from assets/icon.svg: assets/RTerm.icns (macOS),
# assets/icon.ico (Windows) and assets/icon.png (Linux, README).
#
# Only needs to run when the SVG changes: the icons are committed, so that
# builds and CI need no graphics tooling.
#
# Requirements: macOS (iconutil), librsvg (`brew install librsvg`) and python3.
set -euo pipefail

cd "$(dirname "$0")/.."
command -v rsvg-convert >/dev/null || { echo "rsvg-convert not found: brew install librsvg" >&2; exit 1; }
command -v iconutil >/dev/null || { echo "iconutil not found: this script requires macOS" >&2; exit 1; }

iconset="$(mktemp -d)/RTerm.iconset"
mkdir -p "$iconset"

# The sizes macOS expects, each at single and double density.
for size in 16 32 128 256 512; do
    rsvg-convert -w "$size" -h "$size" assets/icon.svg -o "$iconset/icon_${size}x${size}.png"
    double=$((size * 2))
    rsvg-convert -w "$double" -h "$double" assets/icon.svg -o "$iconset/icon_${size}x${size}@2x.png"
done

iconutil --convert icns "$iconset" --output assets/RTerm.icns
rsvg-convert -w 256 -h 256 assets/icon.svg -o assets/icon.png

# Windows: one .ico holding a PNG per size.
ico_sizes=(16 24 32 48 64 128 256)
pngs=()
for size in "${ico_sizes[@]}"; do
    rsvg-convert -w "$size" -h "$size" assets/icon.svg -o "$iconset/ico_$size.png"
    pngs+=("$iconset/ico_$size.png")
done
python3 - assets/icon.ico "${pngs[@]}" <<'PY'
import struct, sys
output, paths = sys.argv[1], sys.argv[2:]
images = [open(path, "rb").read() for path in paths]
offset = 6 + 16 * len(images)
header = struct.pack("<HHH", 0, 1, len(images))
entries = b""
for data in images:
    width, height = struct.unpack(">II", data[16:24])  # from the PNG header
    entries += struct.pack("<BBBBHHII", width % 256, height % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
open(output, "wb").write(header + entries + b"".join(images))
PY
echo "Regenerated assets/RTerm.icns, assets/icon.ico and assets/icon.png"
