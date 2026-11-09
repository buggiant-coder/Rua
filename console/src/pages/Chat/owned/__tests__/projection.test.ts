import { describe, expect, it } from "vitest";
import {
  assistantResponseText,
  createConversationProjector,
  projectConversation as projectConversationImpl,
  type ConversationAssistantResponse,
  type ConversationRuntimeNote,
} from "../projection";
import type { ChatHistory } from "@/api/types";
import { canonicalHistoryFixture } from "./canonicalMessageFixtures";

function projectConversation(
  ...args: Parameters<typeof projectConversationImpl>
): ReturnType<typeof projectConversationImpl> {
  const [conversationId, history, options] = args;
  return projectConversationImpl(
    conversationId,
    canonicalHistoryFixture(history),
    options,
  );
}

function assistantAt(
  projection: ReturnType<typeof projectConversation>,
  index: number,
): ConversationAssistantResponse {
  const item = projection.items[index];
  expect(item.kind).toBe("assistant");
  return item as ConversationAssistantResponse;
}

describe("projectConversation", () => {
  it("preserves completed response identity and objects when older turns are prepended", () => {
    const project = createConversationProjector();
    const user = {
      id: "user-new",
      role: "user",
      turn_id: "turn-new",
      content: "New question",
    };
    const answer = {
      id: "answer-new",
      role: "assistant",
      turn_id: "turn-new",
      content: "New answer",
    };
    const before = project("chat", { messages: [user, answer] });
    const after = project("chat", {
      messages: [
        {
          id: "user-old",
          role: "user",
          turn_id: "turn-old",
          content: "Old question",
        },
        {
          id: "answer-old",
          role: "assistant",
          turn_id: "turn-old",
          content: "Old answer",
        },
        user,
        answer,
      ],
    });
    expect(after.items[3]).toBe(before.items[1]);
    expect(after.items[2]).toBe(before.items[0]);
  });

  it("does not transfer generating state from a new active turn to the completed previous turn", () => {
    const projection = projectConversation("chat-1", {
      status: "running",
      active_turn: {
        turn_id: "turn-2",
        producer_id: "producer-2",
        generation: 2,
        phase: "preparing",
        device_id: "device-1",
        started_at: "2026-08-24T00:00:01Z",
      },
      messages: [
        {
          id: "user-1",
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "Previous question" }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          turn_id: "turn-1",
          type: "message",
          content: [{ type: "text", text: "Previous answer" }],
        },
      ],
    });

    expect(projection.items).toHaveLength(2);
    expect(assistantAt(projection, 1).status).toBe("completed");
    expect(projection.turnActivity).toMatchObject({
      kind: "active",
      responseId: null,
    });
  });

  it("does not bind a completed previous response without current live projection evidence", () => {
    const projection = projectConversation("chat-1", {
      status: "running",
      active_turn: {
        turn_id: "turn-2",
        phase: "preparing",
        authority_device_id: "desktop",
        driver: "rua",
        started_at: "2026-08-30T00:00:00Z",
        queued_count: 0,
      },
      messages: [
        {
          id: "user-1",
          role: "user",
          turn_id: "turn-1",
          content: "Previous question",
        },
        {
          id: "assistant-1",
          role: "assistant",
          turn_id: "turn-1",
          content: "Previous answer",
        },
      ],
    });

    expect(assistantAt(projection, 1).status).toBe("completed");
  });

  it("binds an active live Shell response even before messages carry the Turn id", () => {
    const projection = projectConversation(
      "chat-1",
      {
        status: "running",
        active_turn: {
          turn_id: "turn-shell",
          phase: "streaming",
          authority_device_id: "desktop",
          driver: "rua",
          started_at: "2026-08-30T00:00:00Z",
          queued_count: 0,
        },
        messages: [
          { id: "user-shell", role: "user", content: "Start servers" },
          {
            id: "shell-call",
            role: "assistant",
            type: "plugin_call",
            content: [
              {
                type: "data",
                data: { call_id: "call-shell", name: "bash" },
              },
            ],
          },
        ],
      },
      { liveActive: true, hasLiveProjection: true },
    );

    expect(projection.turnActivity).toMatchObject({
      kind: "active",
      turnId: "turn-shell",
      responseId: "chat-1:assistant:user-shell",
    });
  });

  it("projects persisted user and assistant messages into owned items", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: [{ type: "text", text: "Question" }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Answer" }],
        },
      ],
    });

    expect(projection.conversationId).toBe("chat-1");
    expect(projection.turnActivity).toEqual({ kind: "idle" });
    expect(projection.items).toHaveLength(2);
    expect(projection.items[0]).toMatchObject({
      kind: "user",
      id: "user-1",
    });
    expect(projection.items[1]).toMatchObject({
      kind: "assistant",
      id: "chat-1:assistant:user-1",
      status: "completed",
    });
    expect(assistantResponseText(assistantAt(projection, 1))).toBe("Answer");
  });

  it("preserves a persisted single typed text content part", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { id: "user-1", role: "user", content: "Run a tool" },
        {
          id: "tool-output",
          role: "system",
          type: "plugin_call_output",
          content: { type: "text", text: '{"timed_out":true}' },
        },
      ],
    });

    const assistant = assistantAt(projection, 1);
    expect(assistant.output[0].content).toEqual([
      { type: "text", text: '{"timed_out":true}' },
    ]);
  });

  it("carries ACP origin from a user turn onto its assistant response", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "Question",
          metadata: {
            origin: {
              kind: "acp",
              runner: "codex",
            },
          },
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: "Answer",
        },
      ],
    });

    expect(projection.items[0]).toMatchObject({
      kind: "user",
      acp_origin_runner: "codex",
    });
    expect(assistantAt(projection, 1)).toMatchObject({
      kind: "assistant",
      acp_origin_runner: "codex",
    });
  });

  it("groups consecutive non-user messages into one assistant response", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          role: "user",
          content: "Run a tool",
        },
        {
          id: "reasoning-1",
          role: "assistant",
          type: "reasoning",
          content: [{ type: "text", text: "thinking" }],
        },
        {
          id: "tool-1",
          role: "assistant",
          type: "plugin_call",
          content: [{ data: { name: "Bash" } }],
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Done" }],
        },
      ],
    });

    const assistant = assistantAt(projection, 1);
    expect(assistant.output.map((msg) => msg.id)).toEqual([
      "reasoning-1",
      "tool-1",
      "assistant-1",
    ]);
  });

  it("projects ACP history preview as a separate UI-only item", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "assistant-before",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "previous answer" }],
        },
        {
          id: "acp-history-preview-message-1",
          role: "ui",
          type: "acp_history_preview",
          turn_id: "acp-history-preview-1",
          content: [{ type: "text", text: "previous ACP transcript" }],
          metadata: {
            ui_only: true,
            model_visible: false,
            acp_history_preview: true,
            origin: { kind: "acp", runner: "codex" },
          },
        },
      ],
    });

    expect(projection.items).toHaveLength(2);
    expect(projection.items[0]).toMatchObject({
      kind: "assistant",
    });
    expect(projection.items[1]).toMatchObject({
      kind: "acp_history_preview",
      id: "acp-history-preview-message-1",
      turn_id: "acp-history-preview-1",
      acp_origin_runner: "codex",
    });
    expect(assistantResponseText(assistantAt(projection, 0))).toBe(
      "previous answer",
    );
  });

  it("treats reasoning-only assistant output as visible message fallback", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          role: "user",
          content: "Tell another joke",
        },
        {
          id: "reasoning-only",
          role: "assistant",
          type: "reasoning",
          content: [
            { type: "text", text: "Visible answer trapped in thinking" },
          ],
        },
      ],
    });

    const assistant = assistantAt(projection, 1);
    expect(assistant.output).toHaveLength(1);
    expect(assistant.output[0].type).toBe("message");
    expect(assistant.output[0].reasoning_only_fallback).toBe(true);
    expect(assistantResponseText(assistant)).toBe(
      "Visible answer trapped in thinking",
    );
  });

  it("keeps trailing reasoning after a tool result as reasoning", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Run a command" },
        {
          id: "reasoning-before-tool",
          role: "assistant",
          type: "reasoning",
          content: [{ type: "text", text: "Need to run the command." }],
        },
        {
          id: "tool-call",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: { call_id: "call-1", name: "Bash" },
            },
          ],
        },
        {
          id: "tool-output",
          role: "system",
          type: "plugin_call_output",
          content: [
            {
              type: "data",
              data: { call_id: "call-1", name: "Bash", output: "123\n" },
            },
          ],
        },
        {
          id: "reasoning-final",
          role: "assistant",
          type: "reasoning",
          content: [
            {
              type: "text",
              text: "命令已成功执行，等待 10 秒后输出了 `123`。",
            },
          ],
        },
      ],
    });

    const assistant = assistantAt(projection, 1);
    expect(assistant.output.map((msg) => msg.type)).toEqual([
      "reasoning",
      "plugin_call",
      "plugin_call_output",
      "reasoning",
    ]);
    expect(assistant.output[3].content[0].text).toBe(
      "命令已成功执行，等待 10 秒后输出了 `123`。",
    );
  });

  it("projects Assistant Response Variants without index assumptions", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Question" },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "Selected" }],
          turn_id: "turn-1",
          selected_variant_id: "variant-2",
          assistant_response_variants: [
            {
              variant_id: "variant-1",
              output: [
                {
                  id: "assistant-1",
                  role: "assistant",
                  type: "message",
                  content: [{ type: "text", text: "First" }],
                },
              ],
            },
            {
              variant_id: "variant-2",
              output: [
                {
                  id: "assistant-1",
                  role: "assistant",
                  type: "message",
                  content: [{ type: "text", text: "Selected" }],
                },
              ],
            },
          ],
        },
      ],
    });

    const assistant = assistantAt(projection, 1);
    expect(assistant.turn_id).toBe("turn-1");
    expect(assistant.selected_variant_id).toBe("variant-2");
    expect(assistant.variants).toHaveLength(2);
    expect(assistant.variants[1].output[0].content[0].text).toBe("Selected");
  });

  it("uses the selected assistant response variant as the complete turn output", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        { role: "user", content: "Run a slow command" },
        {
          id: "assistant-tool",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                name: "Bash",
                arguments: '{"command":"sleep 10 && echo 123"}',
              },
            },
          ],
          turn_id: "turn-1",
          selected_variant_id: "variant-tool",
          assistant_response_variants: [
            {
              variant_id: "variant-tool",
              output: [
                {
                  id: "assistant-tool",
                  role: "assistant",
                  type: "plugin_call",
                  content: [
                    {
                      type: "data",
                      data: {
                        call_id: "call-1",
                        name: "Bash",
                        arguments: '{"command":"sleep 10 && echo 123"}',
                      },
                    },
                  ],
                },
              ],
            },
          ],
        },
        {
          id: "assistant-final",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: "命令已成功执行。" }],
        },
        {
          id: "tool-output",
          role: "system",
          type: "plugin_call_output",
          content: [
            {
              type: "data",
              data: {
                call_id: "call-1",
                name: "Bash",
                output: "123\n",
              },
            },
          ],
        },
      ],
    });

    const assistant = assistantAt(projection, 1);
    expect(assistant.output.map((msg) => msg.id)).toEqual(["assistant-tool"]);
    expect(assistantResponseText(assistant)).toBe("");
  });

  it("does not mix the previous selected sibling output during optimistic variant switches", () => {
    const projection = projectConversation(
      "chat-1",
      {
        status: "idle",
        messages: [
          { role: "user", content: "Tell a joke" },
          {
            id: "reasoning-current",
            role: "assistant",
            type: "reasoning",
            content: [{ type: "text", text: "Old reasoning" }],
            turn_id: "turn-1",
            selected_variant_id: "variant-1",
            assistant_response_variants: [
              {
                variant_id: "variant-1",
                output: [
                  {
                    id: "reasoning-v1",
                    role: "assistant",
                    type: "reasoning",
                    content: [{ type: "text", text: "Old reasoning" }],
                  },
                  {
                    id: "message-v1",
                    role: "assistant",
                    type: "message",
                    content: [{ type: "text", text: "Old answer" }],
                  },
                ],
              },
              {
                variant_id: "variant-2",
                output: [
                  {
                    id: "reasoning-v2",
                    role: "assistant",
                    type: "reasoning",
                    content: [{ type: "text", text: "New reasoning" }],
                  },
                  {
                    id: "message-v2",
                    role: "assistant",
                    type: "message",
                    content: [{ type: "text", text: "New answer" }],
                  },
                ],
              },
            ],
          },
          {
            id: "message-v1",
            role: "assistant",
            type: "message",
            content: [{ type: "text", text: "Old answer" }],
          },
        ],
      },
      { selectedVariantOverrides: { "turn-1": "variant-2" } },
    );

    const assistant = assistantAt(projection, 1);
    expect(assistant.output.map((msg) => msg.id)).toEqual([
      "reasoning-v2",
      "message-v2",
    ]);
    expect(assistantResponseText(assistant)).toContain("New answer");
    expect(assistantResponseText(assistant)).not.toContain("Old answer");
  });

  it("appends Runtime Notes from chat history onto the projection", () => {
    const note: ConversationRuntimeNote = {
      runtime_note_id: "note-1",
      kind: "task_attempt_terminal",
      created_at: "2026-08-03T00:00:00Z",
      payload: {
        task_id: "task-1",
        runner_name: "Bash",
        terminal_status: "completed",
        output: "finished",
      },
    };
    const history = {
      status: "idle",
      messages: [
        { role: "user", content: "Start task", turn_id: "turn-1" },
        {
          id: "note-1",
          role: "system",
          type: "runtime_note",
          content: [],
          turn_id: "turn-1",
          runtime_note: note,
        },
      ],
    } as ChatHistory;
    const projection = projectConversation("chat-1", history);

    const assistant = assistantAt(projection, 1);
    expect(assistant.output[0].type).toBe("runtime_note");
    expect(assistant.output[0].runtime_note.payload.task_id).toBe("task-1");
    expect(assistant.output[0].runtime_note.payload.output).toBe("finished");
  });

  it("projects conversation terminal runtime notes as visible interrupted responses", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "执行一个 sleep 命令",
          turn_id: "turn-stopped-1",
        },
        {
          id: "note-stop-1",
          role: "system",
          type: "runtime_note",
          content: [],
          turn_id: "turn-stopped-1",
          runtime_note: {
            runtime_note_id: "note-stop-1",
            kind: "conversation_runtime_terminal",
            created_at: "2026-08-03T00:00:00Z",
            payload: {
              turn_id: "turn-stopped-1",
              terminal_status: "interrupted",
              summary: "conversation interrupted by runtime restart",
            },
          },
        },
        {
          id: "user-2",
          role: "user",
          content: "我打断你了，你继续",
          turn_id: "turn-stopped-2",
        },
        {
          id: "note-stop-2",
          role: "system",
          type: "runtime_note",
          content: [],
          turn_id: "turn-stopped-2",
          runtime_note: {
            runtime_note_id: "note-stop-2",
            kind: "conversation_runtime_terminal",
            created_at: "2026-08-03T00:00:00Z",
            payload: {
              turn_id: "turn-stopped-2",
              terminal_status: "interrupted",
              summary: "conversation interrupted by user stop",
            },
          },
        },
        {
          id: "user-3",
          role: "user",
          content: "你好慢啊，我换了个模型",
          turn_id: "turn-current",
        },
        {
          id: "assistant-3",
          role: "assistant",
          type: "message",
          content: "看起来你拒绝了这条命令的执行",
          turn_id: "turn-current",
        },
      ],
    } as ChatHistory);

    expect(projection.items).toHaveLength(6);
    expect(projection.items[0]).toMatchObject({ kind: "user", id: "user-1" });
    const firstInterrupted = assistantAt(projection, 1);
    expect(firstInterrupted.status).toBe("interrupted");
    expect(firstInterrupted.output[0].type).toBe("runtime_note");
    expect(firstInterrupted.output[0].runtime_note.payload.summary).toBe(
      "conversation interrupted by runtime restart",
    );

    expect(projection.items[2]).toMatchObject({ kind: "user", id: "user-2" });
    const secondInterrupted = assistantAt(projection, 3);
    expect(secondInterrupted.status).toBe("interrupted");

    expect(projection.items[4]).toMatchObject({ kind: "user", id: "user-3" });

    const currentAssistant = assistantAt(projection, 5);
    expect(currentAssistant.turn_id).toBe("turn-current");
    expect(currentAssistant.output.map((msg) => msg.id)).toEqual([
      "assistant-3",
    ]);
  });

  it.each(["completed", "failed", "partial"])(
    "omits %s collaboration results without hiding surrounding messages",
    (status) => {
      const messages = [
        {
          id: "user-1",
          role: "user",
          content: "Investigate",
          turn_id: "turn-1",
        },
        {
          id: "assistant-1",
          role: "assistant",
          content: "Investigating",
          turn_id: "turn-1",
        },
        {
          id: "collaboration-note",
          role: "system",
          content: "Internal collaboration output",
          turn_id: "turn-1",
          runtime_note: {
            runtime_note_id: "collaboration-note",
            kind: "collaboration_result",
            created_at: "2026-08-03T00:00:00Z",
            payload: { status, output: "Internal collaboration output" },
          },
        },
        {
          id: "assistant-2",
          role: "assistant",
          content: "Investigation finished",
          turn_id: "turn-1",
        },
      ];
      const projection = projectConversation("chat-1", { messages });

      expect(projection.items).toHaveLength(2);
      expect(projection.items[0]).toMatchObject({ kind: "user", id: "user-1" });
      expect(assistantAt(projection, 1).output.map((msg) => msg.id)).toEqual([
        "assistant-1",
        "assistant-2",
      ]);
      expect(JSON.stringify(projection.items)).not.toContain("collaboration");
      expect(
        projectConversation("chat-1", { messages: [messages[2]] }).items,
      ).toEqual([]);
      expect(messages[2].runtime_note?.payload.status).toBe(status);
    },
  );

  it("omits memory signal runtime notes from the visible projection", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "为啥不能直接进 BIOS 做设置？",
          turn_id: "turn-1",
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: "可以，先确认当前 BIOS 设置。",
          turn_id: "turn-1",
        },
        {
          id: "runtime-note:memory-signal:memory-signal-1",
          role: "system",
          type: "runtime_note",
          content: [],
          turn_id: "turn-1",
          runtime_note: {
            runtime_note_id: "runtime-note:memory-signal:memory-signal-1",
            kind: "memory_signal",
            created_at: "2026-08-03T00:00:00Z",
            payload: {
              signal_kind: "convention",
              confidence: "high",
              suggested_memory_kind: "preference",
              suggested_scope: "user",
              evidence: "memory-signal-1",
            },
          },
        },
      ],
    } as ChatHistory);

    expect(projection.items).toHaveLength(2);
    const assistant = assistantAt(projection, 1);
    expect(assistant.output.map((msg) => msg.id)).toEqual(["assistant-1"]);
    expect(JSON.stringify(projection.items)).not.toContain("memory-signal-1");
    expect(JSON.stringify(projection.items)).not.toContain("memory_search");
  });

  it("omits persisted memory signal marker messages from the visible projection", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "记住一下：以后回答直接给工程结论。",
          turn_id: "turn-1",
        },
        {
          id: "memory-signal-turn-1",
          role: "user",
          content:
            "<memory_signal>\nsignal_kind: explicit_memory\n</memory_signal>",
          metadata: {
            model_visible_marker: "memory_signal",
            hidden_from_ui: true,
          },
          turn_id: "turn-1",
        },
        {
          id: "assistant-1",
          role: "assistant",
          type: "message",
          content: "好的，我会先搜索已有 Memory。",
          turn_id: "turn-1",
        },
      ],
    });

    expect(projection.items).toHaveLength(2);
    expect(projection.items[0]).toMatchObject({
      kind: "user",
      id: "user-1",
    });
    const assistant = assistantAt(projection, 1);
    expect(assistant.output.map((msg) => msg.id)).toEqual(["assistant-1"]);
    expect(JSON.stringify(projection.items)).not.toContain("<memory_signal>");
  });

  it("keeps partial assistant output before a turn-aborted marker", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "执行一个 sleep 命令",
          turn_id: "turn-stopped",
        },
        {
          id: "assistant-tool",
          role: "assistant",
          type: "plugin_call",
          content: [
            {
              type: "data",
              data: { call_id: "call-1", name: "bash" },
            },
          ],
          turn_id: "turn-stopped",
        },
        {
          id: "turn-aborted-turn-stopped",
          role: "system",
          type: "message",
          content:
            "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>",
          metadata: {
            metadata: {
              model_visible_marker: "turn_aborted",
            },
          },
          turn_id: "turn-stopped",
        },
      ],
    });

    expect(projection.items).toHaveLength(2);
    const assistant = assistantAt(projection, 1);
    expect(assistant.status).toBe("interrupted");
    expect(assistant.output.map((message) => message.type)).toEqual([
      "plugin_call",
      "turn_aborted",
    ]);
    expect(assistantResponseText(assistant)).not.toContain("<turn_aborted>");
  });

  it("keeps partial assistant output before a turn-interjected marker", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      messages: [
        {
          id: "user-1",
          role: "user",
          content: "先跑一个长任务",
          turn_id: "turn-interjected",
        },
        {
          id: "assistant-partial",
          role: "assistant",
          type: "message",
          content: "partial answer",
          turn_id: "turn-interjected",
        },
        {
          id: "turn-interjected-marker",
          role: "system",
          type: "message",
          content:
            "<turn_interjected>\nThe user inserted a new message, so the previous turn was ended before it could continue.\n</turn_interjected>",
          metadata: {
            metadata: {
              model_visible_marker: "turn_interjected",
            },
          },
          turn_id: "turn-interjected",
        },
      ],
    });

    expect(projection.items).toHaveLength(2);
    const assistant = assistantAt(projection, 1);
    expect(assistant.status).toBe("interrupted");
    expect(assistant.output.map((message) => message.type)).toEqual([
      "message",
      "turn_interjected",
    ]);
    expect(assistantResponseText(assistant)).toContain("partial answer");
    expect(assistantResponseText(assistant)).not.toContain(
      "<turn_interjected>",
    );
  });

  it("attaches a late interjection marker to the turn it terminated", () => {
    const projection = projectConversation("chat-late-interjection", {
      status: "idle",
      messages: [
        {
          id: "user-old",
          role: "user",
          turn_id: "turn-old",
          content: "Old question",
        },
        {
          id: "assistant-old",
          role: "assistant",
          type: "message",
          turn_id: "turn-old",
          content: "Partial answer",
        },
        {
          id: "user-new",
          role: "user",
          turn_id: "turn-new",
          content: "Inserted question",
        },
        {
          id: "assistant-new",
          role: "assistant",
          type: "message",
          turn_id: "turn-new",
          content: "New answer",
        },
        {
          id: "turn-interjected-turn-old",
          role: "system",
          type: "message",
          turn_id: "turn-old",
          content:
            "<turn_interjected>\nThe user inserted a new message, so the previous turn was ended before it could continue.\n</turn_interjected>",
          metadata: {
            metadata: {
              model_visible_marker: "turn_interjected",
            },
          },
        },
      ],
    });

    expect(projection.items.map((item) => item.kind)).toEqual([
      "user",
      "assistant",
      "user",
      "assistant",
    ]);
    expect(assistantAt(projection, 1)).toMatchObject({
      turn_id: "turn-old",
      status: "interrupted",
      output: expect.arrayContaining([
        expect.objectContaining({ type: "turn_interjected" }),
      ]),
    });
    expect(assistantAt(projection, 3)).toMatchObject({
      turn_id: "turn-new",
      status: "completed",
    });
    expect(assistantResponseText(assistantAt(projection, 3))).toBe(
      "New answer",
    );
  });

  it("adds an owned generating assistant placeholder for a materialized active turn", () => {
    const projection = projectConversation("chat-1", {
      status: "running",
      active_turn: {
        phase: "streaming",
        turn_id: "turn-slow",
        user_message_id: "user-slow",
        authority_device_id: null,
        driver: "rua",
        started_at: "2026-07-16T00:00:00Z",
        queued_count: 0,
      },
      messages: [
        {
          id: "user-slow",
          role: "user",
          content: "Slow question",
          turn_id: "turn-slow",
        },
      ],
    });

    expect(projection.items).toHaveLength(2);
    expect(projection.items[1]).toMatchObject({
      kind: "assistant",
      status: "generating",
      output: [],
    });
  });

  it("keeps the active turn response generating until authority clears it", () => {
    const projection = projectConversation("chat-1", {
      status: "running",
      active_turn: {
        phase: "streaming",
        turn_id: "turn-active",
        user_message_id: "user-active",
        authority_device_id: null,
        driver: "rua",
        started_at: "2026-07-16T00:00:00Z",
        queued_count: 0,
      },
      messages: [
        {
          id: "user-active",
          role: "user",
          content: "Continue working",
          turn_id: "turn-active",
        },
        {
          id: "assistant-active",
          role: "assistant",
          content: "Partial output",
          turn_id: "turn-active",
        },
        {
          id: "note-active",
          role: "system",
          type: "runtime_note",
          content: [],
          turn_id: "turn-active",
          runtime_note: {
            runtime_note_id: "note-active",
            kind: "conversation_runtime_terminal",
            created_at: "2026-07-16T00:00:01Z",
            payload: {
              turn_id: "turn-active",
              terminal_status: "interrupted",
            },
          },
        },
      ],
    } as ChatHistory);

    expect(assistantAt(projection, 1)).toMatchObject({
      turn_id: "turn-active",
      status: "generating",
    });
  });

  it("uses the backend Active Turn as the running authority", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      active_turn: {
        phase: "streaming",
        turn_id: "turn-1",
        user_message_id: "user-1",
        authority_device_id: null,
        driver: "rua",
        started_at: "2026-07-16T00:00:00Z",
        queued_count: 0,
      },
      messages: [{ id: "user-1", role: "user", content: "Slow question" }],
    });

    expect(projection.turnActivity).toEqual({
      kind: "active",
      turnId: "turn-1",
      userMessageId: "user-1",
      responseId: "chat-1:assistant:user-1",
    });
    expect(projection.items).toHaveLength(2);
    expect(projection.items[1]).toMatchObject({
      kind: "assistant",
      status: "generating",
      output: [],
    });
  });

  it("uses ACP driver details from the backend Active Turn for active placeholders", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      active_turn: {
        phase: "streaming",
        turn_id: "turn-acp-1",
        user_message_id: "user-acp-1",
        authority_device_id: "desktop",
        driver: "acp",
        driver_detail: { runner: "codex" },
        started_at: "2026-07-16T00:00:00Z",
        queued_count: 0,
      },
      messages: [
        {
          id: "user-acp-1",
          role: "user",
          content: [{ type: "text", text: "Run in ACP" }],
        },
      ],
    });

    const assistant = assistantAt(projection, 1);
    expect(assistant.status).toBe("generating");
    expect(assistant.acp_origin_runner).toBe("codex");
  });

  it("uses the targeted remote Active Turn as the running authority", () => {
    const projection = projectConversation("chat-1", {
      status: "idle",
      active_turn: {
        phase: "streaming",
        turn_id: "turn-remote-1",
        user_message_id: "remote-user-1",
        authority_device_id: "desktop",
        driver: "rua",
        started_at: "2026-07-16T00:00:00Z",
        queued_count: 0,
      },
      messages: [
        {
          id: "remote-user-1",
          role: "user",
          content: [{ type: "text", text: "Run on desktop" }],
          turn_id: "turn-remote-1",
          metadata: {
            device_delivery_id: "device-delivery-1",
            source_device_id: "phone",
            target_device_id: "desktop",
          },
        },
      ],
    });

    expect(projection.items).toHaveLength(2);
    expect(projection.items[0]).toMatchObject({
      kind: "user",
      id: "remote-user-1",
    });
    expect(assistantAt(projection, 1).status).toBe("generating");
  });
});

describe("createConversationProjector", () => {
  it("matches a fresh projection after replacements, variant changes, control changes, and rollback", () => {
    const project = createConversationProjector();
    const user = {
      id: "user",
      role: "user",
      turn_id: "turn-1",
      content: "Question",
    };
    const response = {
      id: "answer",
      role: "assistant",
      turn_id: "turn-1",
      content: "Original",
      selected_variant_id: "v1",
      assistant_response_variants: [
        {
          variant_id: "v1",
          output: [
            { id: "v1-answer", role: "assistant", content: "First variant" },
          ],
        },
        {
          variant_id: "v2",
          output: [
            { id: "v2-answer", role: "assistant", content: "Second variant" },
          ],
        },
      ],
    };
    const history = { status: "idle" as const, messages: [user, response] };
    const first = project("chat-cached", history);
    const repeated = project("chat-cached", history);
    expect(repeated).toEqual(projectConversationImpl("chat-cached", history));
    expect(repeated.items[0]).toBe(first.items[0]);
    expect(repeated.items[1]).toBe(first.items[1]);

    const options = { selectedVariantOverrides: { "turn-1": "v2" } };
    const selected = project("chat-cached", history, options);
    expect(selected).toEqual(
      projectConversationImpl("chat-cached", history, options),
    );
    expect(selected.items[0]).toBe(first.items[0]);
    expect(selected.items[1]).not.toBe(first.items[1]);

    const corrected = {
      ...history,
      messages: [user, { ...response, finished_at: "2026-09-19T00:00:00Z" }],
    };
    expect(project("chat-cached", corrected, options)).toEqual(
      projectConversationImpl("chat-cached", corrected, options),
    );
    const running = {
      ...corrected,
      active_turn: {
        turn_id: "turn-1",
        user_message_id: "user",
        phase: "streaming",
        driver: "rua",
        authority_device_id: "desktop",
        queued_count: 0,
        started_at: "2026-09-19T00:00:00Z",
      },
    };
    expect(project("chat-cached", running, options)).toEqual(
      projectConversationImpl("chat-cached", running, options),
    );
    expect(project("chat-cached", { messages: [] })).toEqual(
      projectConversationImpl("chat-cached", { messages: [] }),
    );
    const restored = project("chat-cached", history);
    expect(restored).toEqual(first);
    expect(restored.items[0]).not.toBe(first.items[0]);
    const switched = project("other-chat", history);
    expect(switched).toEqual(projectConversationImpl("other-chat", history));
    expect(switched.items[0]).not.toBe(restored.items[0]);
  });

  it("invalidates inferred user Turn identity and never mutates earlier projections", () => {
    const project = createConversationProjector();
    const user = { id: "user", role: "user", content: "Question" };
    const first = project("chat", { messages: [user] });
    const next = project("chat", {
      messages: [
        user,
        {
          id: "answer",
          role: "assistant",
          content: "Answer",
          turn_id: "assigned-turn",
        },
      ],
    });
    expect(next.items[0]).not.toBe(first.items[0]);
    expect(next.items[0].turn_id).toBe("assigned-turn");
    expect(first.items[0].turn_id).toBeUndefined();
  });
});
