#!/usr/bin/env bash

set -euo pipefail

TARGET="${1:?Usage: build-desktop.sh <target-triple>}"
MODE="${2:-release}"
if [[ "$MODE" != release && "$MODE" != --test-package ]]; then
    printf 'unknown build mode: %s\n' "$MODE" >&2
    exit 2
fi
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONSOLE="$ROOT/console"
TAURI_DIR="$CONSOLE/src-tauri"
DIST="$ROOT/target/dist/desktop/$TARGET"

case "$TARGET" in
    aarch64-apple-darwin|x86_64-apple-darwin)
        PLATFORM_CONFIG="src-tauri/tauri.macos.conf.json"
        BUNDLE_DIR="$TAURI_DIR/target/$TARGET/release/bundle"
        ;;
    x86_64-pc-windows-msvc)
        PLATFORM_CONFIG="src-tauri/tauri.windows.conf.json"
        BUNDLE_DIR="$TAURI_DIR/target/$TARGET/release/bundle"
        ;;
    x86_64-unknown-linux-gnu)
        PLATFORM_CONFIG="src-tauri/tauri.linux.conf.json"
        BUNDLE_DIR="$TAURI_DIR/target/$TARGET/release/bundle"
        ;;
    *)
        printf 'unsupported Desktop target: %s\n' "$TARGET" >&2
        exit 2
        ;;
esac

bash "$ROOT/scripts/ci/prepare-desktop-bundle.sh" "$TARGET"

if [[ "$MODE" == release && "$TARGET" != *linux* && -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    printf '%s\n' 'TAURI_SIGNING_PRIVATE_KEY is required to build updater artifacts' >&2
    exit 1
fi

if [[ -d "$BUNDLE_DIR" ]]; then
    find "$BUNDLE_DIR" -type f \
        \( -name '*.deb' -o -name '*.dmg' -o -name '*.tar.gz' -o -name '*.sig' \
           -o -name '*-setup.exe' -o -name '*.nsis.zip' \) \
        -delete
fi

(
    cd "$CONSOLE"
    EXTRA_CONFIG=()
    if [[ "$MODE" == --test-package ]]; then
        case "$TARGET" in
            *apple*) EXTRA_CONFIG=(--config '{"bundle":{"createUpdaterArtifacts":false,"macOS":{"signingIdentity":"-"}}}') ;;
            *windows*) EXTRA_CONFIG=(--config '{"bundle":{"createUpdaterArtifacts":false,"windows":{"signCommand":null}}}') ;;
        esac
    fi
    CI=true npx tauri build --ci --target "$TARGET" --config "$PLATFORM_CONFIG" "${EXTRA_CONFIG[@]}"
)

mkdir -p "$DIST"
find "$DIST" -maxdepth 1 -type f -delete

ARTIFACT_COUNT=0
while IFS= read -r artifact; do
    destination="$DIST/rua-$TARGET-$(basename "$artifact")"
    cp "$artifact" "$destination"
    printf 'collected: %s\n' "$destination"
    ARTIFACT_COUNT=$((ARTIFACT_COUNT + 1))
done < <(
    find "$BUNDLE_DIR" -type f \
        \( -name '*.deb' -o -name '*.dmg' -o -name '*.tar.gz' -o -name '*.sig' \
           -o -name '*-setup.exe' -o -name '*.nsis.zip' \) \
        ! -name 'rw.*' ! -name 'control.tar.gz' ! -name 'data.tar.gz' \
        | sort
)

EXPECTED_ARTIFACT_COUNT=3
if [[ "$MODE" == --test-package || "$TARGET" == *linux* ]]; then
    EXPECTED_ARTIFACT_COUNT=1
fi
if [[ "$ARTIFACT_COUNT" -ne "$EXPECTED_ARTIFACT_COUNT" ]]; then
    printf 'expected %d release artifacts under %s, found %d\n' \
        "$EXPECTED_ARTIFACT_COUNT" "$BUNDLE_DIR" "$ARTIFACT_COUNT" >&2
    exit 1
fi

printf 'Desktop artifacts: %s (%d files)\n' "$DIST" "$ARTIFACT_COUNT"
