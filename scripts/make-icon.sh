#!/usr/bin/env bash
# Regenerates assets/RTerm.icns (and the README preview PNG) from assets/icon.svg.
#
# Only needs to run when the SVG changes: the .icns is committed, so that
# builds and CI need no graphics tooling.
#
# Requirements: macOS (iconutil) and librsvg (`brew install librsvg`).
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
echo "Regenerated assets/RTerm.icns and assets/icon.png"
