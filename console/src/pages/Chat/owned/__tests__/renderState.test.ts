import { describe, expect, it, vi } from "vitest";
import type { Message } from "../../../../api/types/chat";
import { projectConversation } from "../projection";
import {
  initialConversationStreamFold,
  foldConversationStreamEvent,
} from "../stream";
import { buildConversationRenderHistory } from "../renderState";

describe("buildConversationRenderHistory", () => {
  it("reconciles only the matching active Turn after an authority terminal event", () => {
    const fold = {
      ...initialConversationStreamFold,
      status: "failed" as const,
      sawTerminal: true,
      terminalTurnId: "turn-a",
    };
    for (const active of ["turn-a", "turn-b"]) {
      const history = buildConversationRenderHistory({
        serverHistory: {
          status: "running",
          messages: [],
          active_turn: { turn_id: active } as never,
          control: {
            active_turn: { turn_id: active },
            queue: { entries: [] },
          } as never,
        },
        pendingDeliveries: [],
        activeRun: { user: null, fold },
      });
      expect(history?.active_turn?.turn_id ?? null).toBe(
        active === "turn-a" ? null : "turn-b",
      );
      expect(history?.status).toBe(active === "turn-a" ? "idle" : "running");
    }
  });
  it("pins replay thinking to its original turn when authority advances", () => {
    const fold = foldConversationStreamEvent(
      { ...initialConversationStreamFold, turnId: "turn-a" },
      {
        kind: "thinking_delta",
        messageId: "thinking-a",
        delta: "A",
        raw: {},
      },
    );
    const history = buildConversationRenderHistory({
      serverHistory: {
        active_turn: { turn_id: "turn-b" } as never,
        messages: [
          { id: "a", role: "user", turn_id: "turn-a", content: "A" },
          { id: "b", role: "user", turn_id: "turn-b", content: "B" },
        ],
      },
      pendingDeliveries: [],
      activeRun: { user: null, fold },
    });
    expect(
      history?.messages.map((message) => [message.id, message.turn_id]),
    ).toEqual([
      ["a", "turn-a"],
      ["thinking-a", "turn-a"],
      ["b", "turn-b"],
    ]);
  });

  it("keeps stopped thinking in its turn until that message is durable", () => {
    const user = {
      id: "user-a",
      role: "user",
      turn_id: "turn-a",
      content: "A",
    };
    const nextUser = {
      id: "user-b",
      role: "user",
      turn_id: "turn-b",
      content: "B",
    };
    const stopped = {
      id: "stop-a",
      role: "assistant",
      turn_id: "turn-a",
      type: "turn_aborted",
      content: [],
    };
    const thinking = {
      id: "thinking-a",
      role: "assistant",
      type: "reasoning",
      content: [{ type: "text" as const, text: "Old thinking" }],
    };
    const activeRun = {
      user,
      fold: {
        ...initialConversationStreamFold,
        sawTerminal: true,
        terminalTurnId: "turn-a",
        status: "interrupted" as const,
        output: [thinking],
      },
    };
    const history = buildConversationRenderHistory({
      serverHistory: { messages: [user, stopped, nextUser] },
      pendingDeliveries: [],
      activeRun,
    });
    expect(history?.messages.map((message) => message.id)).toEqual([
      "user-a",
      "stop-a",
      "thinking-a",
      "user-b",
    ]);
    const response = projectConversation("chat", history).items.find(
      (item) => item.kind === "assistant",
    );
    expect(response).toMatchObject({ turn_id: "turn-a" });
    expect(
      response?.kind === "assistant" &&
        response.output.some((message) => message.id === "thinking-a"),
    ).toBe(true);
    const reconciled = buildConversationRenderHistory({
      serverHistory: {
        messages: [user, { ...thinking, turn_id: "turn-a" }, stopped, nextUser],
      },
      pendingDeliveries: [],
      activeRun,
    });
    expect(
      reconciled?.messages.filter((message) => message.id === "thinking-a"),
    ).toHaveLength(1);
  });

  it("reuses unchanged history identities while live text changes, including overlap", () => {
    const reads = vi.fn();
    const messages = Array.from({ length: 200 }, (_, index) => ({
      get id() {
        reads();
        return `old-${index}`;
      },
      role: index % 2 ? "assistant" : "user",
      turn_id: `turn-${Math.floor(index / 2)}`,
      content: `Message ${index}`,
    }));
    const serverHistory = { messages };
    const build = (text: string) =>
      buildConversationRenderHistory({
        serverHistory,
        pendingDeliveries: [],
        activeRun: {
          user: null,
          fold: {
            ...initialConversationStreamFold,
            output: [
              {
                id: "old-199",
                role: "assistant",
                turn_id: "turn-99",
                type: "message",
                content: [{ type: "text", text }],
              },
            ],
          },
        },
      });
    build("first");
    reads.mockClear();
    const updated = build("first second");
    expect(reads.mock.calls.length).toBe(0);
    expect(updated?.messages).toHaveLength(200);
    expect(updated?.messages[199].content).toEqual([
      { type: "text", text: "first second" },
    ]);
    expect(serverHistory.messages[199].content).toBe("Message 199");
  });

  it("invalidates cached identities for a replaced history snapshot", () => {
    const base = {
      messages: [
        { id: "answer", role: "assistant", turn_id: "turn", content: "old" },
      ],
    };
    const activeRun = {
      user: null,
      fold: {
        ...initialConversationStreamFold,
        output: [
          {
            id: "answer",
            role: "assistant",
            turn_id: "turn",
            content: [{ type: "text", text: "live" }],
          },
        ],
      },
    };
    buildConversationRenderHistory({
      serverHistory: base,
      pendingDeliveries: [],
      activeRun,
    });
    const replaced = buildConversationRenderHistory({
      serverHistory: {
        messages: [
          {
            id: "replacement",
            role: "assistant",
            turn_id: "replacement-turn",
            content: "new",
          },
        ],
      },
      pendingDeliveries: [],
      activeRun,
    });
    expect(replaced?.messages.map((message) => message.id)).toEqual([
      "replacement",
      "answer",
    ]);
    expect(base.messages[0].content).toBe("old");
  });

  it("rebuilds Turn coverage when live content corrects a stored message identity", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        messages: [
          { id: "message", role: "user", turn_id: "old-turn", content: "old" },
        ],
      },
      pendingDeliveries: [],
      activeRun: {
        user: {
          id: "new-user",
          role: "user",
          turn_id: "old-turn",
          content: "Question",
        },
        fold: {
          ...initialConversationStreamFold,
          output: [
            {
              id: "message",
              role: "assistant",
              turn_id: "new-turn",
              content: [{ type: "text", text: "corrected" }],
            },
          ],
        },
      },
    });
    expect(history?.messages.map((message) => message.id)).toEqual([
      "message",
      "new-user",
    ]);
    expect(history?.messages[0]).toMatchObject({
      role: "assistant",
      turn_id: "new-turn",
    });
  });

  it("does not append a completed live run already covered by canonical history", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "idle",
        messages: [
          {
            id: "msg-user-stable",
            role: "user",
            turn_id: "turn-1",
            content: [{ type: "text", text: "Question" }],
          },
          {
            id: "msg-assistant-stable",
            role: "assistant",
            turn_id: "turn-1",
            type: "message",
            content: [{ type: "text", text: "Answer" }],
          },
        ],
      },
      pendingDeliveries: [],
      activeRun: {
        user: {
          id: "user-optimistic",
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "Question" }],
        },
        fold: {
          ...initialConversationStreamFold,
          status: "completed",
          sawTerminal: true,
          output: [
            {
              id: "msg-assistant-stable",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "Answer" }],
            },
          ],
        },
      },
    });

    expect(history?.messages.map((message) => message.id)).toEqual([
      "msg-user-stable",
      "msg-assistant-stable",
    ]);
  });

  it("uses the terminal turn id when replay omitted the live user boundary", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "idle",
        messages: [
          {
            id: "msg-user-stable",
            role: "user",
            turn_id: "turn-1",
            content: [{ type: "text", text: "Question" }],
          },
          {
            id: "msg-assistant-stable",
            role: "assistant",
            turn_id: "turn-1",
            type: "message",
            content: [{ type: "text", text: "Answer" }],
          },
        ],
      },
      pendingDeliveries: [],
      activeRun: {
        user: null,
        fold: {
          ...initialConversationStreamFold,
          status: "completed",
          sawTerminal: true,
          terminalTurnId: "turn-1",
          output: [
            {
              id: "msg-assistant-stable",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "Answer" }],
            },
          ],
        },
      },
    });

    expect(history?.messages.map((message) => message.id)).toEqual([
      "msg-user-stable",
      "msg-assistant-stable",
    ]);
  });

  it("fills a canonical in-progress turn from live output without duplicating the user boundary", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "running",
        active_turn: {
          turn_id: "turn-1",
          user_message_id: "msg-user-stable",
          phase: "streaming",
          authority_device_id: "device-a",
          authority_epoch: "epoch-a",
          driver: "rua",
          started_at: "2026-01-01T00:00:00Z",
        },
        control: {
          revision: 1,
          placement: null,
          active_turn: {
            turn_id: "turn-1",
            user_message_id: "msg-user-stable",
            phase: "streaming",
            authority_device_id: "device-a",
            authority_epoch: "epoch-a",
            driver: "rua",
            started_at: "2026-01-01T00:00:00Z",
          },
          input_queue: [],
        },
        messages: [
          {
            id: "msg-user-stable",
            role: "user",
            turn_id: "turn-1",
            content: [{ type: "text", text: "Question" }],
          },
        ],
      },
      pendingDeliveries: [],
      activeRun: {
        user: {
          id: "user-optimistic",
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "Question" }],
        },
        fold: {
          ...initialConversationStreamFold,
          output: [
            {
              id: "msg-assistant-live",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "Streaming" }],
            },
          ],
        },
      },
    });

    expect(history?.messages.map((message) => message.id)).toEqual([
      "msg-user-stable",
      "msg-assistant-live",
    ]);
  });

  it("prefers the live version of the same message while its Turn is running", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "running",
        active_turn: { turn_id: "turn-1" },
        messages: [
          {
            id: "msg-user-stable",
            role: "user",
            turn_id: "turn-1",
            content: [{ type: "text", text: "Question" }],
          },
          {
            id: "msg-assistant-1",
            role: "assistant",
            turn_id: "turn-1",
            type: "message",
            content: [{ type: "text", text: "Partial" }],
          },
        ],
      },
      pendingDeliveries: [],
      activeRun: {
        user: null,
        fold: {
          ...initialConversationStreamFold,
          output: [
            {
              id: "msg-assistant-1",
              role: "assistant",
              turn_id: "turn-1",
              type: "message",
              content: [{ type: "text", text: "Partial continued" }],
            },
          ],
        },
      },
    });

    expect(history?.messages).toHaveLength(2);
    expect(history?.messages[1]).toMatchObject({
      id: "msg-assistant-1",
      content: [{ type: "text", text: "Partial continued" }],
    });
  });

  it("appends a new live message in the same running Turn", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "running",
        active_turn: { turn_id: "turn-1" },
        messages: [
          {
            id: "msg-user-stable",
            role: "user",
            turn_id: "turn-1",
            content: [{ type: "text", text: "Question" }],
          },
          {
            id: "msg-assistant-1",
            role: "assistant",
            turn_id: "turn-1",
            type: "message",
            content: [{ type: "text", text: "First" }],
          },
        ],
      },
      pendingDeliveries: [],
      activeRun: {
        user: null,
        fold: {
          ...initialConversationStreamFold,
          output: [
            {
              id: "msg-assistant-2",
              role: "assistant",
              turn_id: "turn-1",
              type: "message",
              content: [{ type: "text", text: "Second" }],
            },
          ],
        },
      },
    });

    expect(history?.messages.map((message) => message.id)).toEqual([
      "msg-user-stable",
      "msg-assistant-1",
      "msg-assistant-2",
    ]);
  });

  it("keeps authority state running when the live content reaches a terminal event", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "running",
        active_turn: {
          turn_id: "turn-1",
          user_message_id: "msg-user-stable",
          phase: "streaming",
          authority_device_id: "device-a",
          authority_epoch: "epoch-a",
          driver: "rua",
          started_at: "2026-01-01T00:00:00Z",
        },
        control: {
          revision: 1,
          placement: null,
          active_turn: {
            turn_id: "turn-1",
            user_message_id: "msg-user-stable",
            phase: "streaming",
            authority_device_id: "device-a",
            authority_epoch: "epoch-a",
            driver: "rua",
            started_at: "2026-01-01T00:00:00Z",
          },
          input_queue: [],
        },
        messages: [
          {
            id: "msg-user-stable",
            role: "user",
            turn_id: "turn-1",
            content: [{ type: "text", text: "Question" }],
          },
        ],
      },
      pendingDeliveries: [],
      activeRun: {
        user: {
          id: "msg-user-stable",
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "Question" }],
        },
        fold: {
          ...initialConversationStreamFold,
          status: "completed",
          sawTerminal: true,
          output: [
            {
              id: "msg-assistant-live",
              role: "assistant",
              turn_id: "turn-1",
              type: "message",
              content: [{ type: "text", text: "Answer" }],
            },
          ],
        },
      },
    });

    const projection = projectConversation("chat-1", history || undefined);
    expect(history?.status).toBe("running");
    expect(projection.turnActivity.kind).toBe("active");
    expect(projection.items[1]).toMatchObject({
      kind: "assistant",
      status: "generating",
    });
  });

  it("projects pending delivery as one pending user and clears delivering state", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "running",
        messages: [],
      },
      pendingDeliveries: [
        {
          deviceDeliveryId: "device-delivery-1",
          targetDeviceId: "device-remote",
          deviceDeliverySequence: 1,
          acceptedAt: "2026-07-05T00:00:00Z",
          message: {
            id: "local-user-1",
            role: "user",
            content: [{ type: "text", text: "Remote prompt" }],
            remote_delivery_status: "delivering",
          },
        },
      ],
      activeRun: {
        user: null,
        fold: null,
      },
    });

    expect(history?.messages).toHaveLength(1);
    expect(history?.messages[0]).toMatchObject({
      id: "local-user-1",
      role: "user",
      remote_delivery_state: "pending",
      metadata: {
        device_delivery_id: "device-delivery-1",
        target_device_id: "device-remote",
      },
    });
    expect(history?.messages[0]).not.toHaveProperty("remote_delivery_status");
    expect(history?.messages[0]).not.toHaveProperty("remote_pending");
  });

  it("stamps ACP origin on pending remote deliveries while ACP mode is active", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "running",
        messages: [],
      },
      pendingDeliveries: [
        {
          deviceDeliveryId: "device-delivery-1",
          targetDeviceId: "device-remote",
          deviceDeliverySequence: 1,
          message: {
            id: "local-user-1",
            role: "user",
            content: [{ type: "text", text: "Remote ACP prompt" }],
          },
        },
      ],
      activeRun: {
        user: null,
        fold: null,
        acpOriginRunner: "codex",
      },
    });

    const projection = projectConversation("chat-1", history || undefined);

    expect(projection.items[0]).toMatchObject({
      kind: "user",
      acp_origin_runner: "codex",
      remote_delivery_state: "pending",
    });
  });

  it("preserves sending delivery state on active user boundaries", () => {
    const history = buildConversationRenderHistory({
      serverHistory: null,
      pendingDeliveries: [],
      activeRun: {
        user: {
          id: "local-user-1",
          role: "user",
          content: [{ type: "text", text: "Remote prompt" }],
          remote_delivery_state: "sending",
        },
        fold: initialConversationStreamFold,
      },
    });

    const projection = projectConversation("chat-1", history || undefined);

    expect(projection.items[0]).toMatchObject({
      kind: "user",
      remote_delivery_state: "sending",
    });
  });

  it("lets a live device-delivery user boundary claim the pending delivery", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "running",
        messages: [],
      },
      pendingDeliveries: [
        {
          deviceDeliveryId: "device-delivery-1",
          targetDeviceId: "device-remote",
          deviceDeliverySequence: 1,
          message: {
            id: "local-user-1",
            role: "user",
            content: [{ type: "text", text: "Remote prompt" }],
            remote_delivery_status: "delivering",
          },
        },
      ],
      activeRun: {
        user: {
          id: "target-user-1",
          role: "user",
          content: [{ type: "text", text: "Remote prompt" }],
          metadata: {
            device_delivery_id: "device-delivery-1",
          },
        },
        fold: initialConversationStreamFold,
      },
    });

    expect(
      history?.messages.filter((message) => message.role === "user"),
    ).toHaveLength(1);
    expect(history?.messages.map((message) => message.id)).toEqual([
      "target-user-1",
    ]);
  });

  it("lets stable history cover pending delivery by device delivery id", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "running",
        messages: [
          {
            id: "stable-user-1",
            role: "user",
            content: [{ type: "text", text: "Remote prompt" }],
            metadata: {
              device_delivery_id: "device-delivery-1",
            },
          },
        ],
      },
      pendingDeliveries: [
        {
          deviceDeliveryId: "device-delivery-1",
          targetDeviceId: "device-remote",
          deviceDeliverySequence: 1,
          message: {
            id: "local-user-1",
            role: "user",
            content: [{ type: "text", text: "Remote prompt" }],
          },
        },
      ],
      activeRun: {
        user: null,
        fold: null,
      },
    });

    expect(history?.messages.map((message) => message.id)).toEqual([
      "stable-user-1",
    ]);
  });

  it.each([
    {
      identity: "submission id",
      delivery: { submissionId: "submission-1" },
      stable: { metadata: { submission_id: "submission-1" } },
    },
    {
      identity: "input id",
      delivery: { inputId: "input-1" },
      stable: { metadata: { input_id: "input-1" } },
    },
    {
      identity: "message id",
      delivery: {},
      stable: { id: "local-user-1" },
    },
    {
      identity: "Turn id",
      delivery: { turnId: "turn-1" },
      stable: { turn_id: "turn-1" },
    },
  ])(
    "lets stable history cover pending delivery by $identity",
    ({ delivery, stable }) => {
      const history = buildConversationRenderHistory({
        serverHistory: {
          status: "running",
          messages: [
            {
              id: "stable-user-1",
              role: "user",
              content: [{ type: "text", text: "Remote prompt" }],
              ...stable,
            },
          ],
        },
        pendingDeliveries: [
          {
            deviceDeliveryId: "device-delivery-pending",
            targetDeviceId: "device-remote",
            deviceDeliverySequence: 1,
            message: {
              id: "local-user-1",
              role: "user",
              content: [{ type: "text", text: "Remote prompt" }],
            },
            ...delivery,
          },
        ],
        activeRun: {
          user: null,
          fold: null,
        },
      });

      expect(history?.messages).toHaveLength(1);
      expect(history?.messages[0]).toMatchObject({ role: "user" });
      expect(history?.messages[0]).not.toHaveProperty("remote_delivery_state");
    },
  );

  it("keeps separate canonical turns even when user and assistant text repeats", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "idle",
        messages: [
          {
            id: "msg-user-1",
            role: "user",
            turn_id: "turn-1",
            content: [{ type: "text", text: "again" }],
          },
          {
            id: "msg-assistant-1",
            role: "assistant",
            turn_id: "turn-1",
            type: "message",
            content: [{ type: "text", text: "same answer" }],
          },
        ],
      },
      pendingDeliveries: [],
      activeRun: {
        user: {
          id: "msg-user-2",
          role: "user",
          turn_id: "turn-2",
          content: [{ type: "text", text: "again" }],
        },
        fold: {
          ...initialConversationStreamFold,
          output: [
            {
              id: "msg-assistant-2-live",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "same answer" }],
            },
          ],
        },
      },
    });

    expect(history?.messages.map((message) => message.id)).toEqual([
      "msg-user-1",
      "msg-assistant-1",
      "msg-user-2",
      "msg-assistant-2-live",
    ]);
  });

  it("stamps ACP origin on live user and assistant messages", () => {
    const history = buildConversationRenderHistory({
      serverHistory: null,
      pendingDeliveries: [],
      activeRun: {
        acpOriginRunner: "codex",
        user: {
          id: "user-live",
          role: "user",
          content: [{ type: "text", text: "Run command" }],
        },
        fold: {
          ...initialConversationStreamFold,
          output: [
            {
              id: "assistant-live",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "Running" }],
            },
          ],
        },
      },
    });

    const projection = projectConversation("chat-1", history || undefined);

    expect(projection.items[0]).toMatchObject({
      kind: "user",
      acp_origin_runner: "codex",
    });
    expect(projection.items[1]).toMatchObject({
      kind: "assistant",
      output: [
        expect.objectContaining({
          metadata: { origin: { kind: "acp", runner: "codex" } },
        }),
      ],
    });
  });

  it("ignores null entries in persisted history", () => {
    const history = buildConversationRenderHistory({
      serverHistory: {
        status: "idle",
        messages: [
          null,
          {
            id: "valid-message",
            role: "assistant",
            content: [{ type: "text", text: "still renders" }],
          },
        ] as unknown as Message[],
      },
      pendingDeliveries: [],
      activeRun: { user: null, fold: null },
    });

    expect(history?.messages.map((message) => message.id)).toEqual([
      "valid-message",
    ]);
  });

  it("ignores null entries in live output", () => {
    const history = buildConversationRenderHistory({
      serverHistory: null,
      pendingDeliveries: [],
      activeRun: {
        user: null,
        fold: {
          ...initialConversationStreamFold,
          output: [
            null,
            {
              id: "valid-live-message",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "still renders" }],
            },
          ] as unknown as ConversationStreamFold["output"],
        },
      },
    });

    expect(history?.messages.map((message) => message.id)).toEqual([
      "valid-live-message",
    ]);
  });

  it("uses canonical user ACP origin before the local active runner hint", () => {
    const history = buildConversationRenderHistory({
      serverHistory: null,
      pendingDeliveries: [],
      activeRun: {
        acpOriginRunner: "local-stale-runner",
        user: {
          id: "msg-user-live",
          role: "user",
          turn_id: "turn-1",
          content: [{ type: "text", text: "Run command" }],
          metadata: {
            origin: { kind: "acp", runner: "codex" },
          },
        },
        fold: {
          ...initialConversationStreamFold,
          output: [
            {
              id: "assistant-live",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "Running" }],
            },
          ],
        },
      },
    });

    const projection = projectConversation("chat-1", history || undefined);

    expect(projection.items[0]).toMatchObject({
      kind: "user",
      acp_origin_runner: "codex",
    });
    expect(projection.items[1]).toMatchObject({
      kind: "assistant",
      acp_origin_runner: "codex",
      output: [
        expect.objectContaining({
          metadata: { origin: { kind: "acp", runner: "codex" } },
        }),
      ],
    });
  });
});
