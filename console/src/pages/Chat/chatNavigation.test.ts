import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  createLazyChatId,
  forgetChatId,
  lastChatKey,
  loadLastChatId,
  loadRecentChatIds,
  recentChatKey,
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
} from "./conversationLifecycle";

describe("chatNavigation", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
    sessionStorage.clear();
  });

  it("keeps last-chat ids isolated per selected Rust agent", () => {
    saveLastChatId("agent-a", "chat-a");
    saveLastChatId("agent-b", "chat-b");

    expect(lastChatKey("agent-a")).toBe("rua_last_chat_id_agent-a");
    expect(recentChatKey("agent-a")).toBe("rua_recent_chat_ids_agent-a");
    expect(loadLastChatId("agent-a")).toBe("chat-a");
    expect(loadLastChatId("agent-b")).toBe("chat-b");
    expect(loadRecentChatIds("agent-a")).toEqual(["chat-a"]);
    expect(loadRecentChatIds("agent-b")).toEqual(["chat-b"]);
  });

  it("resolves chat entry from stored agent history before creating a lazy id", () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("lazy-chat");
    saveLastChatId("agent-a", "stored-chat");

    expect(resolveChatEntry("agent-a")).toBe("stored-chat");
    expect(randomUUID).not.toHaveBeenCalled();
    expect(resolveChatEntry("agent-b")).toBe("lazy-chat");
  });

  it("clears only the matching selected-agent chat id", () => {
    saveLastChatId("agent-a", "chat-a");

    forgetChatId("agent-a", "other-chat");
    expect(loadLastChatId("agent-a")).toBe("chat-a");

    forgetChatId("agent-a", "chat-a");
    expect(loadLastChatId("agent-a")).toBeNull();
  });

  it("falls back to the next recent chat when the active hint is forgotten", () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("lazy-chat");
    saveLastChatId("agent-a", "older-chat");
    saveLastChatId("agent-a", "newer-chat");

    forgetChatId("agent-a", "newer-chat");

    expect(loadLastChatId("agent-a")).toBeNull();
    expect(loadRecentChatIds("agent-a")).toEqual(["older-chat"]);
    expect(resolveChatEntry("agent-a")).toBe("older-chat");
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

    expect(() => saveLastChatId("", "chat")).not.toThrow();
    expect(() => saveLastChatId("agent-a", "")).not.toThrow();
    expect(() => saveLastChatId("agent-a", "chat-a")).not.toThrow();
    expect(loadLastChatId("agent-a")).toBeNull();

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
    expect(resolveChatEntry("agent-b")).toBe(id);
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
