#!/usr/bin/env bash

set -euo pipefail

TARGET="${1:?Usage: prepare-desktop-bundle.sh <target-triple>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONSOLE="$ROOT/console"
TAURI_DIR="$CONSOLE/src-tauri"
BIN_DIR="$TAURI_DIR/binaries"
PYTHON="${PYTHON:-python3}"
if ! command -v "$PYTHON" >/dev/null 2>&1; then
    PYTHON=python
fi
if ! command -v "$PYTHON" >/dev/null 2>&1; then
    printf '%s\n' 'Python 3 is required to prepare Desktop bundle metadata' >&2
    exit 1
fi

case "$TARGET" in
    aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu)
        RUNTIME_SOURCE="$ROOT/target/$TARGET/release/rua"
        RUNTIME_DEST="$BIN_DIR/rua-$TARGET"
        ;;
    x86_64-pc-windows-msvc)
        RUNTIME_SOURCE="$ROOT/target/$TARGET/release/rua.exe"
        RUNTIME_DEST="$BIN_DIR/rua-$TARGET.exe"
        ;;
    *)
        printf 'unsupported Desktop target: %s\n' "$TARGET" >&2
        exit 2
        ;;
esac

VERSION="$("$PYTHON" - "$ROOT/Cargo.toml" <<'PY'
import pathlib
import re
import sys

text = pathlib.Path(sys.argv[1]).read_text()
section = text.split("[workspace.package]", 1)[1].split("[", 1)[0]
match = re.search(r'^version\s*=\s*"([^"]+)"', section, re.MULTILINE)
if not match:
    raise SystemExit("workspace.package.version not found")
print(match.group(1))
PY
)"

"$PYTHON" - "$VERSION" "$CONSOLE/package.json" "$TAURI_DIR/tauri.conf.json" <<'PY'
import json
import pathlib
import sys

expected = sys.argv[1]
for path in map(pathlib.Path, sys.argv[2:]):
    actual = json.loads(path.read_text()).get("version")
    if actual != expected:
        raise SystemExit(f"{path}: version {actual!r} does not match {expected!r}")
PY

if [[ -n "${GITHUB_REF_NAME:-}" && "$GITHUB_REF_NAME" == v* ]]; then
    if [[ "${GITHUB_REF_NAME#v}" != "$VERSION" ]]; then
        printf 'tag %s does not match product version %s\n' "$GITHUB_REF_NAME" "$VERSION" >&2
        exit 1
    fi
fi

printf '%s\n' "--- Build unified rua Runtime ($TARGET) ---"
cargo build --release -p rua-cli --target "$TARGET" --manifest-path "$ROOT/Cargo.toml"

printf '%s\n' '--- Build Console and Desktop bootstrap ---'
"$PYTHON" "$ROOT/scripts/with-console-build-lock.py" "$BASH" -eu -c '
    cd "$1"
    [[ -x node_modules/.bin/tauri ]] || npm ci
    npm run build
    npm run build:desktop-bootstrap
' console-build "$CONSOLE"

if [[ ! -f "$RUNTIME_SOURCE" ]]; then
    printf 'Runtime artifact missing: %s\n' "$RUNTIME_SOURCE" >&2
    exit 1
fi
if [[ ! -f "$CONSOLE/dist/index.html" ]]; then
    printf '%s\n' 'Console production build is missing dist/index.html' >&2
    exit 1
fi
if [[ ! -f "$CONSOLE/dist-desktop-bootstrap/desktop-bootstrap.html" ]]; then
    printf '%s\n' 'Desktop bootstrap build is missing desktop-bootstrap.html' >&2
    exit 1
fi

"$PYTHON" "$ROOT/scripts/ci/stage-desktop-automation.py" "$TARGET" --runtime "$RUNTIME_SOURCE"

printf 'staged Console: %s\n' "$CONSOLE/dist"
printf 'product version: %s\n' "$VERSION"
