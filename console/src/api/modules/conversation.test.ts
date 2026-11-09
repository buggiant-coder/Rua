import { afterEach, describe, expect, it, vi } from "vitest";

const {
  acpApiMock,
  agentsApiMock,
  chatApiMock,
  consoleApiMock,
  projectChatsApiMock,
} = vi.hoisted(() => ({
  acpApiMock: {
    listChatRunners: vi.fn(),
    getPermissionSnapshot: vi.fn(),
  },
  agentsApiMock: {
    getAgentRunningConfig: vi.fn(),
    updateAgentRunningConfig: vi.fn(),
  },
  chatApiMock: {
    getChat: vi.fn(),
    getRunningConfig: vi.fn(),
    updateRunningConfig: vi.fn(),
    submitPath: vi.fn(
      (chatId: string, agentId: string) =>
        `/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
          chatId,
        )}/console/submit`,
    ),
    livePath: vi.fn(
      (chatId: string, agentId: string) =>
        `/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
          chatId,
        )}/console/live`,
    ),
    updateChat: vi.fn(),
    stopChat: vi.fn(),
    selectVariant: vi.fn(),
    rollback: vi.fn(),
    branch: vi.fn(),
    sendNowQueuedInput: vi.fn(),
  },
  consoleApiMock: {
    getApprovalSnapshot: vi.fn(),
  },
  projectChatsApiMock: {
    getChat: vi.fn(),
    submitPath: vi.fn(
      (projectId: string, chatId: string) =>
        `/projects/${projectId}/chats/${chatId}/console/submit`,
    ),
    livePath: vi.fn(
      (projectId: string, chatId: string) =>
        `/projects/${projectId}/chats/${chatId}/console/live`,
    ),
    updateChat: vi.fn(),
    stopChat: vi.fn(),
    selectVariant: vi.fn(),
    rollback: vi.fn(),
    branch: vi.fn(),
    sendNowQueuedInput: vi.fn(),
    getRunningConfig: vi.fn(),
    updateRunningConfig: vi.fn(),
    listAcpRunners: vi.fn(),
    getApprovalSnapshot: vi.fn(),
    getAcpPermissionSnapshot: vi.fn(),
  },
}));

vi.mock("./acp", () => ({ acpApi: acpApiMock }));
vi.mock("./agents", () => ({ agentsApi: agentsApiMock }));
vi.mock("./chat", () => ({ chatApi: chatApiMock }));
vi.mock("./console", () => ({ consoleApi: consoleApiMock }));
vi.mock("./projectChats", () => ({ projectChatsApi: projectChatsApiMock }));
vi.mock("../authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer test-token" }),
}));
vi.mock("../config", () => ({ getApiUrl: (path: string) => `/api${path}` }));
const configurationRequest = vi.hoisted(() => vi.fn());
vi.mock("../request", () => ({ request: configurationRequest }));

import {
  agentConversationTarget,
  conversationApi,
  projectConversationTarget,
} from "./conversation";

describe("conversationApi target routing", () => {
  it("routes full configuration preview and exact candidate application by scope", async () => {
    const preview = {
      current: { epoch: "old" },
      candidate: { epoch: "new" },
      applied: false,
    };
    await conversationApi.previewConfiguration(
      agentConversationTarget("agent/one"),
      "chat/two",
    );
    expect(configurationRequest).toHaveBeenLastCalledWith(
      "/agents/agent%2Fone/chats/chat%2Ftwo/configuration",
    );
    await conversationApi.applyConfiguration(
      projectConversationTarget("project/one"),
      "chat/two",
      preview,
    );
    expect(configurationRequest).toHaveBeenLastCalledWith(
      "/projects/project%2Fone/chats/chat%2Ftwo/configuration",
      {
        method: "PUT",
        body: JSON.stringify({ expected_epoch: "old", candidate_epoch: "new" }),
      },
    );
  });
  afterEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
  });

  it("preserves Agent control routes through the existing API modules", async () => {
    const target = agentConversationTarget("agent/one");
    const model = { provider_id: "openai", model: "gpt-5" };
    const sendNow = {
      command_id: "command-1",
      authority_epoch: "epoch-1",
    };
    const fetchMock = vi.fn().mockResolvedValue(new Response(null));
    vi.stubGlobal("fetch", fetchMock);

    await conversationApi.updateModel(target, "chat/one", model);
    await conversationApi.submit(target, "chat/one", { message: "hello" });
    await conversationApi.openLive(
      target,
      "chat/one",
      { viewer_id: "viewer-one" },
      new AbortController().signal,
    );
    await conversationApi.getHistory(target, "chat/one", { turnLimit: 20 });
    await conversationApi.stopChat(target, "chat/one", {
      turn_id: "turn-1",
      authority_epoch: "epoch-1",
    });
    await conversationApi.selectVariant(
      target,
      "chat/one",
      "turn/one",
      "variant/one",
    );
    await conversationApi.rollback(target, "chat/one", {
      before_turn_id: "turn/one",
    });
    await conversationApi.branch(target, "chat/one", {
      before_turn_id: "turn/one",
    });
    await conversationApi.sendNowQueuedInput(
      target,
      "chat/one",
      "input/one",
      sendNow,
    );
    await conversationApi.getRunningConfig(target, "chat/one");
    await conversationApi.getApprovalSnapshot(target, "chat/one");
    await conversationApi.getAcpPermissionSnapshot(target, "chat/one");
    await conversationApi.listAcpRunners(
      target,
      "chat/one",
      "device-authority",
    );

    expect(chatApiMock.updateChat).toHaveBeenCalledWith(
      "chat/one",
      { model },
      "agent/one",
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/agents/agent%2Fone/chats/chat%2Fone/console/submit",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ message: "hello" }),
      }),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/agents/agent%2Fone/chats/chat%2Fone/console/live",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ viewer_id: "viewer-one" }),
      }),
    );
    expect(chatApiMock.getChat).toHaveBeenCalledWith("chat/one", "agent/one", {
      turnLimit: 20,
    });
    expect(chatApiMock.stopChat).toHaveBeenCalledWith(
      "chat/one",
      { turn_id: "turn-1", authority_epoch: "epoch-1" },
      "agent/one",
    );
    expect(chatApiMock.selectVariant).toHaveBeenCalledWith(
      "chat/one",
      "turn/one",
      "variant/one",
      "agent/one",
    );
    expect(chatApiMock.rollback).toHaveBeenCalledWith(
      "chat/one",
      { before_turn_id: "turn/one" },
      "agent/one",
    );
    expect(chatApiMock.branch).toHaveBeenCalledWith(
      "chat/one",
      { before_turn_id: "turn/one" },
      "agent/one",
    );
    expect(chatApiMock.sendNowQueuedInput).toHaveBeenCalledWith(
      "chat/one",
      "input/one",
      sendNow,
      "agent/one",
    );
    expect(chatApiMock.getRunningConfig).toHaveBeenCalledWith(
      "chat/one",
      "agent/one",
    );
    expect(consoleApiMock.getApprovalSnapshot).toHaveBeenCalledWith(
      "agent/one",
      "chat/one",
    );
    expect(acpApiMock.getPermissionSnapshot).toHaveBeenCalledWith(
      "agent/one",
      "chat/one",
    );
    expect(acpApiMock.listChatRunners).toHaveBeenCalledWith(
      "agent/one",
      "chat/one",
      "device-authority",
    );
    expect(projectChatsApiMock.stopChat).not.toHaveBeenCalled();
  });

  it("routes Project controls to Project siblings and ignores ACP device targeting", async () => {
    const target = projectConversationTarget("project/one");
    const fetchMock = vi.fn().mockResolvedValue(new Response(null));
    vi.stubGlobal("fetch", fetchMock);

    await conversationApi.submit(
      target,
      "chat/one",
      { message: "hello" },
      "agent/one",
    );
    await conversationApi.openLive(
      target,
      "chat/one",
      { viewer_id: "viewer-one" },
      new AbortController().signal,
    );
    await conversationApi.getHistory(target, "chat/one", { turnLimit: 20 });
    await conversationApi.stopChat(target, "chat/one", {
      turn_id: "turn-1",
      authority_epoch: "epoch-1",
    });
    await conversationApi.selectVariant(
      target,
      "chat/one",
      "turn/one",
      "variant/one",
    );
    await conversationApi.rollback(target, "chat/one", {
      before_turn_id: "turn/one",
    });
    await conversationApi.branch(target, "chat/one", {
      before_turn_id: "turn/one",
    });
    await conversationApi.getRunningConfig(target, "chat/one");
    await conversationApi.getApprovalSnapshot(target, "chat/one");
    await conversationApi.getAcpPermissionSnapshot(target, "chat/one");
    await conversationApi.listAcpRunners(
      target,
      "chat/one",
      "device-must-be-ignored",
    );

    expect(projectChatsApiMock.stopChat).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
      { turn_id: "turn-1", authority_epoch: "epoch-1" },
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/projects/project/one/chats/chat/one/console/submit",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          message: "hello",
          agent_id: "agent/one",
        }),
      }),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/projects/project/one/chats/chat/one/console/live",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          viewer_id: "viewer-one",
        }),
      }),
    );
    expect(projectChatsApiMock.getChat).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
      { turnLimit: 20 },
    );
    expect(projectChatsApiMock.selectVariant).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
      "turn/one",
      "variant/one",
    );
    expect(projectChatsApiMock.rollback).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
      { before_turn_id: "turn/one" },
    );
    expect(projectChatsApiMock.branch).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
      { before_turn_id: "turn/one" },
    );
    expect(projectChatsApiMock.getRunningConfig).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
    );
    expect(projectChatsApiMock.getApprovalSnapshot).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
    );
    expect(projectChatsApiMock.getAcpPermissionSnapshot).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
    );
    expect(projectChatsApiMock.listAcpRunners).toHaveBeenCalledWith(
      "project/one",
      "chat/one",
    );
    expect(chatApiMock.stopChat).not.toHaveBeenCalled();
    expect(acpApiMock.listChatRunners).not.toHaveBeenCalled();
  });
});
