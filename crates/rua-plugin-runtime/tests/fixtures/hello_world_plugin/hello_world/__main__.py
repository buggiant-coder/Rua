"""Python plugin demo and Rust E2E fixture for tools, hooks, skills, and CLI calls.

Uses the helper SDK in python_sdk/; this module is an example plugin, not core
Rua runtime code. Keep stdout for JSON-RPC responses and log to stderr.
"""

from __future__ import annotations

import sys

from rua_plugin_sdk import PluginApp

app = PluginApp("hello_world", "1.0.0")


def say_hi(args):
    name = args.get("name", "world")
    return f"hi {name}"


def on_post_tool_call(payload):
    sys.stderr.write(f"[hello_world] post_tool_call: {payload}\n")
    sys.stderr.flush()
    return '{"logged": true}'


def greet(input_val):
    return f"greetings, {input_val}!"


def cli_hi(args):
    name = args.get("name", "world")
    return f"hi {name}"


app.register_tool(
    "say_hi",
    "Say hi to someone",
    {"type": "object", "properties": {"name": {"type": "string"}}},
    say_hi,
)
app.register_hook("after_tool_call", "post_tool_call_log", on_post_tool_call)
app.register_skill(
    "greeting",
    "A greeting skill",
    "---\nname: greeting\ndescription: A greeting skill\n---\nGreet someone!",
    greet,
)
app.register_cli("hi", "Say hi from CLI", cli_hi)

if __name__ == "__main__":
    try:
        app.run()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
