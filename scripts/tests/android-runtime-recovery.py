#!/usr/bin/env python3
"""Kill/reopen the real installer at durable boundaries on the owned acceptance AVD."""
import argparse
import json
import os
import tempfile
import pathlib
import shlex
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--adb', default=str(pathlib.Path.home() / 'Library/Android/sdk/platform-tools/adb'))
parser.add_argument('--serial', default='emulator-5556')
parser.add_argument('--phases', nargs='+', default=['download', 'unpacking', 'probing', 'before_activation', 'after_activation'])
parser.add_argument('--logs', default='target/android-runtime-recovery')
args = parser.parse_args()
adb = [args.adb, '-s', args.serial]
package = 'com.rua.mobile'
runner = package + '.test/androidx.test.runner.AndroidJUnitRunner'
test_class = package + '.RuntimeRecoveryDeviceTest'
logs = pathlib.Path(args.logs)
logs.mkdir(parents=True, exist_ok=True)

def command(parts, check=True):
    return subprocess.run(adb + parts, check=check, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

def shell(parts, check=True):
    return command(['shell', shlex.join(parts)], check)

def app(parts, check=True):
    return shell(['run-as', package] + parts, check)

def fixture_text(path, text):
    temporary = path + '.tmp'
    script = 'printf %s ' + shlex.quote(text) + ' > ' + shlex.quote(temporary)
    script += ' && sync ' + shlex.quote(temporary)
    script += ' && mv ' + shlex.quote(temporary) + ' ' + shlex.quote(path)
    script += ' && sync ' + shlex.quote(str(pathlib.PurePosixPath(path).parent))
    app(['sh', '-c', script])

def atomic_text(path, text):
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix='.' + path.name)
    try:
        with os.fdopen(fd, 'w') as output:
            output.write(text)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        parent = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(parent)
        finally:
            os.close(parent)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)

avd = command(['emu', 'avd', 'name']).stdout.splitlines()[0]
if avd not in ('rua_native_e2e', 'rua_native_api26_e2e', 'rua_native_api28_e2e'):
    raise SystemExit('This fault harness only operates on named disposable native acceptance AVDs')
allowed = {'download', 'unpacking', 'probing', 'before_activation', 'after_activation'}
if not set(args.phases) <= allowed:
    raise SystemExit('Unknown fault phase')
shell(['am', 'force-stop', package])
run_id = str(int(time.time()))
baseline = 'files/runtime/debian.recovery-baseline-' + run_id
app(['mv', 'files/runtime/debian', baseline])
# These are explicit fixture files outside the replaceable guest.
app(['mkdir', '-p', 'files/runtime/foreign-staging', 'files/home', 'files/runtime-test'])
fixture_text('files/home/recovery-test-data', 'workspace survives')
fixture_text('files/runtime/foreign-staging/keep', 'not owned')
marker = 'files/runtime-test/fault-stage.json'
for index, phase in enumerate(args.phases):
    if index and args.phases[index - 1] != 'download':
        owner = app(['cat', 'files/runtime/debian/.rua-recovery-fixture']).stdout
        if owner != run_id:
            raise SystemExit('Refusing to remove a guest not created by this fault run')
        app(['rm', '-rf', 'files/runtime/debian'])
    app(['rm', '-f', marker])
    pause_method = package + '.RuntimeDownloadRecoveryDeviceTest#pauseTransferForExternalKill' if phase == 'download' else test_class + '#pauseInstallationForExternalProcessKill'
    resume_method = package + '.RuntimeDownloadRecoveryDeviceTest#recoverTransferAfterProcessKill' if phase == 'download' else test_class + '#resumeAfterProcessKillWithoutTouchingWorkspaceOrForeignStaging'
    with (logs / (phase + '-kill.log')).open('w') as output:
        process = subprocess.Popen(adb + ['shell', 'am', 'instrument', '-w', '-e', 'class', pause_method, '-e', 'fault_phase', phase, runner], stdout=output, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 240
        while True:
            result = app(['cat', marker], check=False)
            if result.returncode == 0 and json.loads(result.stdout)['phase'] == phase:
                break
            if process.poll() is not None or time.monotonic() > deadline:
                raise SystemExit('Fault stage was not reached: ' + phase)
            time.sleep(1)
        print('Killing real app process during ' + phase, flush=True)
        shell(['am', 'force-stop', package])
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    result = command(['shell', 'am', 'instrument', '-w', '-e', 'class', resume_method, runner])
    atomic_text(logs / (phase + '-resume.log'), result.stdout)
    if 'OK (1 test)' not in result.stdout:
        raise SystemExit('Recovery failed at ' + phase + ': ' + result.stdout[-3000:])
    if phase != 'download':
        fixture_text('files/runtime/debian/.rua-recovery-fixture', run_id)
    print('PASS real process recovery: ' + phase, flush=True)
print('All requested cuts passed; original fixture guest preserved at ' + baseline, flush=True)
