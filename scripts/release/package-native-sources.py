#!/usr/bin/env python3
"""Archive pinned Android native sources, recipes, notices and Rua transformations."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


def verified_download(url, digest, cache):
    if cache.exists():
        data = cache.read_bytes()
    else:
        with urllib.request.urlopen(url, timeout=120) as response:
            data = response.read()
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError(f'Source checksum mismatch: {url}')
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    manifest_path = ROOT / 'mobile/runtime/native-dependencies.json'
    manifest = json.loads(manifest_path.read_text())
    recipes = manifest['buildRecipes']
    cache = ROOT / 'target/native-source-inputs'
    recipe_data = verified_download(recipes['url'], recipes['sha256'], cache / 'termux-packages.tar.gz')
    sources = {'termux-packages.tar.gz': recipe_data}
    with tarfile.open(fileobj=io.BytesIO(recipe_data)) as archive:
        for package in manifest['packages']:
            name = package['Package']
            recipe_path = f"termux-packages-{recipes['commit']}/packages/{name}/build.sh"
            recipe = archive.extractfile(recipe_path).read().decode()
            version = re.search(r'^TERMUX_PKG_VERSION=[\"\']?([^\"\'\n]+)', recipe, re.MULTILINE)
            if not version or version.group(1) != package['Version'] or package['SourceSHA256'] not in recipe:
                raise ValueError(f'Build recipe does not match pinned package: {name}')
            suffix = '.zip' if package['SourceURL'].endswith('.zip') else '.tar.gz'
            filename = name + '-' + package['Version'] + suffix
            sources[filename] = verified_download(package['SourceURL'], package['SourceSHA256'], cache / filename)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=args.output.parent)
    try:
        with os.fdopen(fd, 'w+b') as output:
            with tarfile.open(fileobj=output, mode='w:gz') as archive:
                for filename, data in sources.items():
                    entry = tarfile.TarInfo('upstream/' + filename)
                    entry.mode = 0o644
                    entry.size = len(data)
                    archive.addfile(entry, io.BytesIO(data))
                for relative in ('mobile/runtime/native-dependencies.json',
                                 'mobile/runtime/licenses',
                                 'scripts/android/stage-native-payload.py',
                                 'docs/android-native-source-delivery.md'):
                    archive.add(ROOT / relative, arcname=relative)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, args.output)
        directory_fd = os.open(args.output.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    print(f'Packaged {len(sources)} verified source archives: {args.output}')


if __name__ == '__main__':
    main()
