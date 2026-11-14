#!/usr/bin/env python3
"""Gated Phone Use session fixture for debug APKs in disposable emulators only.

Never target a physical/user phone or pair the emulator with production Sync.
The test uses only a harmless fixture Activity, and never contacts a real model.

Build and install both debug APKs in the disposable emulator:
  scripts/package-mobile-apk.sh --debug --out-dir target/phone-use-session-e2e/debug
  mobile/mobile-shell/gradlew -p mobile/mobile-shell :app:assembleDebugAndroidTest
  adb -s emulator-5560 install -r target/phone-use-session-e2e/debug/rua-debug-arm64-v8a.apk
  adb -s emulator-5560 install -r mobile/mobile-shell/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
Then run the focused selector:
  python3 scripts/tests/android-phone-session-model.py --port 18099
  adb -s emulator-5560 reverse tcp:18099 tcp:18099
  adb -s emulator-5560 shell am instrument -w -r -e class \
    com.rua.mobile.PhoneUseSessionDeviceTest#realAgentSessionPersistsAcrossThinkingCompletionAndStops \
    com.rua.mobile.test/androidx.test.runner.AndroidJUnitRunner
Finally remove that reverse mapping and stop this fixture.
"""
import argparse
import json
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CASES = {}
LOCK = threading.Lock()
MARKER = re.compile(r'native-phone-session:([a-zA-Z0-9_-]+):(neutral|release-only|release|retain|stop-inherited|stop|contender)(?![a-z-])')


def decoded(value):
    try:
        return json.loads(value) if isinstance(value, str) else value
    except (ValueError, TypeError):
        return None


def observed(value):
    value = decoded(value)
    return isinstance(value, dict) and bool(value.get('observation_id')) and isinstance(value.get('nodes'), list)


def case_for(key, mode):
    with LOCK:
        if key not in CASES:
            CASES[key] = {'mode': mode, 'gate': threading.Event(), 'held': False, 'events': [], 'late_response_cancelled': False}
        assert CASES[key]['mode'] == mode
        return CASES[key]


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *_):
        pass

    def reply(self, value, code=200):
        payload = json.dumps(value).encode()
        self.send_response(code)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        if not self.path.startswith('/control/state'):
            return self.reply({'error': 'unknown fixture route'}, 404)
        with LOCK:
            result = {key: {name: case[name] for name in ['mode', 'held', 'events', 'late_response_cancelled']} for key, case in CASES.items()}
        self.reply(result)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))) or b'{}')
        if self.path == '/control/continue':
            with LOCK:
                case = CASES.get(body.get('case'))
            if case is None:
                return self.reply({'error': 'unknown fixture case'}, 404)
            case['gate'].set()
            return self.reply({'released': True})
        if self.path != '/v1/chat/completions':
            return self.reply({'error': 'unknown fixture route'}, 404)
        messages = body.get('messages', [])
        last_user = next((index for index in range(len(messages) - 1, -1, -1) if messages[index].get('role') == 'user'), None)
        match = MARKER.search(json.dumps(messages[last_user].get('content'))) if last_user is not None else None
        if not match:
            return self.reply({'error': 'explicit native-phone-session case marker required'}, 400)
        key, mode = match.groups()
        case = case_for(key, mode)
        results = [message.get('content') for message in messages[last_user + 1:] if message.get('role') == 'tool']
        exposed = sorted(tool.get('function', {}).get('name') for tool in body.get('tools', []) if str(tool.get('function', {}).get('name', '')).startswith('phone_'))
        event = {'tool_results': len(results), 'exposed_phone_tools': exposed, 'at_monotonic': time.monotonic()}
        if len(results) >= 2:
            event['observation_valid'] = observed(results[1])
            observation = decoded(results[1])
            if observed(observation):
                event['node_count'] = len(observation['nodes'])
                event['screenshot_absent'] = not observation.get('screenshot')
        with LOCK:
            case['events'].append(event)
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Connection', 'close')
        self.end_headers()

        def send(delta, finish=None):
            chunk = {'id': 'phone-session-fixture', 'object': 'chat.completion.chunk', 'model': body.get('model'), 'choices': [{'index': 0, 'delta': delta, 'finish_reason': finish}]}
            self.wfile.write(('data: ' + json.dumps(chunk) + '\n\n').encode())
            self.wfile.flush()

        tool, arguments, passed = None, None, True
        if mode == 'stop-inherited':
            if not results:
                with LOCK:
                    case['held'] = True
                if case['gate'].wait(45):
                    tool, arguments = 'phone_observe', {'include_screenshot': False}
                else:
                    passed = False
                    event['gate_timed_out'] = True
                with LOCK:
                    case['held'] = False
            else:
                passed = False
                event['unexpected_post_stop_tool_result'] = True
        elif mode == 'release-only':
            if not results:
                tool, arguments = 'phone_release', {}
        elif not results:
            tool, arguments = 'phone_status', {}
        elif len(results) == 1:
            tool, arguments = ('phone_list_apps', {}) if mode == 'neutral' else ('phone_observe', {'include_screenshot': False})
        elif mode in ['release', 'retain', 'stop'] and len(results) == 2:
            with LOCK:
                case['held'] = True
            if not case['gate'].wait(45):
                passed = False
                event['gate_timed_out'] = True
            elif mode == 'release':
                tool, arguments = 'phone_release', {}
            elif mode == 'stop':
                observation = decoded(results[1])
                target = next((node for node in observation.get('nodes', []) if node.get('text') == 'Increment'), None) if isinstance(observation, dict) else None
                if target:
                    tool, arguments = 'phone_click', {'observation_id': observation['observation_id'], 'element_token': target['token']}
                else:
                    passed = False
                    event['missing_fixture_increment_node'] = True
            with LOCK:
                case['held'] = False
        elif mode == 'contender':
            passed = not observed(results[1]) and any(word in str(results[1]).lower() for word in ['busy', 'another', 'owned', 'in use'])
        elif mode == 'stop':
            passed = False
            event['unexpected_post_stop_tool_result'] = True
        if mode not in ['neutral', 'contender', 'release-only'] and len(results) >= 2:
            passed = passed and observed(results[1])
        try:
            if tool:
                event['requested_tool'] = tool
                send({'role': 'assistant', 'tool_calls': [{'index': 0, 'id': f'{key}-tool-{len(results)}', 'type': 'function', 'function': {'name': tool, 'arguments': json.dumps(arguments)}}]})
                send({}, 'tool_calls')
            else:
                event['final_pass'] = passed
                send({'role': 'assistant', 'content': 'PHONE_SESSION_PASS' if passed else 'PHONE_SESSION_FAILED'})
                send({}, 'stop')
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            with LOCK:
                case['late_response_cancelled'] = True
        finally:
            self.close_connection = True


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=18099)
    args = parser.parse_args()
    print(f'Phone session fixture on localhost:{args.port}', flush=True)
    ThreadingHTTPServer(('127.0.0.1', args.port), Handler).serve_forever()
