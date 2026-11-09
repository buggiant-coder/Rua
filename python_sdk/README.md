# Python plugin SDK and demo

`rua_plugin_sdk` is a minimal helper library for writing Python plugins that
communicate with Rua's Rust plugin runtime over JSON-RPC on stdin/stdout.
It provides `PluginApp` and registration methods for tools, hooks, skills, and
CLI commands. It is plugin-side support code, not Rua's core runtime.

The runnable **Python plugin demo** lives in
[`crates/rua-plugin-runtime/tests/fixtures/hello_world_plugin/`](../crates/rua-plugin-runtime/tests/fixtures/hello_world_plugin/).
Its [`hello_world/__main__.py`](../crates/rua-plugin-runtime/tests/fixtures/hello_world_plugin/hello_world/__main__.py)
demonstrates all four capabilities. The Rust
[`plugin_e2e.rs`](../crates/rua-server/tests/plugin/plugin_e2e.rs) tests use this
demo to verify plugin startup, calls, and lifecycle behavior.

The SDK and demo are separate: import `PluginApp` from `rua_plugin_sdk` when
writing a plugin; use the demo as an example and test fixture. The demo's
`hello_world` identity is retained because the end-to-end tests reference it.

To launch the demo from the repository root:

```sh
PYTHONPATH="$PWD/python_sdk:$PWD/crates/rua-plugin-runtime/tests/fixtures/hello_world_plugin" \
  python3 -m hello_world
```

The process reads one JSON-RPC request per line from stdin and writes responses
to stdout. Write diagnostic logs to stderr so they do not corrupt the protocol.
