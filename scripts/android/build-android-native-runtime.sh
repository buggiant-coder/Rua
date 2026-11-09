#!/usr/bin/env bash


set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

BUILD_TYPE="release"

usage() {
    cat <<'EOF'
Usage: scripts/android/build-android-native-runtime.sh [--debug|--release]

  --debug     Fast debug build for development APKs.
  --release   Optimized release-android build for production APKs. Default.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug)
            BUILD_TYPE="debug"
            shift
            ;;
        --release)
            BUILD_TYPE="release"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME not set; install Android NDK or set var}"

if ! command -v cargo-ndk >/dev/null 2>&1; then
    echo "ERROR: cargo-ndk not found. Install: cargo install cargo-ndk"
    exit 1
fi

if ! rustup target list --installed | grep -q aarch64-linux-android; then
    rustup target add aarch64-linux-android
fi

case "$BUILD_TYPE" in
    debug)
        cargo ndk -t arm64-v8a build -p rua-cli --features mobile-default
        target="target/aarch64-linux-android/debug/rua"
        ;;
    release)
        cargo ndk -t arm64-v8a build -p rua-cli --features mobile-default --profile release-android
        target="target/aarch64-linux-android/release-android/rua"
        ;;
    *)
        echo "ERROR: unsupported build type: $BUILD_TYPE" >&2
        exit 2
        ;;
esac

echo "built: $target"
if command -v file >/dev/null 2>&1; then
    file "$target"
fi
ls -lh "$target"
