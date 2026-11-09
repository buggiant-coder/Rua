"""Plugin-side JSON-RPC helpers used by the Python plugin demo and Rust E2E tests."""

from __future__ import annotations

import json
import sys


class PluginApp:
    """Register Python plugin handlers and serve the Rust plugin runtime's requests."""

    def __init__(self, name: str, version: str):
        self.name = name
        self.version = version
        self._tools: dict[str, dict] = {}
        self._hooks: dict[tuple[str, str], object] = {}
        self._skills: dict[str, dict] = {}
        self._cli: dict[str, dict] = {}

    def register_tool(self, name, description, parameters, handler):
        self._tools[name] = {
            "description": description,
            "parameters": parameters,
            "handler": handler,
        }

    def register_hook(self, hook_point, hook_name, handler):
        self._hooks[(hook_point, hook_name)] = handler

    def register_skill(self, name, description, content, handler):
        self._skills[name] = {
            "description": description,
            "content": content,
            "handler": handler,
        }

    def register_cli(self, name, description, handler):
        self._cli[name] = {"description": description, "handler": handler}

    def run(self):
        """Serve newline-delimited JSON-RPC; reserve stdout for protocol responses."""
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
            resp = self._dispatch(req)
            sys.stdout.write(json.dumps(resp) + "\n")
            sys.stdout.flush()

    def _dispatch(self, req):
        method = req.get("method", "")
        rid = req.get("id")
        params = req.get("params") or {}

        if method == "initialize":
            return _ok(rid, {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": self.name, "version": self.version},
                "capabilities": {"tools": {}},
            })

        if method == "tools/list":
            tools = [
                {
                    "name": n,
                    "description": t["description"],
                    "inputSchema": t["parameters"],
                }
                for n, t in self._tools.items()
            ]
            return _ok(rid, {"tools": tools})

        if method == "tools/call":
            name = params.get("name", "")
            args = params.get("arguments") or {}
            if name in self._tools:
                text = self._tools[name]["handler"](args)
                return _ok(rid, {
                    "content": [{"type": "text", "text": text}],
                    "isError": False,
                })
            return _err(rid, -32601, f"tool not found: {name}")

        if method == "ping":
            return _ok(rid, {})

        if method == "plugin/invoke_hook":
            hook_point = params.get("hook_point", "")
            results = []
            for (hp, hn), handler in self._hooks.items():
                if hp == hook_point:
                    output = handler(params.get("payload", "{}"))
                    results.append({
                        "plugin_id": self.name,
                        "hook_name": hn,
                        "output": output,
                    })
            return _ok(rid, {"results": results})

        if method == "plugin/invoke_skill":
            skill_name = params.get("skill_name", "")
            input_val = params.get("input", "")
            if skill_name in self._skills:
                output = self._skills[skill_name]["handler"](input_val)
                return _ok(rid, {"output": output})
            return _err(rid, -32002, f"skill not found: {skill_name}")

        if method == "plugin/invoke_cli":
            cmd_name = params.get("cmd_name", "")
            args = params.get("args") or {}
            if cmd_name in self._cli:
                output = self._cli[cmd_name]["handler"](args)
                return _ok(rid, {"output": output})
            return _err(rid, -32003, f"cli command not found: {cmd_name}")

        return _err(rid, -32601, f"unknown method: {method}")


def _ok(rid, result):
    return {"jsonrpc": "2.0", "id": rid, "result": result}


def _err(rid, code, message):
    return {"jsonrpc": "2.0", "id": rid, "error": {"code": code, "message": message}}
