#!/usr/bin/env python3
"""SDK-free APK packaging and release contracts; never contact a real device."""
import json
import os
from pathlib import Path
import shutil
import runpy
import subprocess
import tempfile
import unittest
import zipfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


class AndroidReleaseContractTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rua android scripts ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.log = self.root / "calls.jsonl"
        self.env = {**os.environ, "RUA_SCRIPT_TEST_LOG": str(self.log),
                    "ANDROID_HOME": str(self.root / "sdk"), "ANDROID_SDK_ROOT": "",
                    "PATH": str(self.root / "bin") + os.pathsep + os.environ["PATH"]}
        self.script("scripts/android/report-native-apk.py", """import json, os, sys
with open(os.environ['RUA_SCRIPT_TEST_LOG'], 'a') as log:
    log.write(json.dumps(['verify', *sys.argv[1:]]) + '\\n')
sys.exit(int(os.environ.get('RUA_SCRIPT_TEST_VERIFY_EXIT', '0')))
""")

    def script(self, name, source):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        path.chmod(0o755)
        return path


    def calls(self, kind):
        if not self.log.exists():
            return []
        return [call for line in self.log.read_text().splitlines()
                if (call := json.loads(line))[0] == kind]






    def test_apk_signer_digest_supports_numbered_and_sdk_scoped_certificates(self):
        parse = runpy.run_path(str(ROOT / 'scripts/android/report-native-apk.py'))['signer_sha256']
        digest = 'ab' * 32
        for name in ('Signer #1', 'Signer (minSdkVersion=28, maxSdkVersion=2147483647)', 'V2 Signer:'):
            with self.subTest(name=name):
                self.assertEqual(parse(f'{name} certificate SHA-256 digest: {digest}\n'), digest)
        with self.assertRaisesRegex(ValueError, 'did not report'):
            parse('Source Stamp Signer certificate SHA-256 digest: ' + digest)
        with self.assertRaisesRegex(ValueError, 'multiple signing certificates'):
            parse(f'Signer #1 certificate SHA-256 digest: {digest}\n'
                  f'Signer #2 certificate SHA-256 digest: {"cd" * 32}\n')


    def test_apk_rejects_termux_classes_including_renamed_documents_provider(self):
        inspect = runpy.run_path(str(ROOT / 'scripts/android/report-native-apk.py'))['inspect']
        (self.root / 'sdk/build-tools/36.0.0').mkdir(parents=True)
        for marker in (b'Lcom/termux/app/TermuxService;', b'Lcom/rua/mobile/RuaDocumentsProvider;'):
            with self.subTest(marker=marker):
                apk = self.root / 'retained-termux.apk'
                with zipfile.ZipFile(apk, 'w') as archive:
                    archive.writestr('classes.dex', marker)
                with patch.dict(os.environ, self.env), patch('subprocess.check_output', return_value=''):
                    with self.assertRaisesRegex(ValueError, 'Termux'):
                        inspect(apk)


    def prepare_gate(self, contract_exit):
        gate = self.root / 'scripts/tests/release-gate.sh'
        gate.parent.mkdir(parents=True)
        shutil.copy2(ROOT / 'scripts/tests/release-gate.sh', gate)
        for name in ('clippy-touched', 'check-feature-flags', 'check-config-roundtrip', 'check-threat-model-coverage'):
            self.script('scripts/tests/' + name + '.sh', '#!/bin/sh\nexit 0\n')
        self.script('scripts/release/check-desktop-signing-config.sh', '#!/bin/sh\nexit 0\n')
        for name in ('check-desktop-release-contract', 'check-desktop-automation-staging'):
            self.script('scripts/tests/' + name + '.py', 'pass\n')
        self.script('scripts/tests/check-android-release-contract.py', f'raise SystemExit({contract_exit})\n')
        self.script('bin/cargo', '#!/bin/sh\nexit 0\n')
        return gate

    def test_release_gate_propagates_android_contract_failure(self):
        gate = self.prepare_gate(9)
        report = self.root / 'gate report.txt'
        result = subprocess.run(['bash', str(gate), '--report', str(report)],
                                cwd=self.root, env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('FAIL: android-release-contract', report.read_text())
        self.assertIn('Total: 10  Passed: 9  Failed: 1', report.read_text())

    def test_release_gate_checks_requested_apk_and_propagates_failure(self):
        gate = self.prepare_gate(0)
        for status in (0, 9):
            with self.subTest(apk_verifier_exit=status):
                self.env['RUA_SCRIPT_TEST_VERIFY_EXIT'] = str(status)
                result = subprocess.run(['bash', str(gate), '--android-apk', '/tmp/native app.apk'],
                                        cwd=self.root, env=self.env, text=True, capture_output=True)
                self.assertEqual(result.returncode, 0 if status == 0 else 1, result.stderr)
                self.assertIn('Results: 11/11 passed' if status == 0 else 'FAIL: android-apk-payload', result.stdout)
        self.assertEqual(self.calls('verify')[-1][1:3], ['--apk', '/tmp/native app.apk'])


    @unittest.skipIf(os.name == "nt", "Android packaging runs on Unix hosts")
    def test_packaging_locks_dependency_install_and_artifact_staging(self):
        for name in ("package-mobile-apk.sh", "with-console-build-lock.py"):
            shutil.copy2(ROOT / "scripts" / name, self.root / "scripts" / name)
        mobile = self.root / "mobile/mobile-shell"
        mobile.mkdir(parents=True)
        (mobile / "gradle.properties").write_text("ndkVersion=test\n")
        (self.root / "sdk").mkdir(exist_ok=True)
        for tool in ("java", "patchelf"):
            self.script("bin/" + tool, "#!/bin/sh\nexit 0\n")
        probe = """#!/usr/bin/env python3
import fcntl, json, os, sys
from pathlib import Path
root = Path(os.environ['RUA_SCRIPT_TEST_LOG']).parent
path = root / 'target/console-build.lock'
path.parent.mkdir(exist_ok=True)
with path.open('a+b') as lock:
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        held = False
    except BlockingIOError:
        held = True
with open(os.environ['RUA_SCRIPT_TEST_LOG'], 'a') as log:
    log.write(json.dumps(['console-lock', Path(sys.argv[0]).name, sys.argv[1:], held]) + '\\n')
if Path(sys.argv[0]).name == 'gradlew':
    apk = root / 'mobile/mobile-shell/app/build/outputs/apk/release/rua-release-arm64-v8a.apk'
    apk.parent.mkdir(parents=True, exist_ok=True)
    apk.write_bytes(b'fixture APK')
"""
        self.script("bin/npm", probe)
        self.script("mobile/mobile-shell/gradlew", probe)
        (self.root / "console").mkdir()
        result = subprocess.run(
            ["bash", str(self.root / "scripts/package-mobile-apk.sh"),
             "--release", "--skip-runtime-build", "--out-dir", str(self.root / "APK output")],
            cwd=self.root, env=self.env, text=True, capture_output=True, timeout=20,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = self.calls("console-lock")
        self.assertEqual([call[1] for call in calls], ["npm", "npm", "gradlew"])
        self.assertTrue(all(call[3] for call in calls), calls)
        self.assertTrue((self.root / "APK output/rua-release-arm64-v8a.apk").is_file())
        # Child failures propagate, and the OS lock is released for another build.
        wrapper = self.root / "scripts/with-console-build-lock.py"
        for status in (7, 0):
            completed = subprocess.run(["python3", str(wrapper), "bash", "-c", f"exit {status}"],
                                       env=self.env, timeout=5)
            self.assertEqual(completed.returncode, status)

    def test_native_packaging_shell_entrypoints_parse(self):
        for name in ('scripts/package-mobile-apk.sh', 'scripts/android/build-android-native-runtime.sh',
                     'scripts/tests/release-gate.sh'):
            with self.subTest(script=name):
                result = subprocess.run(['bash', '-n', str(ROOT / name)], text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_download_manifests_pin_debian_and_minimal_native_dependencies(self):
        manifest = json.loads((ROOT / 'mobile/runtime/runtime-manifest.json').read_text())
        self.assertTrue(manifest['id'].startswith('debian-'))
        self.assertEqual(manifest['architecture'], 'arm64')
        artifacts = [manifest['rootfs'], *manifest['packages'], *manifest['toolchains']]
        self.assertEqual(len({item['file'] for item in artifacts}), len(artifacts))
        for item in artifacts:
            self.assertEqual(Path(item['file']).name, item['file'])
            self.assertGreater(item['size'], 0)
            self.assertRegex(item['sha256'], r'^[0-9a-f]{64}$')
        for url in [manifest['rootfs']['url'], *manifest['packageSources'],
                    *(source['url'] for tool in manifest['toolchains'] for source in tool['sources'])]:
            self.assertTrue(url.startswith('https://'), url)
        native = json.loads((ROOT / 'mobile/runtime/native-dependencies.json').read_text())
        self.assertEqual({item['Package'] for item in native['packages']},
                         {'proot', 'libtalloc', 'libandroid-shmem'})
        for item in native['packages']:
            self.assertGreater(int(item['Size']), 0)
            self.assertRegex(item['SHA256'], r'^[0-9a-f]{64}$')
            self.assertTrue(item['SourceURL'].startswith('https://'))
            self.assertRegex(item['SourceSHA256'], r'^[0-9a-f]{64}$')


if __name__ == '__main__':
    unittest.main(verbosity=2)
