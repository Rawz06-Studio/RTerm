#!/usr/bin/env bash
# Builds RTerm for Linux and packs it under dist/ as:
#   RTerm-<version>-linux-<arch>.tar.gz   bin/, share/applications, share/icons:
#                                         extract it into ~/.local or /usr/local
#   RTerm-<version>-linux-<arch>.AppImage a single file to run
#
#   scripts/bundle-linux.sh [version]
#
# With a version (as CI passes it), the binary reports that version. Without
# one, the version from Cargo.toml is used and reported as "<version>-dev".
#
# Building the AppImage downloads appimagetool on first use (cached in
# target/appimagetool); set RTERM_NO_APPIMAGE=1 to skip it.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
version="${1:-$cargo_version}"
if [[ $# -ge 1 ]]; then
    export RTERM_VERSION="$version"
fi
arch="$(uname -m)"
target_dir="${CARGO_TARGET_DIR:-target}"
name="RTerm-$version-linux-$arch"
dist="dist"
rm -rf "$dist"
mkdir -p "$dist"

echo "==> Building RTerm $version ($arch)"
cargo build --release
binary="$target_dir/release/rterm"
"$binary" --version

echo "==> Packing $name.tar.gz"
root="$dist/$name"
install -Dm755 "$binary" "$root/bin/rterm"
install -Dm644 packaging/linux/rterm.desktop "$root/share/applications/rterm.desktop"
install -Dm644 assets/icon.png "$root/share/icons/hicolor/256x256/apps/rterm.png"
tar -C "$dist" -czf "$dist/$name.tar.gz" "$name"

if [[ "${RTERM_NO_APPIMAGE:-}" != 1 ]]; then
    echo "==> Packing $name.AppImage"
    tool="$target_dir/appimagetool/appimagetool-$arch.AppImage"
    case "$arch" in
        x86_64) tool_sha256="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0" ;;
        aarch64) tool_sha256="f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158" ;;
        *) echo "Unsupported AppImage architecture: $arch" >&2; exit 1 ;;
    esac
    if [[ ! -x "$tool" ]]; then
        mkdir -p "$(dirname "$tool")"
        curl -fsSL -o "$tool.tmp" \
            "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage"
        echo "$tool_sha256  $tool.tmp" | sha256sum --check --status
        mv "$tool.tmp" "$tool"
        chmod +x "$tool"
    fi
    echo "$tool_sha256  $tool" | sha256sum --check --status
    appdir="$dist/RTerm.AppDir"
    install -Dm755 "$binary" "$appdir/usr/bin/rterm"
    install -Dm644 packaging/linux/rterm.desktop "$appdir/rterm.desktop"
    install -Dm644 assets/icon.png "$appdir/rterm.png"
    install -Dm644 assets/icon.png "$appdir/usr/share/icons/hicolor/256x256/apps/rterm.png"
    cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/rterm" "$@"
APPRUN
    chmod +x "$appdir/AppRun"
    # Extracting instead of mounting: no FUSE needed (containers, CI).
    APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$arch" "$tool" --no-appstream "$appdir" "$dist/$name.AppImage"
    rm -rf "$appdir"
fi

rm -rf "$root"
(cd "$dist" && sha256sum "$name".tar.gz $(ls "$name".AppImage 2>/dev/null) > "$name.sha256")
echo "==> Done"
ls -lh "$dist"
