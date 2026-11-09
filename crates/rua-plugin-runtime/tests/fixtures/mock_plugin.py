#!/usr/bin/env python3


from __future__ import annotations

import json
import os
import sys
import time

CRASH_AFTER = int(os.environ.get("MOCK_PLUGIN_CRASH_AFTER", "0"))
HANG = os.environ.get("MOCK_PLUGIN_HANG", "") == "1"


def write_line(obj):
    payload = json.dumps(obj) + "\n"
    sys.stdout.write(payload)
    sys.stdout.flush()


def make_response(req_id, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": req_id}
    if error is not None:
        msg["error"] = error
    else:
        msg["result"] = result if result is not None else {}
    return msg


def handle(req):
    method = req.get("method", "")
    req_id = req.get("id")
    params = req.get("params") or {}

    if method == "initialize":
        return make_response(req_id, {
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "mock-plugin", "version": "0.1.0"},
            "capabilities": {"tools": {}},
        })

    if method == "tools/list":
        return make_response(req_id, {
            "tools": [{
                "name": "greet",
                "description": "greet someone",
                "inputSchema": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                },
            }]
        })

    if method == "tools/call":
        args = params.get("arguments") or {}
        name = args.get("name", "world")
        return make_response(req_id, {
            "content": [{"type": "text", "text": f"hello {name}"}],
            "isError": False,
        })

    if method == "ping":
        return make_response(req_id, {})

    if method == "plugin/invoke_hook":
        hook_point = params.get("hook_point", "")
        return make_response(req_id, {
            "results": [{
                "plugin_id": "mock",
                "hook_name": f"hook-{hook_point}",
                "output": f"hooked:{hook_point}",
            }]
        })

    if method == "plugin/invoke_skill":
        skill_name = params.get("skill_name", "")
        input_val = params.get("input", "")
        return make_response(req_id, {
            "output": f"skill:{skill_name}:{input_val}",
        })

    if method == "plugin/invoke_cli":
        cmd_name = params.get("cmd_name", "")
        return make_response(req_id, {
            "output": f"cli:{cmd_name}",
        })

    return make_response(
        req_id,
        error={"code": -32601, "message": f"unknown method: {method}"},
    )


def main():
    absent = os.environ.get("MOCK_PLUGIN_REQUIRE_ABSENT")
    assert not absent or absent not in os.environ
    sys.stderr.write("mock-plugin: started\n")
    sys.stderr.flush()

    request_count = 0

    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue

        if "id" not in req:
            continue

        resp = handle(req)
        write_line(resp)

        if req.get("method") == "initialize":
            if HANG:
                time.sleep(3600)
                return
            continue

        request_count += 1
        if CRASH_AFTER > 0 and request_count >= CRASH_AFTER:
            sys.stderr.write("mock-plugin: crashing as configured\n")
            sys.stderr.flush()
            sys.exit(1)


if __name__ == "__main__":
    try:
        main()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
