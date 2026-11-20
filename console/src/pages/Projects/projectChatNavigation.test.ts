import { chatPath } from "../Chat/conversationAddress";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as drafts from "../Chat/drafts";
import { useAgentStore } from "@/stores/agentStore";
import {
  loadDraftConversationId,
  loadDraftConversationModel,
  saveDraftConversationModel,
} from "../Chat/conversationLifecycle";
import { openChatDraft } from "../Chat/chatNavigation";
import {
  clearProjectChatDraftState,
  discardProjectLocalState,
  lastProjectChatId,
  lastProjectId,
  projectChatDraftKey,
  projectChatExecutionAgent,
  materializeProjectChat,
  rememberProjectChat,
  resolveProjectChatPath,
  setProjectChatDraftAgent,
} from "./projectChatNavigation";

const { projectChatsApiMock } = vi.hoisted(() => ({
  projectChatsApiMock: {
    getChat: vi.fn(),
  },
}));

vi.mock("../../api/modules/projectChats", () => ({
  projectChatsApi: projectChatsApiMock,
}));

describe("projectChatNavigation", () => {
  beforeEach(() => {
    sessionStorage.clear();
    localStorage.clear();
    vi.restoreAllMocks();
    useAgentStore.setState({ selectedAgent: "default" });
    vi.clearAllMocks();
  });

  it("uses the current global Agent on each explicit New Chat while preserving the Project draft", () => {
    useAgentStore.getState().setSelectedAgent("code");
    const chatId = openChatDraft("project-1");
    expect(projectChatExecutionAgent("project-1", chatId)).toBe("code");

    const model = {
      provider_id: "local",
      model: "detail",
      thinking_strength: "high" as const,
    };
    saveDraftConversationModel("project:project-1", chatId, model);
    const attachment = {
      id: "image",
      type: "image" as const,
      name: "p.png",
      url: "/media/p",
    };
    const key = projectChatDraftKey("project-1", chatId);
    drafts.save(key, "keep me", [attachment]);
    useAgentStore.getState().setSelectedAgent("research");
    expect(
      openChatDraft("project-1", { provider_id: "other", model: "source" }),
    ).toBe(chatId);
    expect(projectChatExecutionAgent("project-1", chatId)).toBe("research");
    expect(loadDraftConversationModel("project:project-1", chatId)).toEqual(
      model,
    );
    expect(drafts.loadDraft(key)).toEqual({
      text: "keep me",
      attachments: [attachment],
    });
  });

  it("changes only the Project Chat draft assignment", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-3");
    const chatId = openChatDraft("project-1");

    setProjectChatDraftAgent("project-1", chatId, "qa");

    expect(projectChatExecutionAgent("project-1", chatId)).toBe("qa");
  });

  it("creates a URL identity with an independent default Agent", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-1");

    const chatId = openChatDraft("project-1");

    expect(chatId).toBe("project-chat-1");
    expect(projectChatExecutionAgent("project-1", chatId)).toBe("default");
    expect(lastProjectId()).toBe("project-1");
    expect(lastProjectChatId("project-1")).toBeNull();

    materializeProjectChat("project-1", chatId);
    expect(lastProjectChatId("project-1")).toBe(chatId);
  });

  it("reuses one lazy URL identity across repeated New Chat actions", () => {
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValueOnce("project-chat-1")
      .mockReturnValueOnce("project-chat-2");

    const firstChatId = openChatDraft("project-1");
    drafts.save(projectChatDraftKey("project-1", firstChatId), "pending work");
    const secondChatId = openChatDraft("project-1");

    expect(secondChatId).toBe(firstChatId);
    expect(drafts.load(projectChatDraftKey("project-1", firstChatId))).toBe(
      "pending work",
    );
    expect(randomUUID).toHaveBeenCalledTimes(1);
  });

  it("keeps returning to the existing Project Chat draft", () => {
    vi.spyOn(crypto, "randomUUID")
      .mockReturnValueOnce("project-chat-1")
      .mockReturnValueOnce("project-chat-2");
    const firstChatId = openChatDraft("project-1");
    drafts.save(projectChatDraftKey("project-1", firstChatId), "keep me");

    const secondChatId = openChatDraft("project-1");

    expect(secondChatId).toBe(firstChatId);
    expect(drafts.load(projectChatDraftKey("project-1", firstChatId))).toBe(
      "keep me",
    );
  });

  it("remembers a reopened Project without applying New Chat to its existing draft", async () => {
    useAgentStore.getState().setSelectedAgent("code");
    const first = openChatDraft("project-1");
    useAgentStore.getState().setSelectedAgent("research");
    openChatDraft("project-2");
    expect(await resolveProjectChatPath("project-1")).toBe(chatPath(first));
    expect(lastProjectId()).toBe("project-1");
    expect(projectChatExecutionAgent("project-1", first)).toBe("code");
    expect(useAgentStore.getState().selectedAgent).toBe("research");
  });

  it("creates a new lazy URL identity after materialization", () => {
    vi.spyOn(crypto, "randomUUID")
      .mockReturnValueOnce("project-chat-1")
      .mockReturnValueOnce("project-chat-2");
    const firstChatId = openChatDraft("project-1");

    clearProjectChatDraftState("project-1", firstChatId);
    useAgentStore.getState().setSelectedAgent("agent-b");
    const secondChatId = openChatDraft("project-1");

    expect(secondChatId).toBe("project-chat-2");
    expect(secondChatId).not.toBe(firstChatId);
    expect(projectChatExecutionAgent("project-1", secondChatId)).toBe(
      "agent-b",
    );
  });

  it("keeps lazy URL identities isolated between Projects", () => {
    vi.spyOn(crypto, "randomUUID")
      .mockReturnValueOnce("project-chat-1")
      .mockReturnValueOnce("project-chat-2");

    const first = openChatDraft("project-1");
    const second = openChatDraft("project-2");

    expect(openChatDraft("project-1")).toBe(first);
    expect(openChatDraft("project-2")).toBe(second);
  });

  it("clears pre-submit Agent, model, and composer drafts after materialization", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-1");
    const chatId = openChatDraft("project-1");
    drafts.save(projectChatDraftKey("project-1", chatId), "pending work");
    saveDraftConversationModel("project:project-1", chatId, {
      provider_id: "dashscope",
      model: "qwen-plus",
    });

    materializeProjectChat("project-1", chatId);
    clearProjectChatDraftState("project-1", chatId);

    expect(loadDraftConversationId("project:project-1")).toBeNull();
    expect(projectChatExecutionAgent("project-1", chatId)).toBe("default");
    expect(loadDraftConversationModel("project:project-1", chatId)).toBeNull();
    expect(drafts.load(projectChatDraftKey("project-1", chatId))).toBe("");
    expect(lastProjectChatId("project-1")).toBe(chatId);
  });

  it("stores the pre-submit model through the existing Chat draft path", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-model");
    const chatId = openChatDraft("project-1");

    saveDraftConversationModel("project:project-1", chatId, {
      provider_id: "dashscope",
      model: "qwen-max",
    });

    expect(loadDraftConversationModel("project:project-1", chatId)).toEqual({
      provider_id: "dashscope",
      model: "qwen-max",
    });
    expect(projectChatExecutionAgent("project-1", chatId)).toBe("default");
  });

  it("clears every browser-local hint and draft owned by a deleted Project", () => {
    rememberProjectChat("project-1", "chat-materialized");
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-lazy");
    openChatDraft("project-1");
    drafts.save(
      projectChatDraftKey("project-1", "chat-materialized"),
      "materialized draft",
    );
    drafts.save(projectChatDraftKey("project-2", "chat-other"), "keep");

    discardProjectLocalState("project-1");

    expect(lastProjectId()).toBeNull();
    expect(lastProjectChatId("project-1")).toBeNull();
    expect(
      drafts.load(projectChatDraftKey("project-1", "chat-materialized")),
    ).toBe("");
    expect(drafts.load(projectChatDraftKey("project-2", "chat-other"))).toBe(
      "keep",
    );
  });

  it("restores a remembered Chat address without probing its materialization", async () => {
    rememberProjectChat("project-1", "chat-1");

    await expect(resolveProjectChatPath("project-1")).resolves.toBe(
      "/chats/chat-1",
    );
    expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
  });

  it("preserves a remembered remote Lazy Chat address", async () => {
    rememberProjectChat("project-1", "chat-unmaterialized");

    await expect(resolveProjectChatPath("project-1")).resolves.toBe(
      "/chats/chat-unmaterialized",
    );
    expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
  });

  it("encodes canonical Project and Chat ids", () => {
    expect(chatPath("chat/one")).toBe("/chats/chat%2Fone");
  });
});
