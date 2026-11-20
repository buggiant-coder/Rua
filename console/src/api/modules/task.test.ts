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

  it("loads list and detail routes by Chat ID", async () => {
    vi.mocked(request).mockResolvedValue([]);

    await taskApi.listTasks("chat/b", {
      state: "running",
      limit: 20,
    });
    await taskApi.getTask("chat/b", "task/c");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/chats/chat%2Fb/tasks?state=running&limit=20",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/chats/chat%2Fb/tasks/task%2Fc",
    );
  });

  it("uses the canonical cancel, retry, result, and activity routes", async () => {
    vi.mocked(request).mockResolvedValue({});

    await taskApi.cancelTask("chat", "task");
    await taskApi.retryTask("chat", "task", "agent");
    await taskApi.getFullResult("chat", "task");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/chats/chat/tasks/task/cancel",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(2, "/chats/chat/tasks/task/retry", {
      method: "POST",
      body: JSON.stringify({ authorization_agent_id: "agent" }),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/chats/chat/tasks/task/result");
    expect(taskApi.activityPath("chat")).toBe("/chats/chat/tasks/activity");
  });

  it("routes input and watch controls to the Chat ID", async () => {
    await taskApi.sendInput("chat/b", "task/c", {
      type: "text",
      text: "hello",
    });
    await taskApi.watchTask("chat/b", "task/c");
    await taskApi.unwatchTask("chat/b", "task/c");
    expect(request).toHaveBeenCalledWith(
      "/chats/chat%2Fb/tasks/task%2Fc/input",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenCalledWith(
      "/chats/chat%2Fb/tasks/task%2Fc/watch",
      { method: "POST" },
    );
    expect(request).toHaveBeenCalledWith(
      "/chats/chat%2Fb/tasks/task%2Fc/unwatch",
      { method: "POST" },
    );
  });
});
