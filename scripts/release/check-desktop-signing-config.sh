#!/usr/bin/env bash



set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONFIG="$ROOT/console/src-tauri/tauri.conf.json"
PUBLIC_KEY_FILE="$ROOT/console/src-tauri/updater.pub"
WORKFLOW="$ROOT/.github/workflows/release.yml"
REQUIRE_SECRETS=0

if [[ "${1:-}" == "--require-secrets" ]]; then
    REQUIRE_SECRETS=1
elif [[ $# -gt 0 ]]; then
    printf 'usage: %s [--require-secrets]\n' "$0" >&2
    exit 2
fi

SECRET_NAMES=(
    APPLE_CERTIFICATE
    APPLE_CERTIFICATE_PASSWORD
    APPLE_SIGNING_IDENTITY
    APPLE_API_ISSUER
    APPLE_API_KEY
    APPLE_API_KEY_CONTENT
    WINDOWS_CERTIFICATE
    WINDOWS_CERTIFICATE_PASSWORD
    WINDOWS_CERTIFICATE_THUMBPRINT
    TAURI_SIGNING_PRIVATE_KEY
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD
)

printf '%s\n' '--- Desktop signing contract ---'
python3 - "$CONFIG" "$PUBLIC_KEY_FILE" <<'PY'
import base64
import json
import pathlib
import sys

config_path = pathlib.Path(sys.argv[1])
public_path = pathlib.Path(sys.argv[2])
config = json.loads(config_path.read_text())
file_key = public_path.read_text().strip()
config_key = config.get("plugins", {}).get("updater", {}).get("pubkey", "").strip()
if not file_key or config_key != file_key:
    raise SystemExit("updater public key file and tauri.conf.json do not match")
if not config.get("bundle", {}).get("createUpdaterArtifacts"):
    raise SystemExit("bundle.createUpdaterArtifacts must be enabled")
endpoints = config.get("plugins", {}).get("updater", {}).get("endpoints", [])
if not endpoints or not all(value.startswith("https://") for value in endpoints):
    raise SystemExit("updater endpoints must contain at least one HTTPS URL")
decoded = base64.b64decode(file_key).decode("utf-8")
lines = decoded.splitlines()
if len(lines) < 2 or "minisign public key" not in lines[0]:
    raise SystemExit("updater.pub is not a Tauri minisign public key")
raw_key = base64.b64decode(lines[1])
if len(raw_key) < 10:
    raise SystemExit("updater public key payload is truncated")
print("updater public key: valid and aligned with tauri.conf.json")
PY

for name in "${SECRET_NAMES[@]}"; do
    if ! grep -q "secrets\\.${name}" "$WORKFLOW"; then
        printf 'workflow secret reference missing: %s\n' "$name" >&2
        exit 1
    fi
done
if ! grep -q 'vars\.TAURI_UPDATER_PUBLIC_KEY' "$WORKFLOW"; then
    printf '%s\n' 'workflow variable reference missing: TAURI_UPDATER_PUBLIC_KEY' >&2
    exit 1
fi
printf '%s\n' 'workflow secret names: valid'

if [[ -n "${TAURI_UPDATER_PUBLIC_KEY:-}" ]]; then
    python3 - "$PUBLIC_KEY_FILE" <<'PY'
import os
import pathlib
import sys

expected = pathlib.Path(sys.argv[1]).read_text().strip()
actual = os.environ["TAURI_UPDATER_PUBLIC_KEY"].strip()
if actual != expected:
    raise SystemExit("TAURI_UPDATER_PUBLIC_KEY does not match updater.pub")
print("repository updater public key variable: aligned")
PY
elif [[ "$REQUIRE_SECRETS" -eq 1 ]]; then
    printf '%s\n' 'missing repository variable: TAURI_UPDATER_PUBLIC_KEY' >&2
    exit 1
fi

missing=0
for name in "${SECRET_NAMES[@]}"; do
    if [[ -n "${!name:-}" ]]; then
        printf 'secret %-36s present\n' "$name"
    elif [[ "$REQUIRE_SECRETS" -eq 1 ]]; then
        printf 'secret %-36s MISSING\n' "$name" >&2
        missing=1
    else
        printf 'secret %-36s not checked in contract mode\n' "$name"
    fi
done
if [[ "$missing" -ne 0 ]]; then
    exit 1
fi

if [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "$tmp_dir"' EXIT
    printf '%s' 'rua updater signing alignment probe' > "$tmp_dir/probe.txt"
    (
        cd "$ROOT/console"
        npx tauri signer sign \
            --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
            --password "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
            "$tmp_dir/probe.txt" >/dev/null
    )
    python3 - "$PUBLIC_KEY_FILE" "$tmp_dir/probe.txt.sig" <<'PY'
import base64
import pathlib
import sys

def inner_key(path):
    outer = base64.b64decode(pathlib.Path(path).read_text().strip()).decode("utf-8")
    lines = outer.splitlines()
    if len(lines) < 2:
        raise SystemExit(f"invalid minisign payload: {path}")
    raw = base64.b64decode(lines[1])
    if len(raw) < 10:
        raise SystemExit(f"truncated minisign payload: {path}")
    return raw[2:10]

if inner_key(sys.argv[1]) != inner_key(sys.argv[2]):
    raise SystemExit("TAURI_SIGNING_PRIVATE_KEY does not match updater.pub")
print("updater private key: usable and aligned with committed public key")
PY
fi

printf '%s\n' 'desktop signing preflight: OK'
