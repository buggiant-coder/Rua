import { describe, expect, it } from "vitest";
import type {
  ChatHistory,
  ConversationControlView,
  Message,
} from "@/api/types";
import {
  ConversationConvergence,
  activeRunCoveredByHistory,
} from "../conversationConvergence";
import {
  initialConversationStreamFold,
  type ConversationStreamEvent,
} from "../stream";

const target = { scope: "agent" as const, agentId: "agent-a" };
const user: Message = {
  id: "user-1",
  role: "user",
  content: "question",
  turn_id: "turn-1",
};
function raw(sequence: number) {
  return {
    activity_generation: "generation-1",
    activity_sequence: sequence,
    address: {
      scope: { kind: "agent", agent_id: "agent-a" },
      chat_id: "chat-1",
    },
  };
}
const frames: ConversationStreamEvent[] = [
  { kind: "user_message", message: user, raw: raw(1) },
  { kind: "text_delta", messageId: "assistant-1", delta: "Hello", raw: raw(2) },
  {
    kind: "text_delta",
    messageId: "assistant-1",
    delta: " world",
    raw: raw(3),
  },
  {
    kind: "turn_terminal",
    turnId: "turn-1",
    status: "completed",
    deviceDeliveryId: "delivery-1",
    raw: raw(4),
  },
  {
    kind: "history_watermark_reached",
    durableSequence: 2,
    manifestHash: "final",
    authorityDeviceId: "desktop",
    raw: raw(5),
  },
];
function replay(
  events: ConversationStreamEvent[],
  complete = true,
): ConversationStreamEvent {
  return { kind: "stream_replay", events, complete, raw: {} };
}
function context(currentDeviceId = "desktop") {
  return {
    fold: initialConversationStreamFold,
    user: null as Message | null,
    hasOverlay: true,
    currentDeviceId,
  };
}
function history(sequence: number, messages: Message[] = [user]): ChatHistory {
  return {
    messages,
    durable_coverage: {
      latest_message_sequence: sequence,
      manifest_hash: `history-${sequence}`,
    },
  };
}
function control(
  revision: number,
  active: boolean,
  epoch = "epoch-1",
): ConversationControlView {
  return {
    revision,
    placement: {
      authority_device_id: "desktop",
      authority_epoch: epoch,
      switchable: !active,
    },
    active_turn: active
      ? {
          turn_id: "turn-1",
          authority_device_id: "desktop",
          authority_epoch: epoch,
          driver: "rua",
          phase: "committing",
          started_at: "2026-11-04T00:00:00Z",
        }
      : null,
    queue: { entries: [], paused: false },
    acp: { active: false },
    allowed_controls: {
      stop: active,
      queue: active,
      interject: active,
      edit_queued_input: false,
      delete_queued_input: false,
      switch_authority: !active,
      enter_acp: !active,
      exit_acp: false,
    },
  };
}

describe("ConversationConvergence", () => {
  it.each(["desktop", "phone"])(
    "converges replay and live to identical content and reconciliation intents on %s",
    (device) => {
      const view = new ConversationConvergence("agent-a:chat-1");
      const live = view.openActivity(target, "chat-1", null);
      const current = context(device);
      const deliveries: string[] = [];
      const hydration = new Set<string>();
      let refresh = false;
      for (const frame of frames) {
        const update = live.consume(frame, current)!;
        if (update.fold) current.fold = update.fold;
        if (frame.kind === "user_message") current.user = frame.message;
        deliveries.push(...update.completedDeliveries);
        update.hydrationDevices.forEach((id) => hydration.add(id));
        refresh ||= update.refreshHistory;
      }
      const catchup = view.openActivity(target, "chat-1", null);
      const recovered = catchup.consume(replay(frames), context(device))!;
      expect(recovered.replay?.fold).toEqual(current.fold);
      expect(recovered.replay?.user).toEqual(user);
      expect(recovered.completedDeliveries).toEqual(deliveries);
      expect(recovered.refreshHistory).toBe(refresh);
      expect(recovered.hydrationDevices).toEqual(hydration);
      expect([...hydration]).toEqual(device === "desktop" ? [] : ["desktop"]);
      expect(catchup.cursor).toEqual(live.cursor);
    },
  );

  it("does not commit an incomplete replay cursor, so reconnect can recover the full batch", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const first = view.openActivity(target, "chat-1", null);
    expect(
      first.consume(replay(frames.slice(0, 3), false), context()),
    ).toBeNull();
    expect(first.cursor).toBeNull();
    const second = view.openActivity(target, "chat-1", first.cursor);
    const recovered = second.consume(replay(frames), context())!;
    expect(JSON.stringify(recovered.replay?.fold.output)).toContain(
      "Hello world",
    );
    expect(second.cursor?.sequence).toBe(5);
    expect(second.consume(frames[2], context())?.event).toBeUndefined();
  });

  it("stages a checkpoint across chunks without retaining the old tail or skipping earlier positions", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const reader = view.openActivity(target, "chat-1", {
      generation: "generation-1",
      sequence: 99,
    });
    const checkpoint = frames.slice(0, 3).map((frame) => ({
      ...frame,
      raw: { ...(frame as { raw: object }).raw, activity_checkpoint: true },
    }));
    expect(
      reader.consume(replay(checkpoint.slice(0, 2), false), context()),
    ).toBeNull();
    expect(reader.cursor?.sequence).toBe(99);
    const result = reader.consume(replay(checkpoint.slice(2)), context())!;
    expect(JSON.stringify(result.replay?.fold.output)).toContain("Hello world");
    expect(reader.cursor?.sequence).toBe(3);
  });

  it("rejects foreign scoped activity before advancing the cursor or requesting hydration", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const reader = view.openActivity(target, "chat-1", null);
    const event = {
      ...frames[4],
      raw: {
        ...raw(100),
        address: {
          scope: { kind: "project", project_id: "project-a" },
          chat_id: "chat-1",
        },
      },
    } as ConversationStreamEvent;
    expect(reader.consume(event, context("phone"))?.refreshHistory).toBe(false);
    expect(reader.cursor).toBeNull();
    expect(reader.consume(frames[4], context("phone"))?.refreshHistory).toBe(
      true,
    );
    const duplicate = { ...frames[4], raw: raw(6) } as ConversationStreamEvent;
    expect(reader.consume(duplicate, context("phone"))?.refreshHistory).toBe(
      false,
    );
  });

  it("rejects older history before it can release a live overlay", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const older = view.beginHistoryRead();
    const newer = view.beginHistoryRead();
    expect(view.acceptHistory(newer, history(4))).toBe(true);
    expect(view.acceptHistory(older, history(2))).toBe(false);
    expect(
      view.acceptHistory(newer, {
        ...history(4),
        durable_coverage: {
          latest_message_sequence: 4,
          manifest_hash: "different",
        },
      }),
    ).toBe(false);
  });

  it("invalidates pre-rollback reads while allowing the shorter rolled-back history", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const old = view.beginHistoryRead();
    expect(view.acceptHistory(old, history(4))).toBe(true);
    view.invalidateHistory();
    expect(view.acceptHistory(old, history(5))).toBe(false);
    expect(view.acceptHistory(view.beginHistoryRead(), history(1))).toBe(true);
    const other = new ConversationConvergence("project-a:chat-1");
    expect(other.acceptHistory(view.beginHistoryRead(), history(1))).toBe(
      false,
    );
  });

  it("keeps uncovered live content, then replaces it in place when matching history arrives", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const recovered = view
      .openActivity(target, "chat-1", null)
      .consume(replay(frames), context())!.replay!;
    const partial = history(1);
    expect(view.acceptHistory(view.beginHistoryRead(), partial)).toBe(true);
    expect(activeRunCoveredByHistory(partial, user, recovered.fold)).toBe(
      false,
    );
    const live = view.project({
      serverHistory: partial,
      pendingDeliveries: [],
      activeRun: { user, fold: recovered.fold },
    });
    expect(JSON.stringify(live.history)).toContain("Hello world");
    const complete = history(2, [
      user,
      {
        id: "assistant-1",
        turn_id: "turn-1",
        role: "assistant",
        status: "completed",
        content: [{ type: "text", text: "Hello world" }],
      },
    ]);
    expect(view.acceptHistory(view.beginHistoryRead(), complete)).toBe(true);
    expect(activeRunCoveredByHistory(complete, user, recovered.fold)).toBe(
      true,
    );
    const stable = view.project({
      serverHistory: complete,
      pendingDeliveries: [],
      activeRun: { user: null, fold: null },
    });
    expect(
      stable.history?.messages.filter(
        (message) => message.id === "assistant-1",
      ),
    ).toHaveLength(1);
    expect(JSON.stringify(stable.history)).toContain("Hello world");
  });

  it("does not replace a newer idle control with an older running control", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    expect(view.acceptControl(control(8, false))).toBe(true);
    expect(view.acceptControl(control(7, true))).toBe(false);
    expect(view.acceptControl(control(9, true))).toBe(true);
    expect(view.acceptControl(control(1, true, "epoch-2"))).toBe(true);
  });
  it("uses restored durable coverage before accepting a history response", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const ticket = view.beginHistoryRead();
    const input = {
      pendingDeliveries: [],
      activeRun: { user: null, fold: null },
    };
    view.project({ ...input, serverHistory: history(3) });
    expect(view.acceptHistory(ticket, history(2))).toBe(false);
    expect(view.acceptHistory(ticket, history(5))).toBe(true);
    // React may still render the previous snapshot before applying the new one.
    view.project({ ...input, serverHistory: history(3) });
    expect(view.acceptHistory(ticket, history(4))).toBe(false);
  });

  it("preserves rollback/control ordering within a replay commit", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const transitions: ConversationStreamEvent[] = [
      { kind: "conversation_control", control: control(1, true), raw: raw(1) },
      {
        kind: "history_mutated",
        action: "rolled_back",
        beforeTurnId: "turn-1",
        raw: raw(2),
      },
      { kind: "conversation_control", control: control(2, false), raw: raw(3) },
    ];
    const reader = view.openActivity(target, "chat-1", null);
    expect(
      reader.consume(replay(transitions.slice(0, 2), false), context()),
    ).toBeNull();
    const result = reader.consume(replay(transitions.slice(2)), context())!;
    expect(result.transitions).toEqual(transitions);
    expect(result.refreshHistory).toBe(true);
    expect(result.replay?.fold.output).toEqual([]);
  });
  it("does not resurrect compaction activity from before a replayed rollback", () => {
    const view = new ConversationConvergence("agent-a:chat-1");
    const result = view.openActivity(target, "chat-1", null).consume(
      replay([
        { kind: "context_compaction", status: "started", raw: raw(1) },
        {
          kind: "history_mutated",
          action: "rolled_back",
          beforeTurnId: "turn-1",
          raw: raw(2),
        },
        {
          kind: "text_delta",
          messageId: "assistant-new",
          delta: "new",
          raw: raw(3),
        },
      ]),
      context(),
    )!;
    expect(result.replay?.compaction).toBeNull();
  });
});
