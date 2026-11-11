#!/usr/bin/env bash
# Builds RTerm.app, signs it and packs it into a .dmg and a .zip under dist/.
#
#   scripts/bundle-macos.sh [version]
#
# With a version (as CI passes it), the binary and the bundle carry that
# version. Without one, the version from Cargo.toml is used and the binary
# reports it as "<version>-dev".
#
# Environment:
#   RTERM_ARCH          "universal" (default: Apple Silicon + Intel in one
#                       binary) or "native" (host architecture only, faster)
#   CODESIGN_IDENTITY   signing identity; defaults to "-" (ad-hoc signature).
#                       Set it to a "Developer ID Application: …" identity to
#                       produce a build Gatekeeper trusts once notarized.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
version="${1:-$cargo_version}"
if [[ $# -ge 1 ]]; then
    export RTERM_VERSION="$version"
fi
arch="${RTERM_ARCH:-universal}"
identity="${CODESIGN_IDENTITY:--}"
export MACOSX_DEPLOYMENT_TARGET="11.0"

dist="dist"
app="$dist/RTerm.app"
rm -rf "$dist"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

echo "==> Building RTerm $version ($arch)"
if [[ "$arch" == "universal" ]]; then
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --release --target "$target"
    done
    lipo -create -output "$app/Contents/MacOS/rterm" \
        target/aarch64-apple-darwin/release/rterm \
        target/x86_64-apple-darwin/release/rterm
else
    cargo build --release
    cp target/release/rterm "$app/Contents/MacOS/rterm"
fi

echo "==> Assembling $app"
cp assets/RTerm.icns "$app/Contents/Resources/RTerm.icns"
# CFBundleVersion must be purely numeric: drop any pre-release suffix.
build_number="${version%%-*}"
sed -e "s/@VERSION@/$build_number/" -e "s/@BUILD@/$build_number/" \
    packaging/macos/Info.plist > "$app/Contents/Info.plist"

echo "==> Signing with identity: $identity"
sign_options=(--force --sign "$identity")
if [[ "$identity" != "-" ]]; then
    # A real identity gets the hardened runtime and a timestamp, which
    # notarization requires.
    sign_options+=(--options runtime --timestamp)
fi
codesign "${sign_options[@]}" "$app/Contents/MacOS/rterm"
codesign "${sign_options[@]}" "$app"
codesign --verify --deep --strict "$app"

name="RTerm-$version-macos-$arch"

echo "==> Packing $dist/$name.dmg"
staging="$(mktemp -d)"
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "RTerm" -srcfolder "$staging" -ov -format UDZO "$dist/$name.dmg"
rm -rf "$staging"

echo "==> Packing $dist/$name.zip"
ditto -c -k --keepParent "$app" "$dist/$name.zip"

(cd "$dist" && shasum -a 256 "$name.dmg" "$name.zip" > "$name.sha256")
echo "==> Done:"
ls -lh "$dist"
