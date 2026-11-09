import { beforeEach, describe, expect, it, vi } from "vitest";
import * as drafts from "../Chat/drafts";
import {
  loadDraftChatId,
  loadDraftChatModel,
  saveDraftChatModel,
} from "../Chat/chatNavigation";
import {
  clearProjectChatDraftState,
  createProjectChat,
  discardProjectLocalState,
  lastProjectChatId,
  lastProjectId,
  projectLastSelectedAgent,
  projectChatDraftKey,
  projectChatExecutionAgent,
  projectChatPath,
  materializeProjectChat,
  rememberProjectChat,
  resolveProjectChatPath,
  setProjectChatDraftAgent,
  setProjectLastSelectedAgent,
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
    vi.clearAllMocks();
  });

  it("uses one browser-local recent Agent hint per Project", () => {
    setProjectLastSelectedAgent("project-1", "code");
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-2");

    const chatId = createProjectChat("project-1");

    expect(projectLastSelectedAgent("project-1")).toBe("code");
    expect(projectLastSelectedAgent("project-2")).toBeNull();
    expect(projectChatExecutionAgent("project-1", chatId)).toBe("code");
  });

  it("changes only the Project Chat draft assignment", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-3");
    const chatId = createProjectChat("project-1");

    setProjectChatDraftAgent("project-1", chatId, "qa");

    expect(projectChatExecutionAgent("project-1", chatId)).toBe("qa");
    expect(projectLastSelectedAgent("project-1")).toBeNull();
  });

  it("creates a URL identity with an independent default Agent", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-1");

    const chatId = createProjectChat("project-1");

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

    const firstChatId = createProjectChat("project-1");
    drafts.save(projectChatDraftKey("project-1", firstChatId), "pending work");
    const secondChatId = createProjectChat("project-1");

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
    const firstChatId = createProjectChat("project-1");
    drafts.save(projectChatDraftKey("project-1", firstChatId), "keep me");

    const secondChatId = createProjectChat("project-1");

    expect(secondChatId).toBe(firstChatId);
    expect(drafts.load(projectChatDraftKey("project-1", firstChatId))).toBe(
      "keep me",
    );
  });

  it("creates a new lazy URL identity after materialization", () => {
    vi.spyOn(crypto, "randomUUID")
      .mockReturnValueOnce("project-chat-1")
      .mockReturnValueOnce("project-chat-2");
    const firstChatId = createProjectChat("project-1", "agent-a");

    clearProjectChatDraftState("project-1", firstChatId);
    const secondChatId = createProjectChat("project-1", "agent-b");

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

    const first = createProjectChat("project-1", "agent-a");
    const second = createProjectChat("project-2", "agent-b");

    expect(createProjectChat("project-1")).toBe(first);
    expect(createProjectChat("project-2")).toBe(second);
  });

  it("clears pre-submit Agent, model, and composer drafts after materialization", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-1");
    const chatId = createProjectChat("project-1");
    drafts.save(projectChatDraftKey("project-1", chatId), "pending work");
    saveDraftChatModel("project:project-1", chatId, {
      provider_id: "dashscope",
      model: "qwen-plus",
    });

    materializeProjectChat("project-1", chatId);
    clearProjectChatDraftState("project-1", chatId);

    expect(loadDraftChatId("project:project-1")).toBeNull();
    expect(projectChatExecutionAgent("project-1", chatId)).toBe("default");
    expect(loadDraftChatModel("project:project-1", chatId)).toBeNull();
    expect(drafts.load(projectChatDraftKey("project-1", chatId))).toBe("");
    expect(lastProjectChatId("project-1")).toBe(chatId);
  });

  it("stores the pre-submit model through the existing Chat draft path", () => {
    vi.spyOn(crypto, "randomUUID").mockReturnValue("project-chat-model");
    const chatId = createProjectChat("project-1");

    saveDraftChatModel("project:project-1", chatId, {
      provider_id: "dashscope",
      model: "qwen-max",
    });

    expect(loadDraftChatModel("project:project-1", chatId)).toEqual({
      provider_id: "dashscope",
      model: "qwen-max",
    });
    expect(projectChatExecutionAgent("project-1", chatId)).toBe("default");
  });

  it("clears every browser-local hint and draft owned by a deleted Project", () => {
    rememberProjectChat("project-1", "chat-materialized");
    setProjectLastSelectedAgent("project-1", "code");
    vi.spyOn(crypto, "randomUUID").mockReturnValue("chat-lazy");
    createProjectChat("project-1");
    drafts.save(
      projectChatDraftKey("project-1", "chat-materialized"),
      "materialized draft",
    );
    drafts.save(projectChatDraftKey("project-2", "chat-other"), "keep");

    discardProjectLocalState("project-1");

    expect(lastProjectId()).toBeNull();
    expect(lastProjectChatId("project-1")).toBeNull();
    expect(projectLastSelectedAgent("project-1")).toBeNull();
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
      "/projects/project-1/chats/chat-1",
    );
    expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
  });

  it("preserves a remembered remote Lazy Chat address", async () => {
    rememberProjectChat("project-1", "chat-unmaterialized");

    await expect(resolveProjectChatPath("project-1")).resolves.toBe(
      "/projects/project-1/chats/chat-unmaterialized",
    );
    expect(projectChatsApiMock.getChat).not.toHaveBeenCalled();
  });

  it("encodes canonical Project and Chat ids", () => {
    expect(projectChatPath("project/one", "chat/one")).toBe(
      "/projects/project%2Fone/chats/chat%2Fone",
    );
  });
});
