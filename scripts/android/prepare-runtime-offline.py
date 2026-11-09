#!/usr/bin/env python3
"""Download the APK-pinned upstream artifacts into a portable offline ZIP. No Docker needed."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import urllib.request
import zipfile
import os

spec = importlib.util.spec_from_file_location("stage", Path(__file__).with_name("stage-native-payload.py"))
stage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=Path("target/native-runtime-inputs"))
    parser.add_argument("--out", type=Path, default=Path("target/debian-runtime-offline.zip"))
    args = parser.parse_args()
    manifest = json.loads((Path(__file__).resolve().parents[2] / "mobile/runtime/runtime-manifest.json").read_text())
    for artifact in [manifest["rootfs"], *manifest["packages"], *manifest["toolchains"]]:
        file = args.cache / artifact["file"]
        if not file.is_file():
            if artifact is manifest["rootfs"]:
                token_url = "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/debian:pull"
                with urllib.request.urlopen(token_url, timeout=30) as response:
                    token = json.load(response)["token"]
                requests = [urllib.request.Request(artifact["url"], headers={"Authorization": "Bearer " + token})]
            elif "sources" in artifact:
                requests = [urllib.request.Request(source["url"]) for source in artifact["sources"]]
            else:
                requests = [urllib.request.Request(base + artifact["path"]) for base in manifest["packageSources"]]
            failure = None
            for request in requests:
                try:
                    with urllib.request.urlopen(request, timeout=90) as response:
                        data = response.read(artifact["size"] + 1)
                    if len(data) != artifact["size"] or hashlib.sha256(data).hexdigest() != artifact["sha256"]:
                        raise ValueError("Checksum mismatch: " + artifact["file"])
                    stage.atomic_bytes(file, data)
                    failure = None
                    break
                except Exception as error:
                    failure = error
            if failure:
                raise failure
        if file.stat().st_size != artifact["size"] or hashlib.sha256(file.read_bytes()).hexdigest() != artifact["sha256"]:
            raise ValueError("Invalid cached artifact: " + str(file))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.out.with_suffix(".tmp")
    try:
        with open(temporary, "w+b") as output:
            with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as archive:
                for artifact in [manifest["rootfs"], *manifest["packages"], *manifest["toolchains"]]:
                    entry = zipfile.ZipInfo(artifact["file"], (1980, 1, 1, 0, 0, 0))
                    archive.writestr(entry, (args.cache / artifact["file"]).read_bytes())
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, args.out)
        stage.sync_dir(args.out.parent)
    finally:
        temporary.unlink(missing_ok=True)
    print(f"Verified offline materials: {args.out} ({args.out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
