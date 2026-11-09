#!/usr/bin/env python3
"""Package dependency notices and MPL sources alongside a target's installer."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import urllib.request
import urllib.error
import zipfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]


def add_bytes(archive, name, data):
    entry = tarfile.TarInfo(name)
    entry.size = len(data)
    entry.mode = 0o644
    archive.addfile(entry, io.BytesIO(data))


def license_files(directory):
    for path in directory.rglob('*'):
        if path.is_file() and not path.is_symlink() and any(
            path.name.lower().startswith(prefix)
            for prefix in ('license', 'licence', 'copying', 'copyright', 'notice', 'ofl')
        ):
            yield path


def android_sources(archive, inventories):
    """Use Gradle's selected runtime graph, rather than the build-tool cache."""
    init = '''gradle.projectsEvaluated {
        def app = gradle.rootProject.project(':app')
        app.tasks.register('ruaReleaseDependencies') {
            doLast {
                ['debugRuntimeClasspath', 'coreLibraryDesugaring'].each { configuration ->
                    app.configurations.getByName(configuration).resolvedConfiguration.resolvedArtifacts.each { artifact ->
                        def id = artifact.moduleVersion.id
                        println("RUA_DEP|${id.group}|${id.name}|${id.version}|${artifact.file.absolutePath}")
                    }
                }
            }
        }
    }'''
    with tempfile.TemporaryDirectory() as temporary:
        script = Path(temporary) / 'inventory.gradle'
        with script.open('w') as output:
            output.write(init)
            output.flush()
            os.fsync(output.fileno())
        result = subprocess.check_output([
            str(ROOT / 'mobile/mobile-shell/gradlew'), '-p', str(ROOT / 'mobile/mobile-shell'),
            '--init-script', str(script), ':app:ruaReleaseDependencies', '--console=plain',
        ], cwd=ROOT, text=True)
    seen = set()
    for line in result.splitlines():
        if not line.startswith('RUA_DEP|'):
            continue
        _, group, name, version, filename = line.split('|', 4)
        identity = (group, name, version)
        if identity in seen:
            continue
        seen.add(identity)
        repository = ('https://dl.google.com/dl/android/maven2' if
                      group.startswith('androidx.') or group in ('com.google.android.material', 'com.android.tools')
                      else 'https://repo.maven.apache.org/maven2')
        base = f"{repository}/{group.replace('.', '/')}/{name}/{version}/{name}-{version}"
        source_url = base + '-sources.jar'
        source = None
        try:
            with urllib.request.urlopen(source_url, timeout=120) as response:
                source = response.read()
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
        source_suffix = '-sources.jar'
        if identity == ('com.android.tools', 'desugar_jdk_libs', '1.1.5'):
            source_url = 'https://codeload.github.com/google/desugar_jdk_libs/tar.gz/refs/tags/1.1.5'
            with urllib.request.urlopen(source_url, timeout=120) as response:
                source = response.read()
            if hashlib.sha256(source).hexdigest() != '0f18856d63188c04247dbdbcc54696c38cf732b05f57aa224eb2d69942626fb4':
                raise ValueError('desugar_jdk_libs source checksum mismatch')
            source_suffix = '-sources.tar.gz'
        with urllib.request.urlopen(base + '.pom', timeout=120) as response:
            pom = response.read()
        document = ET.fromstring(pom)
        licenses = [element.text for element in document.findall('.//{*}license/{*}name')]
        if source is None and any(any(term in value.lower() for term in ('mpl', 'gpl', 'general public', 'mozilla')) for value in licenses):
            raise RuntimeError(f'Matching sources required for {group}:{name}:{version}')
        inventories.append({'ecosystem': 'maven', 'name': group + ':' + name, 'version': version,
                            'licenses': licenses, 'source': source_url if source is not None else None,
                            'pom': base + '.pom',
                            'source_sha256': hashlib.sha256(source).hexdigest() if source is not None else None,
                            'artifact_sha256': hashlib.sha256(Path(filename).read_bytes()).hexdigest()})
        prefix = f'android/{group}/{name}-{version}'
        add_bytes(archive, prefix + '.pom', pom)
        if source is not None:
            add_bytes(archive, prefix + source_suffix, source)
        with zipfile.ZipFile(filename) as artifact:
            for member in artifact.infolist():
                if any(part.lower().startswith(('license', 'notice', 'copying', 'copyright'))
                       for part in Path(member.filename).parts):
                    add_bytes(archive, prefix + '/' + member.filename, artifact.read(member))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--desktop', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    inventories = []
    manifests = [ROOT / 'Cargo.toml']
    if args.desktop:
        manifests.append(ROOT / 'console/src-tauri/Cargo.toml')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=args.output.parent)
    try:
        with os.fdopen(fd, 'w+b') as output:
            with tarfile.open(fileobj=output, mode='w:gz') as archive:
                for name in ('LICENSE', 'LICENSES.md', 'NOTICE', 'THIRD_PARTY_NOTICES.md'):
                    archive.add(ROOT / name, arcname=name)
                seen = set()
                for manifest in manifests:
                    command = ['cargo', 'metadata', '--locked', '--format-version', '1',
                               '--filter-platform', args.target, '--manifest-path', str(manifest)]
                    if 'android' in args.target:
                        command += ['--features', 'rua-cli/mobile-default']
                    metadata = json.loads(subprocess.check_output(command, cwd=ROOT))
                    for package in metadata['packages']:
                        if package['id'] in seen:
                            continue
                        seen.add(package['id'])
                        directory = Path(package['manifest_path']).parent
                        name = package['name'] + '-' + package['version']
                        source = package.get('source')
                        source_url = (f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download"
                                      if source and source.startswith('registry+') else source)
                        inventories.append({k: package.get(k) for k in
                                            ('name', 'version', 'license', 'authors', 'repository')}
                                           | {'ecosystem': 'cargo', 'source': source_url,
                                              'workspace': str(manifest.relative_to(ROOT))})
                        if directory.is_relative_to(ROOT):
                            continue  # Included in the matching Rua source archive.
                        notices = list(license_files(directory))
                        for path in notices:
                            archive.add(path, arcname=f'cargo/{name}/{path.relative_to(directory)}')
                        mpl = 'MPL-2.0' in (package.get('license') or '')
                        if mpl or not notices:
                            source_group = 'mpl-sources' if mpl else 'additional-sources'
                            archive.add(directory, arcname=f'{source_group}/{name}',
                                        filter=lambda entry: None if entry.issym() or entry.islnk() else entry)
                lock = json.loads((ROOT / 'console/package-lock.json').read_text())
                for location, package in lock['packages'].items():
                    if not location or package.get('dev'):
                        continue
                    directory = ROOT / 'console' / location
                    if not directory.is_dir():
                        raise RuntimeError(f'Installed npm package missing: {location}')
                    name = location.rsplit('node_modules/', 1)[-1]
                    inventories.append({'ecosystem': 'npm', 'name': name,
                                        'version': package['version'], 'license': package.get('license'),
                                        'source': package.get('resolved'), 'integrity': package.get('integrity')})
                    notices = [path for path in license_files(directory)
                               if 'node_modules' not in path.relative_to(directory).parts]
                    for path in notices:
                        archive.add(path, arcname=f'npm/{location}/{path.relative_to(directory)}')
                    if not notices:
                        archive.add(directory, arcname=f'additional-sources/npm/{location}',
                                    filter=lambda entry: None if entry.issym() or entry.islnk() else entry)
                font = ROOT / 'console/src/assets/fonts'
                if font.exists():
                    for path in font.glob('*.txt'):
                        archive.add(path, arcname=f'fonts/{path.relative_to(font)}')
                if 'android' in args.target:
                    android_sources(archive, inventories)
                add_bytes(archive, 'dependency-inventory.json',
                          json.dumps({'target': args.target,
                                      'scope': 'Conservative resolved Cargo inventory (includes build/dev dependencies) and production npm and selected Android Maven runtime dependencies; not a claim that every entry ships.',
                                      'packages': inventories}, indent=2).encode())
                add_bytes(archive, 'README.txt',
                          b'Rua dependency notices\n\nThis archive accompanies the installer for the target named in dependency-inventory.json.\nCargo and npm license texts retain their component paths. Matching MPL-2.0 crate sources\nare in mpl-sources/. Rua-owned and locally modified vendored sources are in the\nGitHub Source code downloads provided in the same release. Source archive URLs and versions are recorded in the inventory.\nIndependent automation program notices are also included in the installed automation resources.\n')
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, args.output)
        # Windows does not expose directory fsync through Python's os module.
        if os.name == 'posix':
            directory_fd = os.open(args.output.parent, os.O_RDONLY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    print(f'Packaged {len(inventories)} dependency records: {args.output}')


if __name__ == '__main__':
    main()
