#!/usr/bin/env python3
"""Loopback-only npm registry serving a deterministic ACP runner for owned-device tests."""
import base64
import hashlib
import io
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tarfile
from urllib.parse import unquote

PACKAGE = "@agentclientprotocol/codex-acp"
VERSION = "1.1.4"
PORT = 18100
root = Path(__file__).resolve().parents[2]
manifest = {"name": PACKAGE, "version": VERSION, "bin": {"codex-acp": "runner.js"}}
buffer = io.BytesIO()
with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
    for name, content, mode in [
        ("package.json", json.dumps(manifest).encode(), 0o644),
        ("runner.js", (root / "mobile/mobile-shell/app/src/androidTest/assets/native-acp-fixture.js").read_bytes(), 0o755),
    ]:
        entry = tarfile.TarInfo("package/" + name)
        entry.size, entry.mode, entry.mtime = len(content), mode, 0
        archive.addfile(entry, io.BytesIO(content))
tarball = buffer.getvalue()
version = {**manifest, "dist": {"tarball": f"http://127.0.0.1:{PORT}/fixture.tgz", "integrity": "sha512-" + base64.b64encode(hashlib.sha512(tarball).digest()).decode()}}
metadata = json.dumps({"name": PACKAGE, "dist-tags": {"latest": VERSION}, "versions": {VERSION: version}}).encode()

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = unquote(self.path).split("?")[0]
        if path == "/fixture.tgz":
            data, mime = tarball, "application/octet-stream"
        elif path.rstrip("/") == "/" + PACKAGE:
            data, mime = metadata, "application/json"
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

if __name__ == "__main__":
    print(f"Owned-device ACP fixture registry on 127.0.0.1:{PORT}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
