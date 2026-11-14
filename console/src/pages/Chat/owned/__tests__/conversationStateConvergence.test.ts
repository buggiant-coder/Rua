import { describe, expect, it } from "vitest";
import { projectConversation } from "../projection";
import type {
  ChatHistory,
  ConversationControlView,
  Message,
} from "@/api/types";
import {
  deriveTurnActivity,
  reconcileStableConversationHistory,
} from "../conversationStateConvergence";

const authorityEpoch = "0000019fb6ed2d0100000000df92900d9e76431f";

function control(
  revision: number,
  activeTurnId: string | null,
  epoch = authorityEpoch,
): ConversationControlView {
  return {
    revision,
    placement: {
      authority_device_id: "device-authority",
      authority_epoch: epoch,
      switchable: activeTurnId === null,
    },
    active_turn: activeTurnId
      ? {
          turn_id: activeTurnId,
          phase: "committing",
          authority_device_id: "device-authority",
          authority_epoch: epoch,
          driver: "rua",
          started_at: "2026-07-31T00:00:00Z",
        }
      : null,
    queue: { entries: [], paused: false },
    acp: { active: false },
    allowed_controls: {
      stop: activeTurnId !== null,
      queue: activeTurnId !== null,
      interject: activeTurnId !== null,
      edit_queued_input: false,
      delete_queued_input: false,
      switch_authority: activeTurnId === null,
      enter_acp: activeTurnId === null,
      exit_acp: false,
    },
  };
}

function completedTurnMessages(turnId: string): Message[] {
  return [
    {
      id: `user-${turnId}`,
      turn_id: turnId,
      role: "user",
      content: [{ type: "text", text: "user" }],
    },
    {
      id: `assistant-${turnId}`,
      turn_id: turnId,
      role: "assistant",
      type: "message",
      content: [{ type: "text", text: "assistant" }],
    },
  ];
}

function turnMessages(turnId: string, turnStatus: string): Message[] {
  return [
    {
      id: `user-${turnId}`,
      turn_id: turnId,
      turn_status: turnStatus,
      role: "user",
      content: [{ type: "text", text: "user" }],
    },
    {
      id: `assistant-tooluse-${turnId}`,
      turn_id: turnId,
      turn_status: turnStatus,
      role: "assistant",

      status: "completed",
      content: [{ type: "tool_use", id: "call-1", name: "read", input: {} }],
    },
  ];
}

describe("conversationStateConvergence", () => {
  it("starts presentation only after the authority turn has a formal user message", () => {
    const turnId = "turn-current";
    const started: ChatHistory = {
      status: "running",
      active_turn: control(9, turnId).active_turn,
      control: control(9, turnId),
      messages: [],
    };

    expect(deriveTurnActivity(started)).toMatchObject({
      kind: "active",
    });

    const activity = deriveTurnActivity({
      ...started,
      messages: turnMessages(turnId, "in_progress"),
    });
    expect(activity).toEqual({
      kind: "active",
      turnId,
      userMessageId: `user-${turnId}`,
      responseId: null,
    });
  });

  it("uses control as authority when the top-level active turn is stale", () => {
    const history: ChatHistory = {
      status: "running",
      active_turn: control(8, "turn-stale").active_turn,
      control: control(9, null),
      messages: turnMessages("turn-stale", "in_progress"),
    };

    expect(deriveTurnActivity(history)).toEqual({ kind: "idle" });
  });

  it("represents accepted local activity before authority returns a turn id", () => {
    expect(deriveTurnActivity({ status: "idle", messages: [] }, true)).toEqual({
      kind: "active",
      turnId: null,
      userMessageId: null,
      responseId: null,
    });
  });

  it("does not let stable messages close an authority-owned active turn", () => {
    const turnId = "turn-current";
    const current: ChatHistory = {
      status: "running",
      active_turn: control(9, turnId).active_turn,
      control: control(9, turnId),
      messages: [],
    };
    const stable: ChatHistory = {
      messages: completedTurnMessages(turnId),
    };

    const merged = reconcileStableConversationHistory(current, stable);
    expect(merged.messages).toEqual(stable.messages);
    expect(merged.status).toBe("running");
    expect(merged.active_turn?.turn_id).toBe(turnId);
    expect(merged.control?.active_turn?.turn_id).toBe(turnId);
    expect(merged.control?.revision).toBe(9);
  });

  it("retains a live active turn when stable history only covers older turns", () => {
    const current: ChatHistory = {
      status: "running",
      active_turn: control(9, "turn-current").active_turn,
      control: control(9, "turn-current"),
      messages: [],
    };
    const stable: ChatHistory = {
      messages: completedTurnMessages("turn-older"),
    };

    const merged = reconcileStableConversationHistory(current, stable);
    expect(merged.status).toBe("running");
    expect(merged.active_turn?.turn_id).toBe("turn-current");
    expect(merged.control?.revision).toBe(9);
  });

  it("keeps the active turn running when stable history holds a mid-turn completed record", () => {
    const turnId = "turn-current";
    const current: ChatHistory = {
      status: "running",
      active_turn: control(9, turnId).active_turn,
      control: control(9, turnId),
      messages: [],
    };

    const stable: ChatHistory = {
      messages: turnMessages(turnId, "in_progress"),
    };

    const merged = reconcileStableConversationHistory(current, stable);
    expect(merged.status).toBe("running");
    expect(merged.active_turn?.turn_id).toBe(turnId);
    expect(merged.control?.active_turn?.turn_id).toBe(turnId);
  });

  it("does not let a message lifecycle status close the authority-owned Turn", () => {
    const turnId = "turn-current";
    const current: ChatHistory = {
      status: "running",
      active_turn: control(9, turnId).active_turn,
      control: control(9, turnId),
      messages: [],
    };
    const stable: ChatHistory = {
      messages: completedTurnMessages(turnId).map((message) => ({
        ...message,
        status: "completed",
      })),
    };

    const merged = reconcileStableConversationHistory(current, stable);
    expect(merged.status).toBe("running");
    expect(merged.active_turn?.turn_id).toBe(turnId);
    expect(merged.control?.active_turn?.turn_id).toBe(turnId);
  });

  it("closes the active turn only when a newer control snapshot says it is idle", () => {
    const turnId = "turn-current";
    const current: ChatHistory = {
      status: "running",
      active_turn: control(9, turnId).active_turn,
      control: control(9, turnId),
      messages: [],
    };
    const stable: ChatHistory = {
      status: "idle",
      active_turn: null,
      control: control(10, null),
      messages: completedTurnMessages(turnId),
    };

    const merged = reconcileStableConversationHistory(current, stable);
    expect(merged.status).toBe("idle");
    expect(merged.active_turn).toBeNull();
    expect(merged.control?.active_turn).toBeNull();
    expect(merged.control?.revision).toBe(10);
  });

  it("does not let an older control snapshot replace a newer active turn", () => {
    const current: ChatHistory = {
      status: "running",
      active_turn: control(10, "turn-new").active_turn,
      control: control(10, "turn-new"),
      messages: [],
    };
    const stable: ChatHistory = {
      status: "idle",
      active_turn: null,
      control: control(9, null),
      messages: completedTurnMessages("turn-old"),
    };

    const merged = reconcileStableConversationHistory(current, stable);
    expect(merged.status).toBe("running");
    expect(merged.active_turn?.turn_id).toBe("turn-new");
    expect(merged.control?.revision).toBe(10);
  });

  it("accepts a lower revision from a new authority epoch", () => {
    const currentControl = control(10, "turn-old", "epoch-old");
    const stableControl = control(1, "turn-new", "epoch-new");
    const current: ChatHistory = {
      status: "running",
      active_turn: currentControl.active_turn,
      control: currentControl,
      messages: [],
    };
    const stable: ChatHistory = {
      status: "running",
      active_turn: stableControl.active_turn,
      control: stableControl,
      messages: [],
    };

    const merged = reconcileStableConversationHistory(current, stable);
    expect(merged.active_turn?.turn_id).toBe("turn-new");
    expect(merged.control?.revision).toBe(1);
    expect(merged.control?.placement?.authority_epoch).toBe("epoch-new");
  });
  it.each(["completed", "interrupted", "interjected", "failed"])(
    "settles an old active snapshot from the synced %s Turn lifecycle",
    (turnStatus) => {
      const turnId = "turn-current";
      const current: ChatHistory = {
        status: "running",
        active_turn: control(9, turnId).active_turn,
        control: control(9, turnId),
        messages: [],
      };
      const stable: ChatHistory = {
        messages: turnMessages(turnId, turnStatus).map((message) => ({
          ...message,
          finished_at: "2026-07-31T00:01:00Z",
        })),
      };
      const merged = reconcileStableConversationHistory(current, stable);
      expect(merged.messages).toEqual(stable.messages);
      expect(merged.status).toBe("idle");
      expect(merged.active_turn).toBeNull();
      expect(merged.control?.active_turn).toBeNull();
      expect(merged.control?.revision).toBe(9);
      const projection = projectConversation("chat", merged);
      expect(projection.turnActivity.kind).toBe("idle");
      expect(
        projection.items.find((item) => item.kind === "assistant"),
      ).toMatchObject({
        status: turnStatus === "interjected" ? "interrupted" : turnStatus,
      });
    },
  );

  it("does not let an old active snapshot override a terminal Turn during direct projection", () => {
    const turnId = "turn-current";
    expect(
      deriveTurnActivity(
        {
          control: control(9, turnId),
          messages: turnMessages(turnId, "interrupted").map((message) => ({
            ...message,
            finished_at: "2026-07-31T00:01:00Z",
          })),
        },
        true,
      ),
    ).toEqual({ kind: "idle" });
  });

  it("does not close another active Turn from an older terminal Turn", () => {
    const merged = reconcileStableConversationHistory(
      {
        control: control(9, "turn-new"),
        messages: [],
      },
      {
        messages: turnMessages("turn-old", "interrupted").map((message) => ({
          ...message,
          finished_at: "2026-07-31T00:01:00Z",
        })),
      },
    );
    expect(merged.active_turn?.turn_id).toBe("turn-new");
  });

  it("keeps a later regeneration active when it reuses the terminal Turn id", () => {
    const turnId = "turn-current";
    const regenerated = control(10, turnId);
    regenerated.active_turn!.started_at = "2026-07-31T00:02:00Z";
    const merged = reconcileStableConversationHistory(
      {
        control: regenerated,
        messages: [],
      },
      {
        messages: turnMessages(turnId, "interrupted").map((message) => ({
          ...message,
          finished_at: "2026-07-31T00:01:00Z",
        })),
      },
    );
    expect(merged.status).toBe("running");
    expect(deriveTurnActivity(merged).kind).toBe("active");
  });

  it.each([undefined, "invalid"])(
    "requires a valid terminal time (%s)",
    (finishedAt) => {
      const turnId = "turn-current";
      const merged = reconcileStableConversationHistory(
        {
          control: control(9, turnId),
          messages: [],
        },
        {
          messages: turnMessages(turnId, "interrupted").map((message) => ({
            ...message,
            finished_at: finishedAt,
          })),
        },
      );
      expect(deriveTurnActivity(merged).kind).toBe("active");
    },
  );

  it("keeps a newer same-millisecond regeneration active at Rust timestamp precision", () => {
    const turnId = "turn-current";
    const regenerated = control(10, turnId);
    regenerated.active_turn!.started_at = "2026-07-31T00:01:00.123900123Z";
    const merged = reconcileStableConversationHistory(
      {
        control: regenerated,
        messages: [],
      },
      {
        messages: turnMessages(turnId, "interrupted").map((message) => ({
          ...message,
          finished_at: "2026-07-31T08:01:00.123100456+08:00",
        })),
      },
    );
    expect(deriveTurnActivity(merged).kind).toBe("active");
  });
});
