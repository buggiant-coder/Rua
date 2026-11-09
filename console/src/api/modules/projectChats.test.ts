import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));
vi.mock("../config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));
vi.mock("../authHeaders", () => ({
  buildAuthHeaders: vi.fn(() => ({ "X-Test": "1" })),
}));

import { request } from "../request";
import { buildAuthHeaders } from "../authHeaders";
import { projectChatsApi } from "./projectChats";

describe("projectChatsApi", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("requests only scoped Chat metadata and forwards cancellation", async () => {
    const controller = new AbortController();
    await projectChatsApi.getMetadata("project/one", "chat/one", {
      signal: controller.signal,
    });
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/metadata",
      { signal: controller.signal },
    );
  });

  it("lists Project Chats with encoded search, pagination, auth, and total count", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify([{ id: "chat-1", name: "Running" }]), {
        status: 200,
        headers: {
          "Content-Type": "application/json",
          "X-Total-Count": "27",
        },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      projectChatsApi.listChats("project/one", {
        offset: 20,
        limit: 10,
        search: " fix race ",
        tag: "none",
      }),
    ).resolves.toEqual({
      data: [{ id: "chat-1", name: "Running" }],
      total: 27,
    });

    expect(buildAuthHeaders).toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/projects/project%2Fone/chats?offset=20&limit=10&search=fix+race&tag=none",
      {
        headers: { "X-Test": "1" },
        cache: "no-store",
      },
    );
  });

  it("rejects failed Project Chat list requests without fabricating rows", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response("", { status: 503 })),
    );

    await expect(
      projectChatsApi.listChats("project-1", { offset: 0, limit: 20 }),
    ).rejects.toThrow("Failed to list Project Chats: 503");
  });

  it("patches only requested Project Chat management fields", async () => {
    await projectChatsApi.updateChat("project/one", "chat/one", {
      name: "Renamed",
      pinned: true,
      pin_order: 42,
    });

    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone",
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          name: "Renamed",
          pinned: true,
          pin_order: 42,
        }),
      },
    );
  });

  it("uses Project-scoped routes for variant, rollback, and branch actions", async () => {
    await projectChatsApi.selectVariant(
      "project/one",
      "chat/one",
      "turn-1",
      "variant-2",
    );
    await projectChatsApi.rollback("project/one", "chat/one", {
      before_turn_id: "turn-2",
    });
    await projectChatsApi.branch("project/one", "chat/one", {
      before_turn_id: "turn-2",
    });

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/projects/project%2Fone/chats/chat%2Fone",
      {
        method: "PUT",
        body: JSON.stringify({
          turn_id: "turn-1",
          selected_variant_id: "variant-2",
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/projects/project%2Fone/chats/chat%2Fone/rollback",
      {
        method: "POST",
        body: JSON.stringify({ before_turn_id: "turn-2" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/projects/project%2Fone/chats/chat%2Fone/branch",
      {
        method: "POST",
        body: JSON.stringify({ before_turn_id: "turn-2" }),
      },
    );
  });

  it("deletes through the Project Chat route with explicit force state", async () => {
    await projectChatsApi.deleteChat("project/one", "chat/one", true);

    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone?force=true",
      { method: "DELETE" },
    );
  });

  it("commits Agent selection through the canonical Project Chat route", async () => {
    await projectChatsApi.switchAgent(
      "project/one",
      "chat/one",
      "default",
      "code",
    );

    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/agent-selection",
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          expected_agent_id: "default",
          agent_id: "code",
        }),
      },
    );
  });

  it("routes Project model, options, Stop, and queue controls through canonical siblings", async () => {
    const config = {
      max_iters: 12,
      context_compact: {},
      tool_result_compact: {},
    } as never;
    const sendNow = {
      command_id: "command-1",
      authority_epoch: "epoch-1",
    };

    await projectChatsApi.updateChat("project/one", "chat/one", {
      model: { provider_id: "openai", model: "gpt-5" },
    });
    await projectChatsApi.getRunningConfig("project/one", "chat/one");
    await projectChatsApi.getResourcePolicy("project/one", "chat/one");
    await projectChatsApi.updateRunningConfig(
      "project/one",
      "chat/one",
      config,
    );
    await projectChatsApi.stopChat("project/one", "chat/one", {
      turn_id: "turn-1",
      authority_epoch: "epoch-1",
    });
    await projectChatsApi.sendNowQueuedInput(
      "project/one",
      "chat/one",
      "input/one",
      sendNow,
    );
    await projectChatsApi.editQueuedInput(
      "project/one",
      "chat/one",
      "input/one",
      "edited",
    );
    await projectChatsApi.deleteQueuedInput(
      "project/one",
      "chat/one",
      "input/one",
    );

    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({
          model: { provider_id: "openai", model: "gpt-5" },
        }),
      }),
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/running-config",
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/resource-policy",
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/running-config",
      { method: "PUT", body: JSON.stringify(config) },
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/console/stop",
      {
        method: "POST",
        body: JSON.stringify({ turn_id: "turn-1", authority_epoch: "epoch-1" }),
      },
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/queue/input%2Fone/send-now",
      { method: "POST", body: JSON.stringify(sendNow) },
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/queue/input%2Fone",
      { method: "PATCH", body: JSON.stringify({ message: "edited" }) },
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/queue/input%2Fone",
      { method: "DELETE" },
    );
  });

  it("routes Project approval and ACP control families without authority selectors", async () => {
    const approvalBody = {
      decision: "run_once" as const,
      decided_by: "webui",
    };
    const enterBody = {
      runner: "codex",
      cwd: "/repo",
      resume_session_id: "session/one",
      target_device_id: "must-not-cross-project-boundary",
    };

    await projectChatsApi.getApprovalSnapshot("project/one", "chat/one");
    await projectChatsApi.decideApproval(
      "project/one",
      "chat/one",
      "approval/one",
      approvalBody,
    );
    await projectChatsApi.listAcpRunners("project/one", "chat/one");
    await projectChatsApi.listAcpSessions(
      "project/one",
      "chat/one",
      "codex/runner",
      " /repo ",
    );
    await projectChatsApi.enterAcp("project/one", "chat/one", enterBody);
    await projectChatsApi.exitAcp("project/one", "chat/one");
    await projectChatsApi.getAcpStatus("project/one", "chat/one");
    await projectChatsApi.getAcpPermissionSnapshot("project/one", "chat/one");
    await projectChatsApi.decideAcpPermission(
      "project/one",
      "chat/one",
      "request/one",
      "allow/once",
    );
    await projectChatsApi.persistAcpHistoryPreview("project/one", "chat/one", {
      preview_id: 7,
      text: "history",
      runner: "codex",
    });

    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/approvals/snapshot",
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/approvals/approval%2Fone/decide",
      { method: "POST", body: JSON.stringify(approvalBody) },
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/acp/runners",
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/acp/sessions?runner=codex%2Frunner&cwd=%2Frepo",
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/acp/enter",
      {
        method: "POST",
        body: JSON.stringify({
          runner: "codex",
          cwd: "/repo",
          resume_session_id: "session/one",
        }),
      },
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/acp/exit",
      { method: "POST" },
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/acp/status",
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/acp/permissions",
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/acp/permissions/request%2Fone/decide",
      {
        method: "POST",
        body: JSON.stringify({ option_id: "allow/once" }),
      },
    );
    expect(request).toHaveBeenCalledWith(
      "/projects/project%2Fone/chats/chat%2Fone/acp-history-preview",
      {
        method: "POST",
        body: JSON.stringify({
          preview_id: 7,
          text: "history",
          runner: "codex",
        }),
      },
    );
  });
});
