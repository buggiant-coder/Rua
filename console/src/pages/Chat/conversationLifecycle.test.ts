import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  isDraftConversation,
  loadLastConversationId,
  loadRecentConversationIds,
  materializeConversation,
  resolveOrCreateDraftConversationId,
  loadDraftConversationAcp,
  saveDraftConversationAcp,
} from "./conversationLifecycle";

describe("conversationLifecycle", () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    vi.restoreAllMocks();
  });

  it("retains draft ACP choices across reload reads and isolates scopes", () => {
    const selection = {
      runner: "codex",
      cwd: "/workspace",
      resume_session_id: "resume-1",
    };
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-1");
    resolveOrCreateDraftConversationId("no-project");
    saveDraftConversationAcp("no-project", "chat-1", selection);
    expect(loadDraftConversationAcp("no-project", "chat-1")).toEqual(selection);
    expect(loadDraftConversationAcp("project:agent-1", "chat-1")).toBeNull();
    materializeConversation("no-project", "chat-1");
    expect(loadDraftConversationAcp("no-project", "chat-1")).toBeNull();
  });

  it("keeps no-Project and Project drafts isolated by scope", () => {
    vi.spyOn(crypto, "randomUUID")
      .mockReturnValueOnce("agent-draft")
      .mockReturnValueOnce("project-draft");

    expect(resolveOrCreateDraftConversationId("no-project")).toBe(
      "agent-draft",
    );
    expect(resolveOrCreateDraftConversationId("project:project-1")).toBe(
      "project-draft",
    );
    expect(isDraftConversation("no-project", "agent-draft")).toBe(true);
    expect(isDraftConversation("project:project-1", "project-draft")).toBe(
      true,
    );
    expect(isDraftConversation("no-project", "project-draft")).toBe(false);
  });

  it("does not advertise a draft as recent until it materializes", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("draft-1");
    const chatId = resolveOrCreateDraftConversationId("project:project-1");

    expect(loadLastConversationId("project:project-1")).toBeNull();
    expect(loadRecentConversationIds("project:project-1")).toEqual([]);

    materializeConversation("project:project-1", chatId);

    expect(isDraftConversation("project:project-1", chatId)).toBe(false);
    expect(loadLastConversationId("project:project-1")).toBe(chatId);
    expect(loadRecentConversationIds("project:project-1")).toEqual([chatId]);
  });
});
