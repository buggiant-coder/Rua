# Plugin Protocol Specification

## Overview

Plugins communicate with the Rua host via JSON-RPC 2.0 over stdin/stdout.
All plugin-specific methods live under the `plugin/` namespace.

## Transport

Uses the same JSON-RPC 2.0 envelope shape as MCP:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "plugin/<method>",
  "params": { ... }
}
```

Responses follow standard JSON-RPC 2.0:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": { ... }
}
```

Errors:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32601,
    "message": "Method not found"
  }
}
```

## Heartbeat

Plugin liveness is checked by reusing the MCP `tools/list` request as a ping.
If the plugin responds within the configured timeout, it is considered alive.
No dedicated heartbeat method is needed.

## Capability Manifest

On registration, a plugin declares its capabilities as a subset of:

```json
{
  "capabilities": ["tools", "hooks", "skills", "cli"]
}
```

The host only dispatches to a plugin for capability categories it declared.

## Methods

### plugin/register_hook

Register a lifecycle hook.

**Params:**

| Field       | Type   | Required | Description                       |
|-------------|--------|----------|-----------------------------------|
| plugin_id   | string | yes      | Unique plugin identifier          |
| hook_point  | string | yes      | One of the `HookPoint` variants   |
| hook_name   | string | yes      | Human-readable hook name          |
| callback    | string | yes      | Plugin callback name         |
| priority    | i32    | no       | Execution order (default 100)     |

**Result:**

```json
{ "registered": true }
```

### plugin/register_skill

Register a skill into the skill pool.

**Params:**

| Field       | Type   | Required | Description                  |
|-------------|--------|----------|------------------------------|
| plugin_id   | string | yes      | Unique plugin identifier     |
| name        | string | yes      | Skill name                   |
| description | string | yes      | Skill description            |
| content     | string | no       | Skill markdown content       |

**Result:**

```json
{ "registered": true }
```

### plugin/register_cli

Register a CLI subcommand.

**Params:**

| Field       | Type          | Required | Description              |
|-------------|---------------|----------|--------------------------|
| plugin_id   | string        | yes      | Unique plugin identifier |
| name        | string        | yes      | Command name             |
| description | string        | yes      | Command description      |
| callback    | string        | yes      | Plugin callback name     |
| args        | CliArgDef[]  | no       | Argument definitions     |

**Result:**

```json
{ "registered": true }
```

### plugin/invoke_hook

Invoke a registered hook at a given hook point.

**Params:**

| Field      | Type   | Required | Description                     |
|------------|--------|----------|---------------------------------|
| hook_point | string | yes      | The hook point to dispatch      |
| payload    | string | yes      | JSON payload passed to the hook |

**Result:**

```json
{
  "results": [
    {
      "plugin_id": "my-plugin",
      "hook_name": "log-prompt",
      "output": "..."
    }
  ]
}
```

### plugin/invoke_skill

Invoke a plugin-provided skill by name.

**Params:**

| Field      | Type   | Required | Description           |
|------------|--------|----------|-----------------------|
| plugin_id  | string | yes      | Plugin that owns it   |
| skill_name | string | yes      | Skill to invoke       |
| input      | string | yes      | Input payload         |

**Result:**

```json
{
  "output": "..."
}
```

### plugin/invoke_cli

Invoke a plugin-provided CLI command.

**Params:**

| Field      | Type   | Required | Description               |
|------------|--------|----------|---------------------------|
| plugin_id  | string | yes      | Plugin that owns it       |
| cmd_name   | string | yes      | Command name              |
| args       | object | yes      | Key-value argument map    |

**Result:**

```json
{
  "output": "..."
}
```

## Error Codes

Standard JSON-RPC 2.0 error codes apply:

| Code   | Meaning              |
|--------|----------------------|
| -32700 | Parse error          |
| -32600 | Invalid request      |
| -32601 | Method not found     |
| -32602 | Invalid params       |
| -32603 | Internal error       |

Plugin-specific errors use codes in the range -32000 to -32099:

| Code   | Meaning              |
|--------|----------------------|
| -32000 | Plugin not found     |
| -32001 | Hook dispatch failed |
| -32002 | Skill not found      |
| -32003 | CLI command not found|
| -32004 | Invocation failed    |
