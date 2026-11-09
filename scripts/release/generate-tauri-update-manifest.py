#!/usr/bin/env python3


from __future__ import annotations

import argparse
import datetime as dt
import json
from pathlib import Path
from urllib.parse import quote


PLATFORMS = {
    "aarch64-apple-darwin": ("darwin-aarch64", ".app.tar.gz"),
    "x86_64-apple-darwin": ("darwin-x86_64", ".app.tar.gz"),
    "x86_64-pc-windows-msvc": ("windows-x86_64", ".nsis.zip"),
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts", required=True, type=Path)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    version = args.tag.removeprefix("v")
    platforms: dict[str, dict[str, str]] = {}
    for target, (platform, suffix) in PLATFORMS.items():
        archive = single(args.artifacts, f"*{target}*{suffix}")
        signature_path = Path(f"{archive}.sig")
        if not signature_path.is_file():
            raise SystemExit(f"signature missing for {archive.name}")
        signature = signature_path.read_text().strip()
        if not signature:
            raise SystemExit(f"signature is empty: {signature_path}")
        url = (
            f"https://github.com/{args.repository}/releases/download/"
            f"{quote(args.tag, safe='')}/{quote(archive.name, safe='')}"
        )
        platforms[platform] = {"signature": signature, "url": url}

    payload = {
        "version": version,
        "notes": f"Rua Desktop {version}",
        "pub_date": dt.datetime.now(dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "platforms": platforms,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2) + "\n")


def single(root: Path, pattern: str) -> Path:
    matches = [path for path in root.rglob(pattern) if not path.name.endswith(".sig")]
    if len(matches) != 1:
        raise SystemExit(
            f"expected one artifact for {pattern}, found {len(matches)}: {matches}"
        )
    return matches[0]


if __name__ == "__main__":
    main()
