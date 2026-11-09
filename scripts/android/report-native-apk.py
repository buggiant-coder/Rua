#!/usr/bin/env python3
"""Validate the shipped Android boundary and record reproducible APK payload identities."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import tempfile
import zipfile


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as output:
            json.dump(value, output, indent=2, ensure_ascii=False)
            output.write('\n'); output.flush(); os.fsync(output.fileno())
        os.replace(temporary, path)
        fd = os.open(path.parent, os.O_RDONLY)
        try: os.fsync(fd)
        finally: os.close(fd)
    finally:
        if os.path.exists(temporary): os.unlink(temporary)


def signer_sha256(certificate):
    digests = re.findall(r'^(?:Signer (?:#\d+|\([^\n]+\))|V[234] Signer:) certificate SHA-256 digest: ([0-9a-fA-F]{64})$',
                         certificate, re.MULTILINE)
    if not digests:
        raise ValueError('apksigner did not report a signer SHA-256 digest: ' + certificate)
    if len(set(value.lower() for value in digests)) != 1:
        raise ValueError('APK contains multiple signing certificates; report requires a single signer')
    return digests[0].lower()


def inspect(apk):
    sdk = Path(os.environ.get('ANDROID_HOME', os.environ.get('ANDROID_SDK_ROOT', '')))
    versions = sorted((sdk / 'build-tools').glob('*'), key=lambda p: tuple(int(v) for v in re.findall(r'\d+', p.name)))
    if not versions:
        raise ValueError('Set ANDROID_HOME to an SDK containing build-tools')
    badging = subprocess.check_output([str(versions[-1] / 'aapt2'), 'dump', 'badging', str(apk)], text=True)
    def field(pattern):
        match = re.search(pattern, badging)
        if not match: raise ValueError('APK metadata missing: ' + pattern)
        return match.group(1)
    certificate = subprocess.check_output([str(versions[-1] / 'apksigner'), 'verify', '--print-certs', str(apk)], text=True)
    with zipfile.ZipFile(apk) as archive:
        names = archive.namelist()
        forbidden = [name for name in names if any(word in name.lower() for word in ('bootstrap', 'runtime-overlay', 'rootfs.tar', 'node-v', 'installed-rootfs', 'migrate-legacy', 'legacy-ubuntu'))]
        if forbidden: raise ValueError('APK embeds userland: ' + repr(forbidden))
        for name in names:
            if re.fullmatch(r'classes[0-9]*\.dex', name):
                data = archive.read(name)
                if b'Lcom/termux/' in data or b'Lcom/rua/mobile/RuaDocumentsProvider;' in data:
                    raise ValueError('APK still contains Termux-derived classes: ' + name)
        expected = {'librua.so', 'libproot.so', 'libproot-loader.so', 'libtalloc.so', 'libandroid-shmem.so',
                    'librua-process-group.so', 'librua-runtime-gate.so'}
        native = []
        for entry in archive.infolist():
            if not entry.filename.startswith('lib/'): continue
            assert entry.filename.startswith('lib/arm64-v8a/'), entry.filename
            data = archive.read(entry)
            assert data[:4] == b'\x7fELF' and data[4] == 2 and struct.unpack_from('<H', data, 18)[0] == 183, entry.filename
            native.append({'file': entry.filename, 'bytes': entry.file_size, 'compressed_bytes': entry.compress_size,
                           'sha256': hashlib.sha256(data).hexdigest()})
        assert expected <= {Path(value['file']).name for value in native}, 'Missing native runtime artifact'
        assert 'librua-runtime-probe.so' not in {Path(value['file']).name for value in native}, 'Test probe leaked into production APK'
        resources = archive.read('assets/rua-ui.zip')
        assert hashlib.sha256(resources).hexdigest() == archive.read('assets/rua-ui.sha256').decode().strip(), 'Resource identity mismatch'
        manifest_bytes = archive.read('assets/runtime-manifest.json')
        manifest = json.loads(manifest_bytes)
        dependencies = json.loads(archive.read('assets/native-dependencies.json'))
        notices = [name for name in names if name.startswith('assets/licenses/')]
        assert len(notices) >= 4, 'Native dependency notices missing'
        members = [{'file': entry.filename, 'bytes': entry.file_size, 'compressed_bytes': entry.compress_size}
                   for entry in archive.infolist()]
    min_sdk, target_sdk = int(field(r"(?:minSdkVersion|sdkVersion):'(\d+)'")), int(field(r"targetSdkVersion:'(\d+)'"))
    assert min_sdk >= 26 and target_sdk >= 28, 'Declared platform below runtime requirements'
    return {'apk': str(apk.resolve()), 'bytes': apk.stat().st_size, 'sha256': hashlib.sha256(apk.read_bytes()).hexdigest(),
            'package': field(r"package: name='([^']+)'"), 'version_code': field(r"versionCode='([^']+)'"),
            'version_name': field(r"versionName='([^']+)'"), 'min_sdk': min_sdk, 'target_sdk': target_sdk,
            'abi': 'arm64-v8a', 'build_type': 'debug' if 'application-debuggable' in badging else 'release',
            'signer_sha256': signer_sha256(certificate),
            'signing_use': 'repository development key; preview/testing only',
            'native': native, 'native_dependencies': dependencies, 'license_files': notices,
            'resources_sha256': hashlib.sha256(resources).hexdigest(),
            'runtime_manifest_id': manifest['id'], 'runtime_manifest_sha256': hashlib.sha256(manifest_bytes).hexdigest(),
            'first_install_artifact_bytes': sum(item['size'] for item in [manifest['rootfs'], *manifest['packages'], *manifest['toolchains']]),
            'required_free_bytes': manifest['requiredFreeBytes'], 'members': members}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apk', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    report = inspect(args.apk)
    atomic_json(args.out, report)
    print(json.dumps({key: report[key] for key in ('bytes', 'sha256', 'min_sdk', 'target_sdk', 'abi', 'build_type', 'first_install_artifact_bytes')}, ensure_ascii=False))
