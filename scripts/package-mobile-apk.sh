#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$REPO_ROOT"
ORIGINAL_ARGS=("$@")
BUILD_TYPE=debug
BUILD_RUNTIME=1
BUILD_CONSOLE=1
CLEAN=0
INSTALL=0
INSTALL_ALL=0
OUT_DIR="$REPO_ROOT/target/mobile-apk"
PORT="${RUA_MOBILE_PORT:-0}"
SERIALS=()
usage() {
    cat <<'EOF'
Usage: scripts/package-mobile-apk.sh [options]
  --debug | --release       APK build type (default debug).
  --skip-runtime-build      Reuse the matching Android rua build.
  --skip-console-build      Reuse console/dist.
  --clean                   Clean Gradle outputs before packaging.
  --out-dir PATH            Output APK and payload report directory.
  --port PORT               In-app server port (default 0: OS-assigned available port).
  --install                 Install to the only connected device or explicit serials.
  --install-all             Install to all connected adb devices.
  -s, --serial SERIAL       Explicit install target; repeatable.
  -h, --help                Show this help.

Requires JDK 17, Android SDK/NDK, Rust + cargo-ndk, Node/npm, Python 3 and patchelf.
The APK contains native Rua, WebUI and PRoot dependencies. Debian and language
runtimes are downloaded automatically on first launch; no bootstrap is required.
EOF
}
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug) BUILD_TYPE=debug; shift ;;
        --release) BUILD_TYPE=release; shift ;;
        --skip-runtime-build) BUILD_RUNTIME=0; shift ;;
        --skip-console-build) BUILD_CONSOLE=0; shift ;;
        --clean) CLEAN=1; shift ;;
        --out-dir) OUT_DIR="${2:?missing output directory}"; shift 2 ;;
        --port) PORT="${2:?missing port}"; shift 2 ;;
        --install) INSTALL=1; shift ;;
        --install-all) INSTALL=1; INSTALL_ALL=1; shift ;;
        -s|--serial) INSTALL=1; SERIALS+=("${2:?missing serial}"); shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done
if [[ ! "$PORT" =~ ^[0-9]+$ ]] || ((10#$PORT < 0 || 10#$PORT > 65535)); then
    die 'invalid port'
fi
PORT=$((10#$PORT))
for tool in java python3 patchelf; do command -v "$tool" >/dev/null || die "$tool is required"; done
ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[[ -d "$ANDROID_HOME" ]] || die 'Set ANDROID_HOME to the Android SDK'
NDK_VERSION=$(sed -n 's/^ndkVersion=//p' mobile/mobile-shell/gradle.properties)
export ANDROID_HOME
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/$NDK_VERSION}"
export RUA_MOBILE_PORT="$PORT"
if ((BUILD_RUNTIME)); then scripts/android/build-android-native-runtime.sh "--$BUILD_TYPE"; fi
# Rust can build alongside Desktop; shared Console work and APK staging cannot.
if [[ "${RUA_CONSOLE_BUILD_LOCK:-}" != "$REPO_ROOT/target/console-build.lock" ]]; then
    exec python3 "$REPO_ROOT/scripts/with-console-build-lock.py" bash "$0" "${ORIGINAL_ARGS[@]}" --skip-runtime-build
fi
if ((BUILD_CONSOLE)); then
    (cd console && npm ci && npm run build)
fi
TASK=assembleDebug
[[ "$BUILD_TYPE" == release ]] && TASK=assembleRelease
if ((CLEAN)); then mobile/mobile-shell/gradlew -p mobile/mobile-shell clean; fi
SOURCE="$REPO_ROOT/mobile/mobile-shell/app/build/outputs/apk/$BUILD_TYPE/rua-$BUILD_TYPE-arm64-v8a.apk"
# A fresh ZIP avoids dead space left by incremental APK replacement after large ELF changes.
rm -f "$SOURCE"
mobile/mobile-shell/gradlew -p mobile/mobile-shell ":app:$TASK"
[[ -f "$SOURCE" ]] || die "APK missing: $SOURCE"
mkdir -p "$OUT_DIR"
APK="$OUT_DIR/rua-$BUILD_TYPE-arm64-v8a.apk"
python3 - "$SOURCE" "$APK" <<'COPY'
import os, shutil, sys, tempfile
from pathlib import Path
source, target = map(Path, sys.argv[1:])
fd, temporary = tempfile.mkstemp(dir=target.parent)
try:
    with os.fdopen(fd, 'wb') as output, source.open('rb') as input:
        shutil.copyfileobj(input, output); output.flush(); os.fsync(output.fileno())
    os.replace(temporary, target)
    fd = os.open(target.parent, os.O_RDONLY)
    try: os.fsync(fd)
    finally: os.close(fd)
finally:
    if os.path.exists(temporary): os.unlink(temporary)
COPY
python3 scripts/android/report-native-apk.py --apk "$APK" --out "$OUT_DIR/rua-$BUILD_TYPE-payload.json"
if ((INSTALL)); then
    ADB="$ANDROID_HOME/platform-tools/adb"
    if ((${#SERIALS[@]} == 0)); then
        while IFS= read -r serial; do [[ -z "$serial" ]] || SERIALS+=("$serial"); done < <("$ADB" devices | awk 'NR>1 && $2=="device" {print $1}')
        ((${#SERIALS[@]} > 0)) || die 'no connected device'
        ((INSTALL_ALL || ${#SERIALS[@]} == 1)) || die 'multiple devices: specify --serial or --install-all'
    fi
    for serial in "${SERIALS[@]}"; do "$ADB" -s "$serial" install -r "$APK"; done
fi
printf 'APK: %s\n' "$APK"
