#!/usr/bin/env python3
"""Combined native/guest acceptance on a disposable named ARM64 AVD; never a user's phone."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = 'com.rua.mobile'
RUNNER = PACKAGE + '.test/androidx.test.runner.AndroidJUnitRunner'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--adb', default='adb')
    parser.add_argument('--apk', type=Path, default=ROOT / 'target/mobile-apk/rua-debug-arm64-v8a.apk')
    parser.add_argument('--test-apk', type=Path, default=ROOT / 'mobile/mobile-shell/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk')
    parser.add_argument('--offline', type=Path, default=ROOT / 'target/debian-runtime-offline.zip')
    parser.add_argument('--logs', type=Path, default=ROOT / 'target/android-native-runtime-e2e')
    parser.add_argument('--reset-owned', action='store_true', help='Explicitly clear only this named disposable test application before clean acceptance')
    parser.add_argument('--reuse-fixture-servers', action='store_true', help='Use already running local model/registry fixtures on 18099/18100')
    parser.add_argument('--suite', choices=['all', 'runtime', 'host'], default='all')
    args = parser.parse_args()
    os.chdir(ROOT)
    adb = [args.adb, '-s', args.serial]
    def command(parts, **kwargs):
        return subprocess.check_output(adb + parts, **kwargs)
    def shell(parts, **kwargs):
        return command(['shell', shlex.join(parts)], **kwargs)
    avd = command(['emu', 'avd', 'name'], text=True).splitlines()[0]
    if avd not in ('rua_native_e2e', 'rua_native_api26_e2e', 'rua_native_api28_e2e'):
        raise SystemExit('Refusing: use a dedicated rua_native[_api26/_api28]_e2e AVD')
    if args.suite == 'all' and not args.reset_owned:
        parser.error('The clean suite requires --reset-owned on this disposable AVD')
    for path in (args.apk, args.test_apk, args.offline):
        if not path.is_file(): raise SystemExit('Missing input: ' + str(path))
    args.logs.mkdir(parents=True, exist_ok=True)
    spec = importlib.util.spec_from_file_location('stage', ROOT / 'scripts/android/stage-native-payload.py')
    stage = importlib.util.module_from_spec(spec); spec.loader.exec_module(stage)
    def save(name, text): stage.atomic_bytes(args.logs / name, text.encode())
    results, children, handles = [], [], []
    sdk = int(shell(['getprop', 'ro.build.version.sdk'], text=True).strip())
    def network(enabled):
        # The API 26 Google image does not grant CHANGE_WIFI_STATE to adb shell.
        # Elevate only the emulator network control, never the app or its guest.
        prefix = ['su', '0'] if sdk == 26 else []
        for transport in ('wifi', 'data'):
            shell(prefix + ['svc', transport, 'enable' if enabled else 'disable'])
    report = {'avd': avd, 'serial': args.serial, 'sdk': sdk, 'suite': args.suite,
              'android_fingerprint': shell(['getprop', 'ro.build.fingerprint'], text=True).strip(),
              'webview_provider': shell(['dumpsys', 'webviewupdate'], text=True),
              'apk_sha256': hashlib.sha256(args.apk.read_bytes()).hexdigest(),
              'test_apk_sha256': hashlib.sha256(args.test_apk.read_bytes()).hexdigest(), 'steps': results}
    def record(): save('result.json', json.dumps(report, indent=2))
    def test(label, selection, timeout=900):
        start = time.monotonic()
        selection = ','.join(PACKAGE + '.' + value for value in selection.split(','))
        try:
            output = subprocess.run(adb + ['shell', 'am', 'instrument', '-w', '-e', 'class', selection, RUNNER],
                                    capture_output=True, text=True, timeout=timeout)
            save(label + '.log', output.stdout + output.stderr)
            passed = output.returncode == 0 and re.search(r'OK \(\d+ tests?\)', output.stdout) is not None
            results.append({'step': label, 'result': 'pass' if passed else 'fail', 'seconds': round(time.monotonic() - start, 2)})
            record()
            if not passed: raise RuntimeError(label + ': ' + (output.stdout + output.stderr)[-3500:])
            print('PASS ' + label, flush=True)
        except subprocess.TimeoutExpired:
            shell(['am', 'force-stop', PACKAGE])
            results.append({'step': label, 'result': 'timeout'}); record(); raise
    def push(source, name):
        shell(['run-as', PACKAGE, 'mkdir', '-p', 'files/runtime-test'])
        script = f'cat > files/runtime-test/{name}.tmp && sync files/runtime-test/{name}.tmp && mv files/runtime-test/{name}.tmp files/runtime-test/{name} && sync files/runtime-test'
        with source.open('rb') as data:
            subprocess.run(adb + ['shell', shlex.join(['run-as', PACKAGE, 'sh', '-c', script])], stdin=data, check=True)
    try:
        if not args.reuse_fixture_servers:
            for script in ('android-native-runtime-model.py', 'android-runtime-acp-registry.py'):
                output = (args.logs / (script + '.log')).open('wb'); handles.append(output)
                children.append(subprocess.Popen([sys.executable, str(ROOT / 'scripts/tests' / script)], stdout=output, stderr=subprocess.STDOUT))
            time.sleep(1)
            if any(child.poll() is not None for child in children): raise RuntimeError('Fixture server failed; see logs')
        for port in (18099, 18100): command(['reverse', f'tcp:{port}', f'tcp:{port}'])
        command(['install', '-r', str(args.apk)]); command(['install', '-r', str(args.test_apk)])
        if args.suite == 'all':
            shell(['pm', 'clear', PACKAGE])
            if sdk >= 33: shell(['pm', 'grant', PACKAGE, 'android.permission.POST_NOTIFICATIONS'])
            network(False)
            try: test('01-native-cold-start', 'NativeResidentDeviceTest')
            finally:
                shell(['am', 'force-stop', PACKAGE])
                shell(['pm', 'clear', PACKAGE])
                if sdk >= 33: shell(['pm', 'grant', PACKAGE, 'android.permission.POST_NOTIFICATIONS'])
                network(True)
            test('02-automatic-online-install', 'AutomaticRuntimeDeviceTest', 900)
            save('installation.json', shell(['run-as', PACKAGE, 'cat', 'files/runtime-test/first-install.json'], text=True))
            save('installed-usage.txt', shell(['run-as', PACKAGE, 'du', '-sk', 'files/runtime/debian', 'files/guest-home', 'files/runtime/downloads', 'files/app-resources'], text=True))
        if args.suite != 'host':
            subprocess.run([sys.executable, 'scripts/tests/android-runtime-skills.py'], check=True)
            push(ROOT / 'target/android-runtime-skills.zip', 'skills.zip'); push(args.offline, 'offline.zip')
            test('03-agent-and-skills', 'NativeAgentDeviceTest')
            test('04-mcp-acp-plugin', 'NativeMcpDeviceTest,NativeAcpDeviceTest,NativeToolProbeDeviceTest')
            test('05-terminal', 'NativeTerminalDeviceTest', 900)
            test('06-runtime-locks', 'RuntimeGateDeviceTest,NativeProcessGroupDeviceTest')
            command(['install', '-r', str(args.apk)])
            test('07-apk-reuses-environment', 'RuntimeUpdateDeviceTest#coveringApkReusesCompatibleEnvironmentWithZeroNetworkBytes')
            test('08-offline-and-tool-recovery', 'LinuxRuntimeDeviceTest,RuntimeToolchainsDeviceTest')
            test('09-storage-exhaustion', 'RuntimeRecoveryDeviceTest#spaceFailuresPreserveDataAndOwnedCleanupDoesNotRemoveForeignDirectories')
            start = time.monotonic()
            with (args.logs / 'installation-cuts.log').open('wb') as output:
                cut = subprocess.run([sys.executable, 'scripts/tests/android-runtime-recovery.py', '--adb', args.adb,
                                      '--serial', args.serial, '--logs', str(args.logs / 'installation-cuts')],
                                     stdout=output, stderr=subprocess.STDOUT, timeout=1500)
            results.append({'step': '10-installation-kill-recovery', 'result': 'pass' if cut.returncode == 0 else 'fail',
                            'seconds': round(time.monotonic() - start, 2)})
            record()
            if cut.returncode: raise RuntimeError('Installation recovery failed; see installation-cuts.log')
            test('11-restore-tools-after-missing-guest', 'RuntimeToolchainsDeviceTest#firstLaunchAutomaticallyAddsNodeAndUvWithoutReplacingDebian')
        if args.suite != 'runtime':
            test('12-terminal-webview', 'NativeTerminalWebViewDeviceTest')
            test('13-host-power', 'NativeHostDeviceTest#nativeHostAppliesRealLifecyclePowerPolicyAndNetworkFacts')
            if sdk >= 30:
                test('14-phone-and-storage', 'NativeHostDeviceTest#realAgentUsesPhoneBridgeAndRevocationDoesNotStopResident,NativeStorageDeviceTest')
            else:
                results.append({'step': '14-phone-and-storage', 'result': 'not-run', 'reason': 'This fixture uses API 30 screenshot / API 29 MediaStore; requires API 34 lane'})
                record()
        print('Evidence: ' + str(args.logs / 'result.json'), flush=True)
    finally:
        save('logcat.txt', command(['logcat', '-d', '-t', '2500'], text=True))
        for child in children:
            child.terminate()
            try: child.wait(timeout=5)
            except subprocess.TimeoutExpired: child.kill(); child.wait()
        for output in handles: output.close()


if __name__ == '__main__':
    main()
