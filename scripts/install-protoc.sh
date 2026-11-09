#!/usr/bin/env bash


set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/install-protoc.sh

Installs the Protocol Buffers compiler used by prost-build.

Android policy:
  Build the APK from macOS/Linux with scripts/android/build-android-native-runtime.sh.
  Debian guest development can use apt-get to install protobuf-compiler.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

if command -v protoc >/dev/null 2>&1; then
    protoc --version
    exit 0
fi

os="$(uname -s)"

if [[ "$os" == "Darwin" ]]; then
    if ! command -v brew >/dev/null 2>&1; then
        echo "ERROR: Homebrew is required to install protobuf on macOS." >&2
        exit 1
    fi
    brew install protobuf
elif [[ "$os" == "Linux" ]]; then
    if command -v apt-get >/dev/null 2>&1; then
        sudo apt-get update
        sudo apt-get install -y protobuf-compiler
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y protobuf-compiler
    elif command -v pacman >/dev/null 2>&1; then
        sudo pacman -S --needed protobuf
    elif command -v apk >/dev/null 2>&1; then
        sudo apk add protobuf
    else
        echo "ERROR: unsupported Linux package manager; install protoc and ensure it is in PATH." >&2
        exit 1
    fi
else
    echo "ERROR: unsupported OS: $os. Install protoc and ensure it is in PATH." >&2
    exit 1
fi

protoc --version
