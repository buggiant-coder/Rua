#!/usr/bin/env bash

set -euo pipefail

TARGET="${1:?Usage: verify-macos-desktop-bundle.sh <target> [--signed]}"
SIGNED="${2:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BUNDLE_DIR="$ROOT/console/src-tauri/target/$TARGET/release/bundle"
APP="$BUNDLE_DIR/macos/Rua.app"
ARTIFACT_DIR="$ROOT/target/desktop-acceptance/$TARGET"

if [[ ! -d "$APP" ]]; then
    printf 'packaged application not found: %s\n' "$APP" >&2
    exit 1
fi

MAIN="$APP/Contents/MacOS/rua-desktop"
RUNTIME="$APP/Contents/MacOS/rua"
CONSOLE_INDEX="$APP/Contents/Resources/console/index.html"

test -x "$MAIN"
test -x "$RUNTIME"
test -f "$CONSOLE_INDEX"
# The Shell must load before it can report Computer Use as unavailable on 11/12.
for component in rua-desktop rua agent-browser; do
    if otool -L "$APP/Contents/MacOS/$component" | grep -Eq 'ScreenCaptureKit\.framework|libswift_Concurrency\.dylib'; then
        printf 'unexpected capture runtime linked into %s\n' "$component" >&2
        exit 1
    fi
done
for component in rua agent-browser cua-driver; do
    test -x "$APP/Contents/MacOS/$component"
done
"$MAIN" --verify-desktop-bundle

if [[ "$SIGNED" == "--signed" ]]; then
    for component in rua-desktop rua agent-browser cua-driver; do
        codesign --verify --strict --verbose=2 "$APP/Contents/MacOS/$component"
    done
    codesign --verify --deep --strict --verbose=2 "$APP"
    spctl --assess --type execute --verbose=2 "$APP"
    xcrun stapler validate "$APP"
    if compgen -G "$BUNDLE_DIR/dmg/*.dmg" >/dev/null; then
        for dmg in "$BUNDLE_DIR"/dmg/*.dmg; do
            xcrun stapler validate "$dmg"
        done
    fi
fi

mkdir -p "$ARTIFACT_DIR"
DATA_DIR="$(mktemp -d)"
LOG_FILE="$ARTIFACT_DIR/desktop-shell.log"
RUA_DESKTOP_ROOT="$DATA_DIR" "$MAIN" >"$LOG_FILE" 2>&1 &
SHELL_PID=$!

cleanup() {
    if [[ ! -f "$ARTIFACT_DIR/desktop.png" ]]; then
        screencapture -x "$ARTIFACT_DIR/desktop.png" || true
    fi
    if kill -0 "$SHELL_PID" 2>/dev/null; then
        "$MAIN" --quit >/dev/null 2>&1 || true
        wait "$SHELL_PID" || true
    fi
}
trap cleanup EXIT

ENDPOINT_FILE="$DATA_DIR/desktop-endpoint.json"
for _ in $(seq 1 240); do
    [[ -f "$ENDPOINT_FILE" ]] && break
    sleep 0.5
done
if [[ ! -f "$ENDPOINT_FILE" ]]; then
    printf '%s\n' 'Desktop Runtime did not publish endpoint state' >&2
    exit 1
fi

PORT="$(python3 - "$ENDPOINT_FILE" <<'PY'
import json
import pathlib
import sys
print(json.loads(pathlib.Path(sys.argv[1]).read_text())["port"])
PY
)"
curl --fail --silent --show-error "http://127.0.0.1:$PORT/readyz" >"$ARTIFACT_DIR/readyz.json"
curl --fail --silent --show-error "http://127.0.0.1:$PORT/console/" >"$ARTIFACT_DIR/console.html"
grep -q '<div id="root">' "$ARTIFACT_DIR/console.html"

sleep 3
screencapture -x "$ARTIFACT_DIR/desktop.png" || true
"$MAIN" --quit >/dev/null 2>&1
wait "$SHELL_PID"
trap - EXIT

test -d "$DATA_DIR"
printf 'macOS packaged application acceptance passed: %s\n' "$APP"
