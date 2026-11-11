#!/usr/bin/env bash
# Regenerates the README screenshots in docs/screenshots/.
#
# RTerm is launched with a throwaway home directory and driven through its
# RTERM_RUN / RTERM_DEMO hooks, so nothing from your own setup (prompt, paths,
# history) ends up in the pictures.
#
# Requirements: macOS with an unlocked session (window capture is refused on
# the lock screen), plus Screen Recording and Automation (System Events)
# permissions for your terminal. RTerm's window comes to the front while the
# pictures are taken.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build
binary="$PWD/target/debug/rterm"
output="$PWD/docs/screenshots"
mkdir -p "$output"

# The physical path, so that RTerm shortens it to "~" in tabs and prompts.
home="$(mktemp -d)"
home="$(cd "$home" && pwd -P)"

# Never leave RTerm running or the temporary home behind, even on failure.
pid=""
cleanup() {
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
    rm -rf "$home"
}
trap cleanup EXIT

mkdir -p "$home/project/src"
(cd "$home/project" && git init -q . && touch src/main.rs src/lib.rs README.md Cargo.toml)

# Prints the id of RTerm's on-screen window.
window_id() {
    swift - <<'EOF'
import CoreGraphics
import Foundation
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
for window in windows where (window[kCGWindowOwnerName as String] as? String) == "rterm" {
    let bounds = window[kCGWindowBounds as String] as? [String: Any]
    if ((bounds?["Height"] as? Double) ?? 0) > 100, let id = window[kCGWindowNumber as String] as? Int {
        print(id)
        break
    }
}
EOF
}

# capture <file> <seconds to wait> — RTERM_RUN / RTERM_DEMO come from the caller.
capture() {
    HOME="$home" "$binary" >/dev/null 2>&1 &
    pid=$!
    # The window takes a moment to appear.
    local id="" attempt
    for attempt in 1 2 3 4 5 6 7 8 9 10; do
        sleep 0.5
        id="$(window_id)"
        [[ -n "$id" ]] && break
    done
    if [[ -z "$id" ]]; then
        echo "error: RTerm's window never showed up" >&2
        exit 1
    fi
    # A window left in the background is not redrawn: bring it to the front.
    osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $pid) to true"
    sleep "$2"
    # Window capture occasionally fails for no visible reason: try again.
    for attempt in 1 2 3; do
        screencapture -x -o -l "$(window_id)" "$output/$1" && break
        sleep 1
    done
    [[ -s "$output/$1" ]]
    kill "$pid"
    wait "$pid" 2>/dev/null || true
    pid=""
    echo "wrote docs/screenshots/$1"
}

RTERM_RUN=$'cd project\nls\ngit status --short\ncargo --version\nls /missing' \
    capture blocks.png 5

RTERM_DEMO='new-tab,theme:RTerm Light,palette,type:theme' \
    capture palette.png 3

RTERM_DEMO='new-tab,new-tab,vertical-tabs,complete:git ch' \
    capture completion.png 3
