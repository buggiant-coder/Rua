import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { acpApi } from "./acp";

describe("acpApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust canonical ACP runner routes instead of QwenPaw config/acp aliases", async () => {
    const createBody = {
      key: "claude/code",
      command: "claude",
      args: ["--print"],
    } as Parameters<typeof acpApi.createRunner>[0];
    const updateBody = {
      enabled: false,
    } as Parameters<typeof acpApi.updateRunner>[1];

    await acpApi.listRunners();
    await acpApi.createRunner(createBody);
    await acpApi.getRunner("claude/code");
    await acpApi.checkRunner("claude/code");
    await acpApi.startRunnerInstall("claude/code");
    await acpApi.getRunnerInstallTask("task/1");
    await acpApi.updateRunner("claude/code", updateBody);
    await acpApi.toggleRunner("claude/code");
    await acpApi.deleteRunner("claude/code");

    expect(request).toHaveBeenNthCalledWith(1, "/acp");
    expect(request).toHaveBeenNthCalledWith(2, "/acp", {
      method: "POST",
      body: JSON.stringify(createBody),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/acp/claude%2Fcode");
    expect(request).toHaveBeenNthCalledWith(4, "/acp/claude%2Fcode/check");
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/acp/claude%2Fcode/install/start",
      {
        method: "POST",
      },
    );
    expect(request).toHaveBeenNthCalledWith(6, "/acp/install-tasks/task%2F1");
    expect(request).toHaveBeenNthCalledWith(7, "/acp/claude%2Fcode", {
      method: "PUT",
      body: JSON.stringify(updateBody),
    });
    expect(request).toHaveBeenNthCalledWith(8, "/acp/claude%2Fcode/toggle", {
      method: "PATCH",
    });
    expect(request).toHaveBeenNthCalledWith(9, "/acp/claude%2Fcode", {
      method: "DELETE",
    });
  });

  it("passes target device ids through per-chat ACP lifecycle routes", async () => {
    await acpApi.listChatRunners("agent/a", "chat/1", "device-b");
    await acpApi.listChatSessions(
      "agent/a",
      "chat/1",
      "codex",
      "/workspace",
      "device-b",
    );
    await acpApi.enterChat("agent/a", "chat/1", {
      runner: "codex",
      cwd: "/workspace",
      target_device_id: "device-b",
    });
    await acpApi.exitChat("agent/a", "chat/1", "device-b");
    await acpApi.chatStatus("agent/a", "chat/1", "device-b");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent%2Fa/chats/chat%2F1/acp/runners?target_device_id=device-b",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2Fa/chats/chat%2F1/acp/sessions?runner=codex&cwd=%2Fworkspace&target_device_id=device-b",
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent%2Fa/chats/chat%2F1/acp/enter",
      {
        method: "POST",
        body: JSON.stringify({
          runner: "codex",
          cwd: "/workspace",
          target_device_id: "device-b",
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agents/agent%2Fa/chats/chat%2F1/acp/exit?target_device_id=device-b",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/agents/agent%2Fa/chats/chat%2F1/acp/status?target_device_id=device-b",
    );
  });

  it("uses chat-scoped ACP permission relay routes with exact option ids", async () => {
    await acpApi.getPermissionSnapshot("agent/a", "chat/1");
    await acpApi.decidePermission(
      "agent/a",
      "chat/1",
      "permission/1",
      "allow_session",
    );

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent%2Fa/chats/chat%2F1/acp/permissions",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2Fa/chats/chat%2F1/acp/permissions/permission%2F1/decide",
      {
        method: "POST",
        body: JSON.stringify({ option_id: "allow_session" }),
      },
    );
  });
});
