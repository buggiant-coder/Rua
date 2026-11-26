#!/usr/bin/env bash


set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

BUILD_TYPE="release"
PLATFORM="${CARGO_NDK_PLATFORM:-}"

usage() {
    cat <<'EOF'
Usage: scripts/android/build-android-native-runtime.sh [--debug|--release] [-P|--platform API]

  --debug     Fast debug build for development APKs.
  --release   Optimized release-android build for production APKs. Default.
  -P, --platform API
              Android API level to link against (default: minSdkVersion from
              mobile/mobile-shell/gradle.properties, else 26; CARGO_NDK_PLATFORM
              is honoured first). cargo-ndk's own default is 21, which does not
              match minSdkVersion and silently lowers the recorded platform.
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
        -P|--platform)
            PLATFORM="${2:?missing API level after $1}"
            shift 2
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

# Fall back to the single source of truth for the minimum platform rather than
# hard-coding a number that can drift away from what the APK manifest declares.
if [[ -z "$PLATFORM" ]]; then
    PLATFORM="$(sed -n 's/^minSdkVersion=//p' mobile/mobile-shell/gradle.properties 2>/dev/null | head -1)"
    PLATFORM="${PLATFORM:-26}"
fi
if [[ ! "$PLATFORM" =~ ^[0-9]+$ ]]; then
    echo "ERROR: invalid API level: $PLATFORM" >&2
    exit 2
fi

if ! command -v cargo-ndk >/dev/null 2>&1; then
    echo "ERROR: cargo-ndk not found. Install: cargo install cargo-ndk"
    exit 1
fi

if ! rustup target list --installed | grep -q aarch64-linux-android; then
    rustup target add aarch64-linux-android
fi

# Honour a relocated target directory; CI and the Android PRoot environment
# both set CARGO_TARGET_DIR, and hard-coding target/ would then report a path
# that does not exist (and, under set -e, fail an otherwise good build).
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
export CARGO_TARGET_DIR

case "$BUILD_TYPE" in
    debug)
        cargo ndk -t arm64-v8a -P "$PLATFORM" build -p rua-cli --features mobile-default
        target="$CARGO_TARGET_DIR/aarch64-linux-android/debug/rua"
        ;;
    release)
        cargo ndk -t arm64-v8a -P "$PLATFORM" build -p rua-cli --features mobile-default --profile release-android
        target="$CARGO_TARGET_DIR/aarch64-linux-android/release-android/rua"
        ;;
    *)
        echo "ERROR: unsupported build type: $BUILD_TYPE" >&2
        exit 2
        ;;
esac

echo "built: $target (API $PLATFORM)"
if command -v file >/dev/null 2>&1; then
    file "$target"
fi

# cargo-ndk injects the platform through the linker environment, so a wrapper
# mismatch does not fail the build -- it silently lowers the recorded platform.
# Read back what the ELF actually claims; the note's first four description
# bytes are the API level as a little-endian uint32.
recorded_api() { # $1 = ELF path
    local hex
    hex="$("$readelf_bin" -n "$1" 2>/dev/null | awk '
        /notes found in: .note.android.ident/ { want = 1; next }
        want && /description data:/ {
            sub(/.*description data: */, "")
            n = split($0, byte, " ")
            for (i = 1; i <= 4 && i <= n; i++) printf "%s", byte[i]
            exit
        }')"
    [[ ${#hex} -eq 8 ]] || return 1
    printf '%d' "0x${hex:6:2}${hex:4:2}${hex:2:2}${hex:0:2}"
}

# Prefer llvm-readelf: binutils readelf -n omits the note's description bytes,
# which is exactly the field being checked here.
readelf_bin="$(command -v llvm-readelf || true)"
if [[ -z "$readelf_bin" && -d "${ANDROID_NDK_HOME:-}/toolchains" ]]; then
    # pipefail is set, so a find/head pipeline must not leak a non-zero status
    # when the toolchain directory simply does not hold the tool.
    readelf_bin="$(find "$ANDROID_NDK_HOME/toolchains" -name llvm-readelf -type f -perm -u+x 2>/dev/null | head -1 || true)"
fi
if [[ -f "$target" && -n "$readelf_bin" ]]; then
    if recorded="$(recorded_api "$target")"; then
        echo "recorded API: $recorded"
        if ((recorded != PLATFORM)); then
            echo "WARNING: $target records API $recorded, expected $PLATFORM" >&2
        fi
    fi
fi
ls -lh "$target"
