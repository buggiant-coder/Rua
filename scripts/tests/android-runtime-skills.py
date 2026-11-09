#!/usr/bin/env python3
"""Build deterministic Skill API fixtures with real local wheel/npm dependency installation."""
import io
import json
from pathlib import Path
import zipfile
import importlib.util


def archive(files):
    output = io.BytesIO()
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as bundle:
        for name, content in files.items():
            bundle.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), content)
    return output.getvalue()


wheel = archive({
    'rua_fixture_dependency/__init__.py': 'VALUE = "PYTHON_DEPENDENCY_OK"\n',
    'rua_fixture_dependency-1.0.dist-info/METADATA': 'Metadata-Version: 2.1\nName: rua-fixture-dependency\nVersion: 1.0\n',
    'rua_fixture_dependency-1.0.dist-info/WHEEL': 'Wheel-Version: 1.0\nGenerator: rua-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
    'rua_fixture_dependency-1.0.dist-info/RECORD': '',
})
files = {}
for name, language in [('native-python-fixture', 'Python'), ('native-node-fixture', 'Node')]:
    files[f'{name}/SKILL.md'] = f'---\nname: {name}\ndescription: Deterministic {language} runtime acceptance fixture.\n---\nRun `bash scripts/run.sh` from this skill directory, with RUA_FIXTURE_WORKSPACE set to the canonical workspace.\n'
files.update({
    'native-python-fixture/requirements.txt': './deps/rua_fixture_dependency-1.0-py3-none-any.whl\n',
    'native-python-fixture/deps/rua_fixture_dependency-1.0-py3-none-any.whl': wheel,
    'native-python-fixture/scripts/run.sh': '''#!/bin/bash
set -eu
uv venv --python /usr/bin/python3 .venv
uv pip install --no-index --python .venv/bin/python -r requirements.txt
export PATH="$PWD/.venv/bin:$PATH"
chmod +x scripts/main.py
scripts/main.py
''',
    'native-python-fixture/scripts/main.py': '''#!/usr/bin/env python
import json, os, pathlib, subprocess, sys
from rua_fixture_dependency import VALUE
assert sys.platform == 'linux' and VALUE == 'PYTHON_DEPENDENCY_OK'
assert '/skills/native-python-fixture/.venv/' in sys.executable
assert not any(k in os.environ for k in ['RUA_PHONE_TOKEN','RUA_PHONE_SOCKET','RUA_PHONE_HOST_PID','RUA_ANDROID_NATIVE_DIR','LD_LIBRARY_PATH'])
nested = subprocess.check_output([sys.executable, 'scripts/nested.py'], text=True).strip()
assert nested == VALUE
pathlib.Path(os.environ['RUA_FIXTURE_WORKSPACE'], 'python-skill-output.json').write_text(json.dumps({'dependency': VALUE, 'nested': nested, 'platform': sys.platform}))
print('PYTHON_SKILL_OK')
''',
    'native-python-fixture/scripts/nested.py': 'from rua_fixture_dependency import VALUE\nprint(VALUE)\n',
    'native-node-fixture/package.json': json.dumps({'name':'rua-node-skill','version':'1.0.0','private':True,'dependencies':{'rua-fixture-package':'file:deps/package'}}),
    'native-node-fixture/deps/package/package.json': json.dumps({'name':'rua-fixture-package','version':'1.0.0','main':'index.js'}),
    'native-node-fixture/deps/package/index.js': "module.exports = 'NODE_DEPENDENCY_OK';\n",
    'native-node-fixture/scripts/run.sh': '''#!/bin/bash
set -eu
node --version
npm --version
npx --version
npm install --ignore-scripts --no-audit --no-fund --offline
chmod +x scripts/main.js
scripts/main.js
''',
    'native-node-fixture/scripts/main.js': '''#!/usr/bin/env node
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {execFileSync} = require('node:child_process');
const dependency = require('rua-fixture-package');
assert.equal(process.platform, 'linux');
assert.equal(process.arch, 'arm64');
assert.equal(dependency, 'NODE_DEPENDENCY_OK');
for (const key of ['RUA_PHONE_TOKEN','RUA_PHONE_SOCKET','RUA_PHONE_HOST_PID','RUA_ANDROID_NATIVE_DIR','LD_LIBRARY_PATH']) assert.equal(process.env[key], undefined);
const nested = execFileSync(process.execPath, ['scripts/nested.js'], {encoding:'utf8'}).trim();
assert.equal(nested, dependency);
fs.writeFileSync(path.join(process.env.RUA_FIXTURE_WORKSPACE, 'node-skill-output.json'), JSON.stringify({dependency,nested,platform:process.platform,arch:process.arch}));
console.log('NODE_SKILL_OK');
''',
    'native-node-fixture/scripts/nested.js': "console.log(require('rua-fixture-package'));\n",
})
if __name__ == '__main__':
    stage_path = Path(__file__).resolve().parents[1] / 'android/stage-native-payload.py'
    spec = importlib.util.spec_from_file_location('stage', stage_path)
    stage = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(stage)
    output = Path('target/android-runtime-skills.zip')
    stage.atomic_bytes(output, archive(files))
    print(output)
