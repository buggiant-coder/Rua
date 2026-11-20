import { beforeEach, describe, expect, it, vi } from "vitest";
import { saveDraftConversationId } from "./conversationLifecycle";
import {
  historyProvesConversationMaterialized,
  resolveConversationRoute,
} from "./conversationRouteController";

describe.each(["no-project", "project:project-1"])(
  "conversation route controller for %s",
  (scope) => {
    beforeEach(() => localStorage.clear());

    it("keeps a local draft lazy without loading history", async () => {
      saveDraftConversationId(scope, "chat-draft");
      const loadHistory = vi.fn();

      await expect(
        resolveConversationRoute({
          scope,
          conversationId: "chat-draft",
          loadHistory,
        }),
      ).resolves.toEqual({ history: null, unmaterialized: true });
      expect(loadHistory).not.toHaveBeenCalled();
    });

    it("recognizes the persisted header of an empty Chat", async () => {
      const history = { agent_id: "saved-agent", messages: [], meta: {} };

      await expect(
        resolveConversationRoute({
          scope,
          conversationId: "chat-remote",
          loadHistory: async () => history,
        }),
      ).resolves.toEqual({ history, unmaterialized: false });
    });

    it("recognizes authoritative history as materialized", async () => {
      const history = { messages: [{ role: "user", content: "hello" }] };

      await expect(
        resolveConversationRoute({
          scope,
          conversationId: "chat-saved",
          loadHistory: async () => history,
        }),
      ).resolves.toEqual({ history, unmaterialized: false });
    });
  },
);

it("treats running and authority state as materialization evidence", () => {
  expect(
    historyProvesConversationMaterialized({ messages: [], status: "running" }),
  ).toBe(true);
  expect(
    historyProvesConversationMaterialized({
      messages: [],
      control: { placement: { authority_device_id: "device-1" } },
    }),
  ).toBe(true);
});
