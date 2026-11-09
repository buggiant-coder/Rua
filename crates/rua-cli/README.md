# rua-cli

Rua unified CLI.

## ACP server

The `rua acp` subcommand runs Rua as an ACP server on stdio. Editors like
Zed, or another agent acting as an ACP client, can spawn it as a subprocess and
communicate with the selected Rua agent through the stable ACP v1 protocol.

If a Rua server is reachable through `--base-url`, `RUA_HOST` /
`RUA_PORT`, or the installed endpoint configuration, the ACP process reuses
that server's resident Agent Runtime. It does not open the same working
directory a second time. When no server is listening, it falls back to a
standalone embedded runtime.

### Zed configuration

Add the following to `~/.config/zed/settings.json`:

```json
{
  "agent_servers": {
    "rua": {
      "command": "rua",
      "args": ["acp"],
      "env": {}
    }
  }
}
```

To use a specific agent profile and override its model for this ACP server
process:

```json
{
  "agent_servers": {
    "rua": {
      "command": "rua",
      "args": [
        "acp",
        "--agent",
        "my-agent",
        "--provider-id",
        "provider-id",
        "--model-id",
        "model-id"
      ],
      "env": {}
    }
  }
}
```

`--provider-id` and `--model-id` must be passed together. They override the
selected agent's model for this ACP server process.

The `--workspace` global flag can also be passed to override the default data
directory:

```json
{
  "agent_servers": {
    "rua": {
      "command": "rua",
      "args": ["--workspace", "/path/to/workspace", "acp"],
      "env": {}
    }
  }
}
```
