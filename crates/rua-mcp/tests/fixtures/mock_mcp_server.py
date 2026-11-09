#!/usr/bin/env python3


from __future__ import annotations

import json
import os
import sys
import threading
import time

SCENARIO = os.environ.get("MOCK_SCENARIO", "fifo")
DELAY_MS = int(os.environ.get("MOCK_DELAY_MS", "0"))
INITIALIZE_DELAY_MS = int(os.environ.get("MOCK_INITIALIZE_DELAY_MS", "0"))
CRASH_AFTER = int(os.environ.get("MOCK_CRASH_AFTER", "1"))

_stdout_lock = threading.Lock()


def write_line(obj):

    payload = json.dumps(obj) + "\n"
    with _stdout_lock:
        sys.stdout.write(payload)
        sys.stdout.flush()


def write_notification(method, params=None):
    write_line({"jsonrpc": "2.0", "method": method, "params": params or {}})


def make_response(req_id, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": req_id}
    if error is not None:
        msg["error"] = error
    else:
        msg["result"] = result if result is not None else {}
    return msg


def respond(req):
    method = req.get("method", "")
    req_id = req.get("id")
    if method == "initialize":
        return make_response(req_id, {
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "mock-mcp", "version": "0.0.1"},
            "capabilities": {},
        })
    if method == "tools/list":
        return make_response(req_id, {
            "tools": [
                {
                    "name": "echo",
                    "description": "echo input as text",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"msg": {"type": "string"}},
                    },
                }
            ]
        })
    if method == "tools/call":
        args = (req.get("params") or {}).get("arguments") or {}
        return make_response(req_id, {
            "content": [{"type": "text", "text": str(args.get("msg", ""))}],
            "isError": False,
        })
    if method == "ping":
        return make_response(req_id, {})
    return make_response(
        req_id,
        error={"code": -32601, "message": f"unknown method: {method}"},
    )


def delayed_respond(req, delay_ms):
    time.sleep(delay_ms / 1000.0)
    write_line(respond(req))


def main():
    count_file = os.environ.get("MOCK_START_LOG")
    if count_file:
        with open(count_file, "a") as output:
            output.write(str(os.getpid()) + "\n")
    request_count = 0
    queued = []

    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            sys.stderr.write(f"mock: invalid JSON (dropped): {line[:80]}\n")
            sys.stderr.flush()
            continue

        request_log = os.environ.get("MOCK_REQUEST_LOG")
        if request_log:
            with open(request_log, "a") as output:
                output.write(req.get("method", "") + "\n")

        if "id" not in req:
            continue


        if req.get("method") == "initialize":
            if INITIALIZE_DELAY_MS:
                time.sleep(INITIALIZE_DELAY_MS / 1000.0)
            write_line(respond(req))
            if SCENARIO == "blocked_stdin":
                time.sleep(60)
            continue

        request_count += 1

        if SCENARIO == "crash":
            if request_count > CRASH_AFTER:

                sys.exit(1)
            write_line(respond(req))
            continue

        if SCENARIO == "delay":
            threading.Thread(
                target=delayed_respond,
                args=(req, DELAY_MS),
                daemon=True,
            ).start()
            continue

        if SCENARIO == "interleave":
            write_notification("notifications/progress", {"progress": 0.5})
            write_line(respond(req))
            write_notification("notifications/cancelled", {"reason": "ignored"})
            continue

        if SCENARIO == "reverse":
            queued.append(req)
            if len(queued) == 1:

                continue
            second = queued.pop()
            first = queued.pop(0)
            write_line(respond(second))
            write_line(respond(first))
            continue


        write_line(respond(req))


if __name__ == "__main__":
    try:
        main()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
