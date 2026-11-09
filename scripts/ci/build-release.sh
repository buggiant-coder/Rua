#!/usr/bin/env bash
set -euo pipefail
TARGET="${1:?Usage: build-release.sh <linux-target-triple>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
case "$TARGET" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
    *) printf 'unsupported Linux target: %s\n' "$TARGET" >&2; exit 2 ;;
esac
cd "$ROOT"
cargo build --locked --release -p rua-cli --target "$TARGET"
(cd console && npm ci && npm run build)
VERSION=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
mkdir -p target/dist
STAGING=$(mktemp -d)
trap 'rm -rf "$STAGING"' EXIT
install -m 755 "target/$TARGET/release/rua" "$STAGING/rua"
cp -R console/dist "$STAGING/console"
cp LICENSE LICENSES.md NOTICE THIRD_PARTY_NOTICES.md README.md "$STAGING/"
cp docs/getting-started.zh-CN.md "$STAGING/GETTING_STARTED.zh-CN.md"
tar -czf "target/dist/rua-$VERSION-$TARGET.tar.gz" -C "$STAGING" .
printf 'Linux headless archive: target/dist/rua-%s-%s.tar.gz\n' "$VERSION" "$TARGET"
