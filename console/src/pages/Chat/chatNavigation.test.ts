import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearDraftChatId,
  createLazyChatId,
  draftChatKey,
  draftChatModelKey,
  forgetChatId,
  lastChatKey,
  loadDraftChatId,
  loadDraftChatModel,
  loadLastChatId,
  loadRecentChatIds,
  recentChatKey,
  resolveChatEntry,
  resolveOrCreateDraftChatId,
  saveDraftChatId,
  saveDraftChatModel,
  saveLastChatId,
} from "./chatNavigation";

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

  it("stores draft chat ids per agent under the draft prefix", () => {
    expect(draftChatKey("agent-a")).toBe("rua_draft_chat_id_agent-a");
    expect(loadDraftChatId("agent-a")).toBeNull();

    saveDraftChatId("agent-a", "draft-a");
    saveDraftChatId("agent-b", "draft-b");

    expect(loadDraftChatId("agent-a")).toBe("draft-a");
    expect(loadDraftChatId("agent-b")).toBe("draft-b");
  });

  it("returns existing draft or creates and persists a new one", () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("fresh-draft");

    expect(resolveOrCreateDraftChatId("agent-a")).toBe("fresh-draft");
    expect(loadDraftChatId("agent-a")).toBe("fresh-draft");
    expect(randomUUID).toHaveBeenCalledTimes(1);

    randomUUID.mockReturnValue("should-not-be-used");
    expect(resolveOrCreateDraftChatId("agent-a")).toBe("fresh-draft");
    expect(randomUUID).toHaveBeenCalledTimes(1);
  });

  it("routes resolveChatEntry fallback through the draft mechanism", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("lazy-chat");

    expect(resolveChatEntry("agent-b")).toBe("lazy-chat");
    expect(loadDraftChatId("agent-b")).toBe("lazy-chat");
    expect(resolveChatEntry("agent-b")).toBe("lazy-chat");
  });

  it("clears drafts unconditionally or only when the chat id matches", () => {
    saveDraftChatId("agent-a", "draft-a");

    clearDraftChatId("agent-a", "other-chat");
    expect(loadDraftChatId("agent-a")).toBe("draft-a");

    clearDraftChatId("agent-a", "draft-a");
    expect(loadDraftChatId("agent-a")).toBeNull();

    saveDraftChatId("agent-a", "draft-a2");
    clearDraftChatId("agent-a");
    expect(loadDraftChatId("agent-a")).toBeNull();
  });

  it("keeps a lazy chat model with its draft and clears both together", () => {
    const model = { provider_id: "dashscope", model: "qwen-plus" };
    saveDraftChatId("agent-a", "draft-a");
    saveDraftChatModel("agent-a", "draft-a", model);

    expect(draftChatModelKey("agent-a", "draft-a")).toBe(
      "rua_draft_chat_model_agent-a:draft-a",
    );
    expect(loadDraftChatModel("agent-a", "draft-a")).toEqual(model);

    clearDraftChatId("agent-a", "draft-a");
    expect(loadDraftChatId("agent-a")).toBeNull();
    expect(loadDraftChatModel("agent-a", "draft-a")).toBeNull();
  });
});
