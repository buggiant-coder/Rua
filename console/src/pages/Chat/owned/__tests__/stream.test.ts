import { afterEach, describe, expect, it, vi } from "vitest";
import {
  finishConversationStream,
  foldConversationStreamEvent,
  initialConversationStreamFold,
  interruptConversationStream,
  parseConversationStreamChunk,
} from "../stream";
import { readSseChunks } from "../sse";
import { createCanonicalStreamFixture } from "./canonicalMessageFixtures";

afterEach(() => {
  vi.useRealTimers();
});

function foldRaw(chunks: string[]) {
  const fixture = createCanonicalStreamFixture();

  return chunks
    .map((chunk) => {
      try {
        return parseConversationStreamChunk(
          JSON.stringify(fixture.normalize(JSON.parse(chunk))),
        );
      } catch {
        return parseConversationStreamChunk(chunk);
      }
    })
    .reduce(foldConversationStreamEvent, initialConversationStreamFold);
}

describe("owned conversation stream adapter", () => {
  it("projects a single typed text tool result to its payload", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "tool_result",
        message_id: "msg-tool",
        id: "call-tool",
        name: "task_await",
        content: {
          type: "text",
          text: '{"timed_out":true}',
        },
      }),
    ]);

    expect(state.output).toEqual([
      expect.objectContaining({
        type: "plugin_call_output",
        content: [
          expect.objectContaining({
            data: expect.objectContaining({ output: '{"timed_out":true}' }),
          }),
        ],
      }),
    ]);
  });

  it("projects ordered typed text parts without parsing their payloads", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "tool_result",
        message_id: "msg-tool",
        id: "call-tool",
        content: [
          { type: "text", text: '{"nested":"json"}' },
          { type: "text", text: "second part" },
        ],
      }),
    ]);

    expect(state.output).toEqual([
      expect.objectContaining({
        content: [
          expect.objectContaining({
            data: expect.objectContaining({
              output: '{"nested":"json"}\nsecond part',
            }),
          }),
        ],
      }),
    ]);
  });

  it("keeps provider response completion non-terminal", () => {
    const state = foldRaw([
      JSON.stringify({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "Hello" }],
          },
        ],
      }),
      JSON.stringify({
        object: "response",
        status: "completed",
        output: [
          {
            id: "assistant-1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "Hello done" }],
          },
        ],
      }),
    ]);

    expect(state.status).toBe("generating");
    expect(state.sawTerminal).toBe(false);
    expect(state.output[0].content[0].text).toBe("Hello done");
  });

  it("keeps prior output when provider completion has no content", () => {
    const state = foldRaw([
      JSON.stringify({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-1",
            role: "assistant",
            content: "visible text",
          },
        ],
      }),
      JSON.stringify({
        object: "response",
        status: "completed",
        output: [],
      }),
    ]);

    expect(state.status).toBe("generating");
    expect(state.sawTerminal).toBe(false);
    expect(state.output[0].content[0].text).toBe("visible text");
  });

  it("folds backend text deltas before the terminal frame", () => {
    const generating = foldRaw([
      JSON.stringify({ type: "start", chat_id: "chat-1", agent_id: "default" }),
      JSON.stringify({
        type: "text_delta",
        message_id: "msg-text-1",
        delta: "Hel",
      }),
      JSON.stringify({
        type: "text_delta",
        message_id: "msg-text-1",
        delta: "lo",
      }),
    ]);

    expect(generating.status).toBe("generating");
    expect(generating.output).toHaveLength(1);
    expect(generating.output[0].id).toBe("msg-text-1");
    expect(generating.output[0].type).toBe("message");
    expect(generating.output[0].content[0].text).toBe("Hello");

    const completed = foldConversationStreamEvent(
      generating,
      parseConversationStreamChunk(
        JSON.stringify({ type: "done", finish_reason: "stop" }),
      ),
    );

    expect(completed.status).toBe("generating");
    expect(completed.sawTerminal).toBe(false);
    expect(completed.output[0].content[0].text).toBe("Hello");
  });

  it("rejects a message-bearing delta without canonical message_id", () => {
    const event = parseConversationStreamChunk(
      JSON.stringify({ type: "text_delta", delta: "orphan" }),
    );

    expect(event.kind).toBe("parse_error");
  });

  it("folds backend usage frames into the live stream state", () => {
    const state = foldRaw([
      JSON.stringify({ type: "text_delta", delta: "Hello" }),
      JSON.stringify({
        type: "usage",
        usage: {
          prompt_tokens: 10500,
          completion_tokens: 770,
          total_tokens: 11270,
          cache_read_tokens: 4096,
        },
        last_call_usage: {
          prompt_tokens: 962,
          completion_tokens: 70,
          total_tokens: 1032,
          cache_read_tokens: 512,
        },
        max_input_length: 131100,
      }),
      JSON.stringify({ type: "done", finish_reason: "stop" }),
    ]);

    expect(state.status).toBe("generating");
    expect(state.sawTerminal).toBe(false);
    expect(state.usage?.usage).toMatchObject({
      prompt_tokens: 10500,
      completion_tokens: 770,
      total_tokens: 11270,
      cache_read_tokens: 4096,
    });
    expect(state.usage?.usage?.prompt_tokens).toBe(10500);
    expect(state.usage?.last_call_usage).toMatchObject({
      prompt_tokens: 962,
      completion_tokens: 70,
      total_tokens: 1032,
      cache_read_tokens: 512,
    });
    expect(state.usage?.max_input_length).toBe(131100);
    expect(state.output[0].content[0].text).toBe("Hello");
  });

  it("parses turn_terminal as the explicit terminal fact", () => {
    const generating = foldRaw([
      JSON.stringify({ type: "text_delta", delta: "Hello" }),
    ]);
    const event = parseConversationStreamChunk(
      JSON.stringify({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "completed",
        device_delivery_id: "device-delivery-1",
      }),
    );
    const completed = foldConversationStreamEvent(generating, event);

    expect(event).toMatchObject({
      kind: "turn_terminal",
      turnId: "turn-1",
      status: "completed",
      deviceDeliveryId: "device-delivery-1",
    });
    expect(completed.status).toBe("completed");
    expect(completed.sawTerminal).toBe(true);
    expect(completed.output[0].content[0].text).toBe("Hello");
  });

  it("ignores provider response frames after turn_terminal", () => {
    const terminal = foldRaw([
      JSON.stringify({ type: "text_delta", delta: "Hello" }),
      JSON.stringify({
        type: "turn_terminal",
        turn_id: "turn-1",
        status: "completed",
      }),
    ]);
    const trailing = foldConversationStreamEvent(
      terminal,
      parseConversationStreamChunk(
        JSON.stringify({
          object: "response",
          status: "completed",
          output: [
            {
              id: "provider-message-1",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "Hello again" }],
            },
          ],
        }),
      ),
    );

    expect(trailing).toBe(terminal);
    expect(trailing.output[0].content[0].text).toBe("Hello");
  });

  it("folds interjected turn_terminal with the interjection marker", () => {
    const generating = foldRaw([
      JSON.stringify({ type: "text_delta", delta: "partial" }),
    ]);
    const interjected = foldConversationStreamEvent(
      generating,
      parseConversationStreamChunk(
        JSON.stringify({
          type: "turn_terminal",
          turn_id: "turn-1",
          status: "interjected",
        }),
      ),
    );

    expect(interjected.status).toBe("interrupted");
    expect(interjected.sawTerminal).toBe(true);
    expect(interjected.output.map((message) => message.type)).toContain(
      "turn_interjected",
    );
    expect(JSON.stringify(interjected.output)).toContain("<turn_interjected>");
  });

  it("parses context compaction status without creating assistant output", () => {
    const event = parseConversationStreamChunk(
      JSON.stringify({
        type: "context_compaction",
        status: "started",
        mode: "blocking",
      }),
    );
    const state = foldConversationStreamEvent(
      initialConversationStreamFold,
      event,
    );

    expect(event.kind).toBe("context_compaction");
    expect(state).toEqual(initialConversationStreamFold);
  });

  it("parses conversation control without creating assistant output", () => {
    const event = parseConversationStreamChunk(
      JSON.stringify({
        type: "conversation_control",
        reason: "queue_accepted",
        control: {
          active_turn: null,
          queue: {
            entries: [
              {
                input_id: "input-1",
                state: "queued",
                text: "queued from authority",
                created_at: "2026-07-16T00:00:00Z",
                updated_at: "2026-07-16T00:00:00Z",
                editable: true,
                deletable: true,
              },
            ],
          },
          acp: { active: false },
          allowed_controls: {
            stop: false,
            queue: false,
            interject: false,
            edit_queued_input: false,
            delete_queued_input: false,
            switch_authority: true,
            enter_acp: true,
            exit_acp: false,
          },
        },
      }),
    );
    const state = foldConversationStreamEvent(
      initialConversationStreamFold,
      event,
    );

    expect(event.kind).toBe("conversation_control");
    if (event.kind === "conversation_control") {
      expect(event.reason).toBe("queue_accepted");
      expect(event.control.queue.entries[0].text).toBe("queued from authority");
    }
    expect(state).toEqual(initialConversationStreamFold);
  });

  it("parses history watermark readiness without creating assistant output", () => {
    const event = parseConversationStreamChunk(
      JSON.stringify({
        type: "history_watermark_reached",
        durable_sequence: 1,
        manifest_hash: "test-manifest-1",
        agent_id: "agent-a",
        chat_id: "chat-a",
        authority_device_id: "device-b",
      }),
    );
    const state = foldConversationStreamEvent(
      initialConversationStreamFold,
      event,
    );

    expect(event.kind).toBe("history_watermark_reached");
    if (event.kind === "history_watermark_reached") {
      expect(event.agentId).toBe("agent-a");
      expect(event.chatId).toBe("chat-a");
      expect(event.authorityDeviceId).toBe("device-b");
    }
    expect(state).toEqual(initialConversationStreamFold);
  });

  it("parses versioned history watermark readiness", () => {
    const event = parseConversationStreamChunk(
      JSON.stringify({
        type: "history_watermark_reached",
        agent_id: "agent-a",
        chat_id: "chat-a",
        authority_device_id: "device-b",
        durable_sequence: 15,
        manifest_hash: "manifest-15",
      }),
    );

    expect(event.kind).toBe("history_watermark_reached");
    if (event.kind === "history_watermark_reached") {
      expect(event.durableSequence).toBe(15);
      expect(event.manifestHash).toBe("manifest-15");
      expect(event.authorityDeviceId).toBe("device-b");
    }
    expect(
      foldConversationStreamEvent(initialConversationStreamFold, event),
    ).toEqual(initialConversationStreamFold);
  });

  it("does not parse unversioned history watermark readiness", () => {
    const event = parseConversationStreamChunk(
      JSON.stringify({ type: "history_watermark_reached" }),
    );
    expect(event.kind).not.toBe("history_watermark_reached");
  });

  it("parses a Live gap as an explicit history refresh request", () => {
    const event = parseConversationStreamChunk(
      JSON.stringify({
        type: "history_refresh_required",
        reason: "live_activity_gap",
        chat_id: "chat-a",
        requested_after_sequence: 4,
        first_available_sequence: 9,
      }),
    );

    expect(event.kind).toBe("history_refresh_required");
    if (event.kind === "history_refresh_required") {
      expect(event.reason).toBe("live_activity_gap");
      expect(event.requestedAfterSequence).toBe(4);
      expect(event.firstAvailableSequence).toBe(9);
    }
    expect(
      foldConversationStreamEvent(initialConversationStreamFold, event),
    ).toEqual(initialConversationStreamFold);
  });

  it("parses user message boundaries without adding assistant output", () => {
    const event = parseConversationStreamChunk(
      JSON.stringify({
        type: "user_message",
        message: {
          id: "user-queued",
          role: "user",
          content: [{ type: "text", text: "queued input" }],
        },
      }),
    );
    const state = foldConversationStreamEvent(
      initialConversationStreamFold,
      event,
    );

    expect(event).toMatchObject({
      kind: "user_message",
      message: {
        id: "user-queued",
        role: "user",
        content: [{ type: "text", text: "queued input" }],
      },
    });
    expect(state).toEqual(initialConversationStreamFold);
  });

  it("keeps empty text delta as an in-progress assistant bubble", () => {
    const state = foldConversationStreamEvent(
      initialConversationStreamFold,
      parseConversationStreamChunk(
        JSON.stringify({
          type: "text_delta",
          message_id: "msg-empty-text",
          delta: "",
        }),
      ),
    );

    expect(state.status).toBe("generating");
    expect(state.output).toHaveLength(1);
    expect(state.output[0].role).toBe("assistant");
    expect(state.output[0].status).toBe("in_progress");
    expect(state.output[0].content[0].text).toBe("");
  });

  it("retracts a failed model call before folding its retry", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "text_delta",
        message_id: "msg-call-1-text",
        delta: "partial",
      }),
      JSON.stringify({
        type: "thinking_delta",
        message_id: "msg-call-1-reasoning",
        delta: "partial reasoning",
      }),
      JSON.stringify({
        type: "tool_call_delta",
        message_id: "msg-tc-call-tool",
        id: "call-tool",
        index: 0,
        name: "example",
        arguments_delta: "{",
      }),
      JSON.stringify({
        type: "model_call_reset",
        model_call_id: "call-1",
      }),
      JSON.stringify({
        type: "text_delta",
        message_id: "msg-call-1-text",
        delta: "complete",
      }),
    ]);

    expect(state.output).toHaveLength(1);
    expect(state.output[0]).toEqual(
      expect.objectContaining({
        id: "msg-call-1-text",
        type: "message",
        status: "in_progress",
      }),
    );
    expect(state.output[0].content[0].text).toBe("complete");
  });

  it("retracts every Responses message in a failed model call", () => {
    const state = foldRaw([
      ...[
        "msg-previous-text-provider-1",
        "msg-call-1-text-provider-1",
        "msg-call-1-text-provider-2",
      ].map((message_id) =>
        JSON.stringify({
          type: "text_delta",
          message_id,
          delta: "partial",
        }),
      ),
      JSON.stringify({ type: "model_call_reset", model_call_id: "call-1" }),
      JSON.stringify({
        type: "text_delta",
        message_id: "msg-call-1-text-provider-retry",
        delta: "complete",
      }),
    ]);

    expect(state.output.map((message) => message.id)).toEqual([
      "msg-previous-text-provider-1",
      "msg-call-1-text-provider-retry",
    ]);
  });

  it("folds AgentScope content deltas before message completion", () => {
    const generating = foldRaw([
      JSON.stringify({
        object: "response",
        status: "in_progress",
        output: null,
      }),
      JSON.stringify({
        object: "message",
        status: "in_progress",
        id: "reasoning-1",
        type: "reasoning",
        role: "assistant",
        content: null,
      }),
      JSON.stringify({
        object: "content",
        status: "in_progress",
        type: "text",
        index: 0,
        delta: true,
        msg_id: "reasoning-1",
        text: "Need ",
      }),
      JSON.stringify({
        object: "content",
        status: "in_progress",
        type: "text",
        index: 0,
        delta: true,
        msg_id: "reasoning-1",
        text: "answer",
      }),
      JSON.stringify({
        object: "message",
        status: "in_progress",
        id: "message-1",
        type: "message",
        role: "assistant",
        content: null,
      }),
      JSON.stringify({
        object: "content",
        status: "in_progress",
        type: "text",
        index: 0,
        delta: true,
        msg_id: "message-1",
        text: "Hel",
      }),
    ]);

    expect(generating.status).toBe("generating");
    expect(generating.output).toHaveLength(2);
    expect(generating.output[0].type).toBe("reasoning");
    expect(generating.output[0].content[0].text).toBe("Need answer");
    expect(generating.output[1].type).toBe("message");
    expect(generating.output[1].content[0].text).toBe("Hel");

    const completed = foldConversationStreamEvent(
      generating,
      parseConversationStreamChunk(
        JSON.stringify({
          object: "message",
          status: "completed",
          id: "message-1",
          type: "message",
          role: "assistant",
          content: [
            {
              object: "content",
              status: "completed",
              type: "text",
              index: 0,
              text: "Hello",
            },
          ],
        }),
      ),
    );

    expect(completed.output).toHaveLength(2);
    expect(completed.output[0].type).toBe("reasoning");
    expect(completed.output[1].type).toBe("message");
    expect(completed.output[1].content[0].text).toBe("Hello");
  });

  it("folds backend thinking and tool deltas into visible output", () => {
    const state = foldRaw([
      JSON.stringify({ type: "thinking_delta", delta: "Need tool" }),
      JSON.stringify({
        type: "tool_call_delta",
        id: "call-1",
        index: 0,
        name: "Bash",
        arguments_delta: '{"command"',
      }),
      JSON.stringify({
        type: "tool_call_delta",
        id: "call-1",
        index: 0,
        arguments_delta: ':"echo hi"}',
      }),
      JSON.stringify({
        type: "tool_result_delta",
        id: "call-1",
        name: "Bash",
        output_delta: "hi",
      }),
    ]);

    expect(state.status).toBe("generating");
    expect(state.output[0].type).toBe("reasoning");
    expect(state.output[0].content[0].text).toBe("Need tool");
    expect(state.output[1].type).toBe("plugin_call");
    expect(state.output[1].content[0].data).toMatchObject({
      id: "call-1",
      name: "Bash",
      arguments: '{"command":"echo hi"}',
    });
    expect(state.output[2].type).toBe("plugin_call_output");
    expect(state.output[2].content[0].data).toMatchObject({
      id: "call-1",
      name: "Bash",
      output: "hi",
    });
  });

  it("preserves parallel tool identity across replay chunks and inverse results", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "stream_replay",
        complete: false,
        events: [
          {
            type: "tool_call_delta",
            id: "call-1",
            name: "Bash",
            arguments_delta: '{"command":"echo ',
          },
          {
            type: "tool_call",
            id: "call-2",
            name: "Read",
            arguments: '{"path":"README.md"}',
          },
        ],
      }),
      JSON.stringify({
        type: "stream_replay",
        complete: true,
        events: [
          {
            type: "tool_call_delta",
            id: "call-1",
            arguments_delta: 'first"}',
          },
          {
            type: "tool_result",
            id: "call-2",
            name: "Read",
            content: "second result",
          },
          {
            type: "tool_result_delta",
            id: "call-1",
            name: "Bash",
            output_delta: "first ",
          },
          {
            type: "tool_result",
            id: "call-1",
            name: "Bash",
            content: "first result",
          },
          {
            type: "tool_result",
            id: "call-2",
            name: "Read",
            content: "second result",
          },
        ],
      }),
    ]);

    expect(state.output).toHaveLength(4);
    expect(state.output.map((message) => message.tool_call_id)).toEqual([
      "call-1",
      "call-2",
      "call-2",
      "call-1",
    ]);
    expect(state.output[0].content[0].data).toMatchObject({
      arguments: '{"command":"echo first"}',
    });
    expect(state.output[2].content[0].data).toMatchObject({
      output: "second result",
    });
    expect(state.output[3].content[0].data).toMatchObject({
      output: "first result",
    });
  });

  it("folds WebUI data content frames into live tool state", () => {
    const state = foldRaw([
      JSON.stringify({
        object: "message",
        status: "in_progress",
        id: "msg-tc-call-1",
        type: "plugin_call",
        role: "assistant",
        content: null,
      }),
      JSON.stringify({
        object: "content",
        status: "in_progress",
        type: "data",
        index: 0,
        delta: false,
        msg_id: "msg-tc-call-1",
        data: { call_id: "call-1", name: "", arguments: "" },
      }),
      JSON.stringify({
        object: "content",
        status: "in_progress",
        type: "data",
        index: 0,
        delta: false,
        msg_id: "msg-tc-call-1",
        data: { call_id: "call-1", name: "Write" },
      }),
      JSON.stringify({
        object: "content",
        status: "in_progress",
        type: "data",
        index: 0,
        delta: true,
        msg_id: "msg-tc-call-1",
        data: {
          call_id: "call-1",
          name: "Write",
          arguments_delta: '{"command"',
        },
      }),
      JSON.stringify({
        object: "content",
        status: "in_progress",
        type: "data",
        index: 1,
        delta: true,
        msg_id: "msg-tc-call-1",
        data: {
          call_id: "call-1",
          name: "Write",
          arguments_delta: ':"echo hi"}',
        },
      }),
      JSON.stringify({
        object: "message",
        status: "in_progress",
        id: "msg-tr-call-1",
        type: "plugin_call_output",
        role: "tool",
        content: null,
      }),
      JSON.stringify({
        object: "content",
        status: "in_progress",
        type: "data",
        index: 0,
        delta: false,
        msg_id: "msg-tr-call-1",
        data: { call_id: "call-1", name: "Bash", output: "hi" },
      }),
    ]);

    expect(state.output[0].content[0].data).toMatchObject({
      call_id: "call-1",
      name: "Write",
      arguments: '{"command":"echo hi"}',
    });
    expect(state.output[1].content[0].data).toMatchObject({
      call_id: "call-1",
      name: "Bash",
      output: "hi",
    });
  });

  it("adds a local turn-aborted marker when interrupted", () => {
    const generating = foldRaw([
      JSON.stringify({ type: "thinking_delta", delta: "Need tool" }),
      JSON.stringify({
        type: "tool_call",
        id: "call-1",
        name: "bash",
        arguments: '{"command":"sleep 10"}',
      }),
    ]);

    const interrupted = interruptConversationStream(generating);

    expect(interrupted.status).toBe("interrupted");
    expect(interrupted.sawTerminal).toBe(false);
    expect(interrupted.output.map((message) => message.type)).toEqual([
      "reasoning",
      "plugin_call",
      "turn_aborted",
    ]);
    expect(interrupted.output[2]).toMatchObject({
      role: "system",
      status: "interrupted",
    });
  });

  it("patches a tool name from a title-only tool delta", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "tool_call_delta",
        id: "call-1",
        index: 0,
        arguments_delta: '{"path":"README.md"}',
      }),
      JSON.stringify({
        type: "tool_call_delta",
        id: "call-1",
        index: 0,
        name: "Read File",
        arguments_delta: "",
      }),
    ]);

    expect(state.output).toHaveLength(1);
    expect(state.output[0].type).toBe("plugin_call");
    expect(state.output[0].content[0].data).toMatchObject({
      id: "call-1",
      name: "Read File",
      arguments: '{"path":"README.md"}',
    });
  });

  it("does not create a thinking block from whitespace after a tool", () => {
    const state = foldRaw([
      JSON.stringify({ type: "thinking_delta", delta: "Need tool" }),
      JSON.stringify({
        type: "tool_call_delta",
        id: "call-1",
        index: 0,
        name: "Bash",
        arguments_delta: '{"command":"echo hi"}',
      }),
      JSON.stringify({ type: "thinking_delta", delta: "\n" }),
      JSON.stringify({
        type: "tool_call_delta",
        id: "call-2",
        index: 0,
        name: "Bash",
        arguments_delta: '{"command":"pwd"}',
      }),
    ]);

    expect(state.output.map((item) => item.type)).toEqual([
      "reasoning",
      "plugin_call",
      "plugin_call",
    ]);
  });

  it("promotes trailing completed reasoning after a tool result to visible text", () => {
    const state = foldRaw([
      JSON.stringify({
        object: "message",
        status: "completed",
        id: "reasoning-before-tool",
        type: "reasoning",
        role: "assistant",
        content: [{ type: "text", text: "Need to run the command." }],
      }),
      JSON.stringify({
        object: "message",
        status: "completed",
        id: "tool-call",
        type: "plugin_call",
        role: "assistant",
        content: [{ type: "data", data: { call_id: "call-1" } }],
      }),
      JSON.stringify({
        object: "message",
        status: "completed",
        id: "tool-output",
        type: "plugin_call_output",
        role: "tool",
        content: [
          { type: "data", data: { call_id: "call-1", output: "123\n" } },
        ],
      }),
      JSON.stringify({
        object: "message",
        status: "completed",
        id: "reasoning-final",
        type: "reasoning",
        role: "assistant",
        content: [
          {
            type: "text",
            text: "命令已成功执行，等待 10 秒后输出了 `123`。",
          },
        ],
      }),
      JSON.stringify({ object: "response", status: "completed", output: [] }),
    ]);

    expect(state.status).toBe("generating");
    expect(state.sawTerminal).toBe(false);
    expect(state.output.map((msg) => msg.type)).toEqual([
      "reasoning",
      "plugin_call",
      "plugin_call_output",
      "reasoning",
    ]);
  });

  it("does not turn EOF without a runtime terminal frame into terminal state", () => {
    const generating = foldRaw([
      JSON.stringify({
        object: "response",
        status: "in_progress",
        output: [
          {
            id: "assistant-1",
            role: "assistant",
            content: "partial",
          },
        ],
      }),
    ]);

    const finished = finishConversationStream(generating);

    expect(finished.status).toBe("generating");
    expect(finished.sawTerminal).toBe(false);
    expect(finished.output[0].content[0].text).toBe("partial");
  });

  it("folds explicit interrupted runtime terminal frames separately from failures", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "interrupted",
        reason: "user_stop",
      }),
    ]);

    expect(state.status).toBe("interrupted");
    expect(state.error).toBeNull();
    expect(state.sawTerminal).toBe(false);
  });

  it("folds interjected terminal frames with the interjection marker", () => {
    const state = foldRaw([
      JSON.stringify({ type: "text_delta", delta: "partial" }),
      JSON.stringify({
        type: "interrupted",
        reason: "interjected",
      }),
    ]);

    expect(state.status).toBe("interrupted");
    expect(state.sawTerminal).toBe(false);
    expect(state.output.map((msg) => msg.type)).toEqual([
      "message",
      "turn_interjected",
    ]);
    expect(JSON.stringify(state.output)).toContain("<turn_interjected>");
    expect(JSON.stringify(state.output)).not.toContain("<turn_aborted>");
  });

  it("keeps completed tool results completed when a later stop interrupts the run", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "tool_result",
        id: "call-1",
        name: "Bash",
        content: "done",
      }),
      JSON.stringify({
        type: "interrupted",
        reason: "user_stop",
      }),
    ]);

    expect(state.status).toBe("interrupted");
    expect(state.output[0].type).toBe("plugin_call_output");
    expect(state.output[0].status).toBe("completed");
  });

  it("keeps provider failures non-terminal but parse failures terminal", () => {
    const failed = foldRaw([
      JSON.stringify({
        object: "response",
        status: "failed",
        error: { message: "boom" },
      }),
    ]);
    expect(failed.status).toBe("failed");
    expect(failed.error).toEqual({ message: "boom" });
    expect(failed.sawTerminal).toBe(false);

    const malformed = foldRaw(["{not json"]);
    expect(malformed.status).toBe("failed");
    expect(malformed.error).toBeInstanceOf(Error);
    expect(malformed.sawTerminal).toBe(true);
  });

  it("parses Runtime Notes as output events", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "runtime_note",
        runtime_note: {
          runtime_note_id: "note-1",
          kind: "task_attempt_terminal",
          created_at: "2026-08-03T00:00:00Z",
          payload: {
            task_id: "task-1",
            terminal_status: "completed",
            output: "done",
          },
        },
      }),
    ]);

    expect(state.output[0].type).toBe("runtime_note");
    expect(state.output[0].runtime_note?.payload.task_id).toBe("task-1");
  });

  it.each(["completed", "failed", "partial"])(
    "keeps %s collaboration results out of stream output",
    (status) => {
      const state = foldRaw([
        JSON.stringify({
          type: "runtime_note",
          runtime_note: {
            runtime_note_id: "collaboration-note",
            kind: "collaboration_result",
            created_at: "2026-08-03T00:00:00Z",
            payload: { status, output: "Internal collaboration output" },
          },
        }),
      ]);

      expect(state).toEqual(initialConversationStreamFold);
    },
  );

  it("keeps memory signal runtime notes out of stream output", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "runtime_note",
        runtime_note: {
          runtime_note_id: "runtime-note:memory-signal:memory-signal-1",
          kind: "memory_signal",
          created_at: "2026-08-03T00:00:00Z",
          payload: {
            signal_kind: "convention",
            confidence: "high",
            suggested_memory_kind: "preference",
            suggested_scope: "user",
            evidence: "convention signal detected for Memory management",
          },
        },
      }),
    ]);

    expect(state).toEqual(initialConversationStreamFold);
  });

  it("keeps tool approval request frames out of stream output", () => {
    const toolApproval = foldRaw([
      JSON.stringify({
        type: "tool_call",
        id: "call-1",
        name: "Bash",
        arguments: '{"cmd":"rm -rf tmp"}',
      }),
      JSON.stringify({
        type: "tool_approval_request",
        request_id: "approval-1",
        call_id: "call-1",
        tool_name: "Bash",
        prompt: "Bash requires confirmation",
      }),
    ]);

    expect(toolApproval.output).toHaveLength(1);
    expect(toolApproval.output[0].type).toBe("plugin_call");
    expect(toolApproval.output[0].content[0].data).not.toHaveProperty(
      "approval",
    );
  });

  it("keeps orphan tool approval request frames out of tool cards", () => {
    const toolApprovalWithArguments = foldRaw([
      JSON.stringify({
        type: "tool_approval_request",
        id: "approval-2",
        name: "Write",
        arguments: { path: "file.txt" },
      }),
    ]);

    expect(toolApprovalWithArguments.output).toHaveLength(0);
  });

  it("does not synthesize approval cards when approval arrives before tool_call", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "tool_approval_request",
        request_id: "approval-3",
        call_id: "call-late",
        tool_name: "Bash",
        prompt: "Bash requires confirmation",
      }),
      JSON.stringify({
        type: "tool_call",
        id: "call-late",
        name: "Bash",
        arguments: '{"cmd":"rm -rf tmp"}',
      }),
    ]);

    expect(state.output).toHaveLength(1);
    expect(state.output[0].type).toBe("plugin_call");
    expect(state.output[0].content[0].data).toMatchObject({
      id: "call-late",
      name: "Bash",
      arguments: '{"cmd":"rm -rf tmp"}',
    });
    expect(state.output[0].content[0].data).not.toHaveProperty("approval");
  });

  it("does not render a second approval card when one approval frame lacks call_id", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "tool_call",
        id: "call-real",
        name: "Bash",
        arguments: '{"command":"rm -rf tmp"}',
      }),
      JSON.stringify({
        type: "tool_approval_request",
        request_id: "approval-4",
        id: "approval-4",
        name: "Bash",
        arguments: null,
      }),
      JSON.stringify({
        type: "tool_approval_request",
        request_id: "approval-4",
        call_id: "call-real",
        tool_name: "Bash",
        arguments: { command: "rm -rf tmp" },
        prompt: "Bash requires confirmation",
      }),
    ]);

    const approvalCards = state.output.filter(
      (message) =>
        message.type === "plugin_call" &&
        message.content.some((part) => {
          const data = part.data;
          return Boolean(
            data &&
              typeof data === "object" &&
              (data as Record<string, unknown>).approval,
          );
        }),
    );

    expect(approvalCards).toHaveLength(0);
    expect(state.output).toHaveLength(1);
    expect(state.output[0].id).toBe("msg-tc-call-real");
    expect(state.output[0].content[0].data).toMatchObject({
      id: "call-real",
      name: "Bash",
      arguments: '{"command":"rm -rf tmp"}',
    });
    expect(state.output[0].content[0].data).not.toHaveProperty("approval");
  });

  it("parses ask_user frames as user-answer requests, not approvals", () => {
    const state = foldRaw([
      JSON.stringify({
        type: "ask_user_request",
        id: "ask-1",
        request_id: "ask-1",
        prompt: "Which option?",
        questions: [
          {
            question: "Which option?",
            options: ["A", "B"],
            multi_select: false,
          },
        ],
      }),
    ]);

    expect(state.output[0].type).toBe("ask_user_request");
    expect(state.output[0].content[0].type).toBe("ask_user_request");
    expect(state.output[0].content[0].data).toMatchObject({
      request_id: "ask-1",
      prompt: "Which option?",
      questions: [
        {
          question: "Which option?",
          options: ["A", "B"],
          multi_select: false,
        },
      ],
      status: "waiting",
    });
    expect(state.output[0].content[0].type).not.toBe("tool_approval_request");
  });

  it("parses WebUI ask_user_request message payloads", () => {
    const state = foldRaw([
      JSON.stringify({
        object: "message",
        status: "completed",
        id: "msg-ask-1",
        type: "ask_user_request",
        role: "assistant",
        content: [
          {
            object: "content",
            status: "completed",
            type: "ask_user_request",
            index: 0,
            msg_id: "msg-ask-1",
            data: {
              id: "ask-1",
              request_id: "ask-1",
              prompt: "Which option?",
              questions: [
                {
                  question: "Which option?",
                  options: ["A", "B"],
                  multi_select: false,
                },
              ],
              status: "waiting",
            },
          },
        ],
      }),
    ]);

    expect(state.output[0].type).toBe("ask_user_request");
    expect(state.output[0].content[0].type).toBe("ask_user_request");
    expect(state.output[0].content[0].data).toMatchObject({
      request_id: "ask-1",
      status: "waiting",
      questions: [{ question: "Which option?", options: ["A", "B"] }],
    });
  });

  it("reads consecutive SSE frames", async () => {
    const encoder = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(
          encoder.encode(
            'data: {"object":"response","status":"in_progress"}\n\n',
          ),
        );
        controller.enqueue(
          encoder.encode(
            'data: {"object":"response","status":"completed"}\n\n',
          ),
        );
        controller.close();
      },
    });

    const chunks = [];
    for await (const chunk of readSseChunks(body)) {
      chunks.push(chunk.data);
    }

    expect(chunks).toEqual([
      '{"object":"response","status":"in_progress"}',
      '{"object":"response","status":"completed"}',
    ]);
  });

  it("times out and cancels a stream that stops producing bytes", async () => {
    vi.useFakeTimers();
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      cancel() {
        cancelled = true;
      },
    });

    const consume = (async () => {
      for await (const _chunk of readSseChunks(body, { idleTimeoutMs: 100 })) {
      }
    })();
    const assertion = expect(consume).rejects.toMatchObject({
      name: "SseReadIdleTimeoutError",
    });

    await vi.advanceTimersByTimeAsync(100);
    await assertion;
    expect(cancelled).toBe(true);
  });

  it("resets the idle timeout whenever heartbeat bytes arrive", async () => {
    vi.useFakeTimers();
    const encoder = new TextEncoder();
    let controller: ReadableStreamDefaultController<Uint8Array>;
    const body = new ReadableStream<Uint8Array>({
      start(streamController) {
        controller = streamController;
      },
    });
    const chunks: string[] = [];
    const consume = (async () => {
      for await (const chunk of readSseChunks(body, { idleTimeoutMs: 100 })) {
        chunks.push(chunk.data);
      }
    })();

    controller!.enqueue(encoder.encode(": keepalive\n\n"));
    await vi.advanceTimersByTimeAsync(75);
    controller!.enqueue(encoder.encode(": keepalive\n\n"));
    await vi.advanceTimersByTimeAsync(75);
    controller!.enqueue(encoder.encode('data: {"type":"done"}\n\n'));
    controller!.close();
    await consume;

    expect(chunks).toEqual(['{"type":"done"}']);
  });

  it("does not accept bare NDJSON payloads", async () => {
    const encoder = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoder.encode('{"type":"done"}\n'));
        controller.close();
      },
    });
    const chunks = [];

    for await (const chunk of readSseChunks(body)) {
      chunks.push(chunk.data);
    }

    expect(chunks).toEqual([]);
  });
});
