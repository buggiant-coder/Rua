# agent-types

AI Agent shared types and compatibility semantics for the rua-rust workspace.

## Purpose

`agent-types` is the lowest-level type library in rua-rust. It defines the
canonical data model shared across all rua crates: provider adapters, ReAct
agent, tool system, MCP, memory, server, and WebUI.

This crate is designed for **compatibility first** -- it faithfully carries
Python Rua / AgentScope semantics rather than pursuing an ideal new design.

## Compatibility

### Python Rua / AgentScope

| Python / AgentScope block | Rust canonical type                        | WebUI MessageType     |
|---------------------------|--------------------------------------------|-----------------------|
| text                      | `ContentPart::Text` / `TextDelta`          | MESSAGE               |
| thinking                  | `ContentPart::Thinking` / `ThinkingDelta`  | REASONING             |
| tool_use                  | `ToolCall` / `ContentPart::ToolUse`        | PLUGIN_CALL           |
| tool_result               | `ToolResult` / `ContentPart::ToolResult`   | PLUGIN_CALL_OUTPUT    |
| error                     | `AgentEvent::Error`                        | ERROR                 |
| usage                     | `TokenUsage` / `AgentEvent::Usage`         | metadata              |
| start/end                 | `AgentEvent::Start/End`                    | metadata              |

### Provider Support

Provider adapters map provider-specific events to `AgentEvent` through
intermediate types defined in the `provider_mapping` module:

- **OpenAI-compatible** (including Azure, Groq, Together, etc.)
- **DashScope/Qwen** (`reasoning_content` for chain-of-thought)
- **Anthropic** (content blocks, thinking with signature, tool_use)
- **Gemini** (parts, functionCall, functionResponse)
- **Local models** (plain text stream)

### WebUI

The `webui_mapping` module provides constants and helpers for mapping events
to WebUI `MessageType` values. The WebUI renders messages based on these types
without modification -- the Rust backend must produce compatible output.

## Design Principles

- **Forward-compatible**: enums are `#[non_exhaustive]`, structs carry
  `metadata` maps for unknown fields.
- **Provider-neutral**: no provider SDK is hardcoded.
- **Low dependency**: only `serde`, `serde_json`, `chrono`, `uuid`.
  No async, no network, no runtime.
- **No unsafe**: `#![forbid(unsafe_code)]`.

## Module Overview

| Module             | Key Types                                        |
|--------------------|--------------------------------------------------|
| `role`             | `Role` (system/user/assistant/tool/developer)    |
| `content`          | `ContentPart` (text, thinking, image, tool, ...) |
| `model_dialogue_item` | `ModelDialogueItem`                                 |
| `tool`             | `ToolDefinition`, `ToolCall`, `ToolResult`, ...  |
| `usage`            | `TokenUsage`                                     |
| `finish`           | `FinishReason`                                   |
| `request`          | `ChatRequest`                                    |
| `response`         | `ChatResponse`                                   |
| `event`            | `AgentEvent` / `ChatStreamEvent`                 |
| `provider_mapping` | OpenAI/Anthropic/Gemini intermediate types        |
| `webui_mapping`    | WebUI MessageType constants and helpers           |

## Usage

```rust
use agent_types::{ModelDialogueItem, Role, ContentPart, AgentEvent, FinishReason};

// Create a simple text message
let msg = ModelDialogueItem::text(Role::User, "Hello, world!");

// Access content
assert_eq!(msg.text_content(), "Hello, world!");

// Create a streaming request
let req = agent_types::ChatRequest::streaming(vec![msg])
    .with_model("gpt-4o")
    .with_temperature(0.7);
```

## Testing

Golden fixture tests verify compatibility with Python Rua blocks and
provider stream formats:

```text
fixtures/agent-types/python/     -- Python/AgentScope block fixtures
fixtures/agent-types/provider/   -- Provider stream event fixtures (JSONL)
```

Run all tests:

```bash
cargo test -p agent-types
```

Run golden fixture tests only:

```bash
cargo test -p agent-types golden
```
