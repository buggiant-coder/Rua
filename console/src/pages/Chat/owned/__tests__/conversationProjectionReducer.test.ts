import { describe, expect, it } from "vitest";
import type { ChatHistory, Message } from "@/api/types";
import { initialConversationStreamFold } from "../stream";
import {
  reduceConversationProjection,
  type ConversationProjectionState,
} from "../conversationProjectionReducer";

function message(id: string, role: Message["role"], text: string): Message {
  return { id, role, content: text, turn_id: "turn-1" };
}

function history(
  messages: Message[],
  sequence: number,
  hash: string,
): ChatHistory {
  return {
    messages,
    durable_coverage: {
      latest_message_sequence: sequence,
      manifest_hash: hash,
    },
  };
}

const noRuntimeInput = {
  pendingDeliveries: [],
  activeRun: { user: null, fold: null },
};

describe("reduceConversationProjection", () => {
  it("keeps durable coverage monotonic when an older History response arrives", () => {
    const current = reduceConversationProjection(null, {
      ...noRuntimeInput,
      serverHistory: history(
        [message("assistant-2", "assistant", "new")],
        2,
        "sha256:new",
      ),
    });

    const reduced = reduceConversationProjection(current, {
      ...noRuntimeInput,
      serverHistory: history(
        [message("assistant-1", "assistant", "old")],
        1,
        "sha256:old",
      ),
    });

    expect(reduced.durableCoverage).toEqual(current.durableCoverage);
    expect(reduced.history?.durable_coverage).toEqual(current.durableCoverage);
  });

  it("renders one Message when Live and durable History use the same identity", () => {
    const previous: ConversationProjectionState = {
      history: history([], 0, "sha256:empty"),
      durableCoverage: {
        latest_message_sequence: 0,
        manifest_hash: "sha256:empty",
      },
    };
    const reduced = reduceConversationProjection(previous, {
      serverHistory: history(
        [message("assistant-1", "assistant", "stable")],
        1,
        "sha256:one",
      ),
      pendingDeliveries: [],
      activeRun: {
        user: null,
        fold: {
          ...initialConversationStreamFold,
          output: [message("assistant-1", "assistant", "live")],
        },
      },
    });

    expect(reduced.history?.messages).toHaveLength(1);
    expect(reduced.history?.messages[0]).toMatchObject({
      id: "assistant-1",
      content: "live",
    });

    const stable = reduceConversationProjection(reduced, {
      ...noRuntimeInput,
      serverHistory: history(
        [message("assistant-1", "assistant", "live")],
        2,
        "sha256:stable",
      ),
    });
    expect(stable.history?.messages).toHaveLength(1);
    expect(stable.history?.messages[0]).toMatchObject({
      id: "assistant-1",
      content: "live",
    });
    expect(stable.durableCoverage?.latest_message_sequence).toBe(2);
  });
});
