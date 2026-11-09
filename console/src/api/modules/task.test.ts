import { beforeEach, describe, expect, it, vi } from "vitest";
import { request } from "../request";
import { taskApi } from "./task";

vi.mock("../request", () => ({
  request: vi.fn(),
}));

describe("taskApi", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("scopes list and detail routes by agent and chat", async () => {
    vi.mocked(request).mockResolvedValue([]);

    const target = { scope: "agent" as const, agentId: "agent/a" };
    await taskApi.listTasks(target, "chat/b", {
      state: "running",
      limit: 20,
    });
    await taskApi.getTask(target, "chat/b", "task/c");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent%2Fa/chats/chat%2Fb/tasks?state=running&limit=20",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2Fa/chats/chat%2Fb/tasks/task%2Fc",
    );
  });

  it("uses the canonical cancel, retry, result, and activity routes", async () => {
    vi.mocked(request).mockResolvedValue({});
    const target = { scope: "agent" as const, agentId: "agent" };

    await taskApi.cancelTask(target, "chat", "task");
    await taskApi.retryTask(target, "chat", "task", "agent");
    await taskApi.getFullResult(target, "chat", "task");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent/chats/chat/tasks/task/cancel",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent/chats/chat/tasks/task/retry",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent/chats/chat/tasks/task/result",
    );
    expect(taskApi.activityPath(target, "chat")).toBe(
      "/agents/agent/chats/chat/tasks/activity",
    );
  });

  it("uses Project-scoped Task routes for Project Chats", async () => {
    vi.mocked(request).mockResolvedValue([]);
    const target = {
      scope: "project" as const,
      projectId: "project/a",
      agentId: "agent-a",
    };

    await taskApi.listTasks(target, "chat/b");
    await taskApi.getTask(target, "chat/b", "task/c");
    await taskApi.cancelTask(target, "chat/b", "task/c");
    await taskApi.retryTask(target, "chat/b", "task/c", "agent-b");
    await taskApi.getFullResult(target, "chat/b", "task/c");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/projects/project%2Fa/chats/chat%2Fb/tasks",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/projects/project%2Fa/chats/chat%2Fb/tasks/task%2Fc",
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/projects/project%2Fa/chats/chat%2Fb/tasks/task%2Fc/cancel",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/projects/project%2Fa/chats/chat%2Fb/tasks/task%2Fc/retry",
      {
        method: "POST",
        body: JSON.stringify({ authorization_agent_id: "agent-b" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/projects/project%2Fa/chats/chat%2Fb/tasks/task%2Fc/result",
    );
    expect(taskApi.activityPath(target, "chat/b")).toBe(
      "/projects/project%2Fa/chats/chat%2Fb/tasks/activity",
    );
  });
});
