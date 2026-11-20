import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  createLazyChatId,
  forgetChatId,
  openChatDraft,
  resolveChatEntry,
  saveLastChatId,
} from "./chatNavigation";

import { useAgentStore } from "@/stores/agentStore";
import { readDraftChatContext } from "./chatContext";
import {
  chatDraftScope,
  clearDraftConversation,
  isDraftConversation,
  loadLastConversationId,
  loadRecentConversationIds,
} from "./conversationLifecycle";

describe("chatNavigation", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
    sessionStorage.clear();
  });

  it("keeps one ordered standalone history across Agent selections", () => {
    useAgentStore.setState({ selectedAgent: "agent-a" });
    saveLastChatId("chat-a");
    useAgentStore.setState({ selectedAgent: "agent-b" });
    saveLastChatId("chat-b");
    expect(loadLastConversationId(chatDraftScope())).toBe("chat-b");
    expect(loadRecentConversationIds(chatDraftScope())).toEqual([
      "chat-b",
      "chat-a",
    ]);
    expect(loadRecentConversationIds(chatDraftScope("project-one"))).toEqual(
      [],
    );
  });

  it("resolves chat entry from stored standalone history before creating a lazy id", () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("lazy-chat");
    saveLastChatId("stored-chat");

    expect(resolveChatEntry()).toBe("stored-chat");
    expect(randomUUID).not.toHaveBeenCalled();
    forgetChatId("stored-chat");
    expect(resolveChatEntry()).toBe("lazy-chat");
  });

  it("clears only the matching standalone chat id", () => {
    saveLastChatId("chat-a");

    forgetChatId("other-chat");
    expect(loadLastConversationId(chatDraftScope())).toBe("chat-a");

    forgetChatId("chat-a");
    expect(loadLastConversationId(chatDraftScope())).toBeNull();
  });

  it("falls back to the next recent chat when the active hint is forgotten", () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("lazy-chat");
    saveLastChatId("older-chat");
    saveLastChatId("newer-chat");

    forgetChatId("newer-chat");

    expect(loadLastConversationId(chatDraftScope())).toBeNull();
    expect(loadRecentConversationIds(chatDraftScope())).toEqual(["older-chat"]);
    expect(resolveChatEntry()).toBe("older-chat");
    expect(randomUUID).not.toHaveBeenCalled();
  });

  it("ignores empty inputs and unavailable storage", () => {
    const setSpy = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {
        throw new Error("storage disabled");
      });
    const getSpy = vi
      .spyOn(Storage.prototype, "getItem")
      .mockImplementation(() => {
        throw new Error("storage disabled");
      });

    expect(() => saveLastChatId("")).not.toThrow();
    expect(() => saveLastChatId("chat-a")).not.toThrow();
    expect(loadLastConversationId(chatDraftScope())).toBeNull();

    expect(setSpy).toHaveBeenCalledTimes(1);
    expect(getSpy).toHaveBeenCalledTimes(1);
  });

  it("creates lazy chat ids from crypto UUIDs", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-chat");

    expect(createLazyChatId()).toBe("lazy-chat");
  });

  it("reuses the no-Project draft across Agents without changing its input identity", () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("fresh-draft");
    useAgentStore.setState({ selectedAgent: "agent-a" });
    const id = openChatDraft();
    expect(readDraftChatContext(id)).toEqual({
      agentId: "agent-a",
      projectId: null,
    });
    useAgentStore.setState({ selectedAgent: "agent-b" });
    expect(resolveChatEntry()).toBe(id);
    expect(readDraftChatContext(id)?.agentId).toBe("agent-a");
    expect(openChatDraft()).toBe(id);
    expect(readDraftChatContext(id)).toEqual({
      agentId: "agent-b",
      projectId: null,
    });
    expect(randomUUID).toHaveBeenCalledOnce();
  });

  it("clears only the matching draft, preserving other Project and existing Chat state", () => {
    const first = openChatDraft();
    const other = openChatDraft("project-1");
    clearDraftConversation(chatDraftScope(), other);
    expect(isDraftConversation(chatDraftScope(), first)).toBe(true);
    clearDraftConversation(chatDraftScope(), first);
    expect(readDraftChatContext(first)).toBeNull();
    expect(isDraftConversation(chatDraftScope("project-1"), other)).toBe(true);
    expect(openChatDraft()).not.toBe(first);
  });
});
