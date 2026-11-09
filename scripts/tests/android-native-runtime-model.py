#!/usr/bin/env python3
"""Deterministic model fixture for the real Android Agent -> tools -> Debian acceptance."""
import argparse
import json
import shlex
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASH = """set -eu
printf 'from-shell' > native-input.txt
printf '#!/bin/bash\nprintf NESTED_SCRIPT_OK\\n' > native-nested.sh
bash native-nested.sh
python3 -c 'import sys; assert sys.platform == "linux"; print("SHELL_DEBIAN_OK")'
"""
PYTHON = """import os,sys,pathlib,json
assert sys.platform == 'linux'
assert sys.executable == '/usr/bin/python3'
base = pathlib.Path.cwd().parent
assert (base / 'native-input.txt').read_text() == 'from-shell'
assert not any(k in os.environ for k in ['RUA_PHONE_TOKEN','RUA_PHONE_SOCKET','RUA_PHONE_HOST_PID','RUA_ANDROID_NATIVE_DIR','LD_LIBRARY_PATH'])
(base / 'native-output.json').write_text(json.dumps({'python':sys.version,'cwd':str(pathlib.Path.cwd()),'uid':os.getuid()}))
print('PYTHON_DEBIAN_OK')
"""

def observations(value):
    if isinstance(value, str):
        try:
            yield from observations(json.loads(value))
        except (ValueError, TypeError):
            pass
    elif isinstance(value, list):
        for item in value:
            yield from observations(item)
    elif isinstance(value, dict):
        if 'observation_id' in value and 'nodes' in value:
            yield value
        else:
            for item in value.values():
                yield from observations(item)

class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if not body.get('stream'):
            payload = json.dumps({'id':'fixture-probe','object':'chat.completion','model':body.get('model'),'choices':[{'index':0,'message':{'role':'assistant','content':'fixture ready'},'finish_reason':'stop'}]}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        results = [json.dumps(m.get('content'), ensure_ascii=False) for m in body.get('messages', []) if m.get('role') == 'tool']
        combined = '\n'.join(results)
        encoded = json.dumps(body.get('messages', []))
        mode = next((mode for mode in ['upgrade-history', 'upgrade-check', 'phone-denied', 'phone-terminate', 'phone-stop', 'phone', 'python-stop', 'python-timeout', 'bash-stop', 'skills', 'node-stop', 'mcp-stop'] if 'native-runtime-' + mode in encoded), None)
        if mode == 'upgrade-history':
            tool, arguments = None, None
        elif mode == 'upgrade-check':
            tool, arguments = ('bash', {'command': 'bash upgrade-skill.sh', 'yield_seconds': 10}) if not results else (None, None)
        elif mode and mode.startswith('phone'):
            observed = list(observations([m.get('content') for m in body.get('messages', []) if m.get('role') == 'tool']))
            if mode == 'phone-denied':
                tool, arguments = ('phone_observe', {'include_screenshot': False}) if not results else (None, None)
            elif not results:
                tool, arguments = 'phone_status', {}
            elif len(results) == 1:
                tool, arguments = 'phone_observe', {'include_screenshot': False}
            elif observed and len(results) == 2:
                observation = observed[0]
                if mode in ('phone-terminate', 'phone-stop'):
                    tool, arguments = 'phone_swipe', {'observation_id': observation['observation_id'], 'x': 100, 'y': 700, 'end_x': 100, 'end_y': 400, 'duration_ms': 1000}
                else:
                    node = next((n for n in observation['nodes'] if n.get('text') == 'Increment'), None)
                    tool, arguments = ('phone_click', {'observation_id': observation['observation_id'], 'element_token': node['token']}) if node else (None, None)
            elif observed and len(results) == 3 and mode == 'phone':
                node = next(n for n in observed[0]['nodes'] if n.get('text') == 'Increment')
                tool, arguments = 'phone_click', {'observation_id': observed[0]['observation_id'], 'element_token': node['token']}
            elif len(results) == 4 and mode == 'phone':
                time.sleep(0.3)
                tool, arguments = 'phone_observe', {'include_screenshot': False}
            else:
                tool, arguments = None, None
        elif mode == 'mcp-stop':
            tool = next((t['function']['name'] for t in body.get('tools', []) if 'wait' in t['function']['name'] and 'native' in t['function']['name']), None) if not results else None
            arguments = {} if tool else None
        elif mode == 'skills':
            if not results:
                tool, arguments = 'runtime_info', {}
            elif len(results) == 1:
                tool, arguments = 'bash', {'command': 'set -eu; export RUA_FIXTURE_WORKSPACE="$PWD"; for skill in native-python-fixture native-node-fixture; do (cd "skills/$skill"; cat SKILL.md; bash scripts/run.sh); done', 'yield_seconds': 10}
            elif 'PYTHON_SKILL_OK' in combined and 'NODE_SKILL_OK' in combined and 'SKILL_WORKSPACE_OK' not in combined:
                tool, arguments = 'python', {'code': "import json,pathlib; p=pathlib.Path.cwd().parent; a=json.loads((p/'python-skill-output.json').read_text()); b=json.loads((p/'node-skill-output.json').read_text()); assert a['dependency']=='PYTHON_DEPENDENCY_OK'; assert b['dependency']=='NODE_DEPENDENCY_OK'; print('SKILL_WORKSPACE_OK')", 'timeout_secs':30}
            else:
                tool, arguments = None, None
        elif mode == 'node-stop' and not results:
            code = "const fs=require('node:fs');const {spawn}=require('node:child_process');const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)']);fs.writeFileSync('/root/native-node-stop-pids',process.pid+' '+child.pid);setInterval(()=>{},1000)"
            tool, arguments = 'bash', {'command': 'node -e ' + shlex.quote(code), 'yield_seconds':10}
        elif mode and not results:
            code = "import os,subprocess,time; p=subprocess.Popen(['/usr/bin/python3','-c','import time; time.sleep(120)']); open('/root/native-" + mode + "-pids','w').write(str(os.getpid())+' '+str(p.pid)); time.sleep(120)"
            if mode == 'bash-stop':
                tool, arguments = 'bash', {'command': '/usr/bin/python3 -c ' + shlex.quote(code), 'yield_seconds': 10}
            else:
                tool, arguments = 'python', {'code': code, 'timeout_secs': 3 if mode.endswith('timeout') else 120}
        elif mode:
            tool, arguments = None, None
        elif not results:
            tool, arguments = 'bash', {'command': BASH, 'yield_seconds': 10}
        elif 'SHELL_DEBIAN_OK' in combined and 'PYTHON_DEBIAN_OK' not in combined:
            tool, arguments = 'python', {'code': PYTHON, 'timeout_secs': 30}
        elif 'PYTHON_DEBIAN_OK' in combined and 'WORKSPACE_ROUNDTRIP_OK' not in combined:
            tool, arguments = 'bash', {'command': 'test -s native-output.json && cat native-output.json && printf WORKSPACE_ROUNDTRIP_OK', 'yield_seconds': 10}
        else:
            tool, arguments = None, None
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Connection', 'close')
        self.end_headers()
        model = body.get('model', 'native-runtime-model')
        def send(delta, finish=None):
            message = {'id':'native-runtime-fixture','object':'chat.completion.chunk','model':model,'choices':[{'index':0,'delta':delta,'finish_reason':finish}]}
            self.wfile.write(('data: ' + json.dumps(message) + '\n\n').encode())
        if tool:
            print('real Agent requested fixture step:', tool, 'after', len(results), 'results', flush=True)
            send({'role':'assistant','tool_calls':[{'index':0,'id':f'native-tool-{len(results)}','type':'function','function':{'name':tool,'arguments':json.dumps(arguments)}}]})
            send({}, 'tool_calls')
        else:
            passed = ('timed out' in combined) if mode == 'python-timeout' else all(marker in combined for marker in ['NESTED_SCRIPT_OK', 'SHELL_DEBIAN_OK', 'PYTHON_DEBIAN_OK', 'WORKSPACE_ROUNDTRIP_OK'])
            if mode == 'upgrade-history':
                passed = True
            elif mode == 'upgrade-check':
                passed = 'MIGRATED_SKILL_OK' in combined
            elif mode == 'phone':
                passed = 'clicks=1' in combined and ('refused' in combined or 'stale' in combined.lower() or 'expired' in combined.lower())
            elif mode == 'phone-denied':
                passed = 'accessibility' in combined.lower() and 'permission' in combined.lower()
            elif mode in ('phone-terminate', 'phone-stop'):
                passed = 'cancel' in combined.lower() or 'outcome_unknown' in combined
            if mode == 'skills':
                passed = all(marker in combined for marker in ['PYTHON_SKILL_OK','NODE_SKILL_OK','SKILL_WORKSPACE_OK'])
            print('Agent result:', 'PASS' if passed else combined, flush=True)
            send({'role':'assistant','content':'NATIVE_AGENT_ALL_PASS' if passed else 'NATIVE_AGENT_FAILED: ' + combined})
            send({}, 'stop')
        self.wfile.write(b'data: [DONE]\n\n')
        self.wfile.flush()
        self.close_connection = True

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port',type=int,default=18099)
    args = parser.parse_args()
    print('Native Agent model fixture on localhost', args.port, flush=True)
    ThreadingHTTPServer(('127.0.0.1', args.port), Handler).serve_forever()
