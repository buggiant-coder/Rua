#!/usr/bin/env bash
set -euo pipefail

INSTALLERS="$(cd "${1:-target/dist/desktop/x86_64-unknown-linux-gnu}" && pwd)"
sudo apt-get update -qq
sudo apt-get install -y xvfb dbus-x11 "$INSTALLERS"/*.deb
rua-desktop --verify-desktop-bundle
export RUA_DESKTOP_ROOT
RUA_DESKTOP_ROOT="$(mktemp -d "${RUNNER_TEMP:-/tmp}/rua-linux-preview.XXXXXX")"
mkdir -p target
set +e
xvfb-run -a dbus-run-session -- env WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 timeout 45 rua-desktop > target/linux-desktop-startup.log 2>&1
status=$?
set -e
echo "Desktop startup exit status: $status (124 means the timeout stopped a running app)"
cat target/linux-desktop-startup.log
if test -f "$RUA_DESKTOP_ROOT/logs/startup.log"; then
    tail -n 150 "$RUA_DESKTOP_ROOT/logs/startup.log"
fi
test "$status" -eq 124
test -s "$RUA_DESKTOP_ROOT/desktop-endpoint.json"
python3 - <<'PYTHON'
import json, os, pathlib, tomllib
endpoint = json.loads((pathlib.Path(os.environ['RUA_DESKTOP_ROOT']) / 'desktop-endpoint.json').read_text())
version = tomllib.loads(pathlib.Path('Cargo.toml').read_text())['workspace']['package']['version']
assert endpoint['rua_version'] == version, endpoint
assert endpoint['host'] == '127.0.0.1' and endpoint['port'] > 0, endpoint
print('Installed Desktop Runtime started:', endpoint)
PYTHON
