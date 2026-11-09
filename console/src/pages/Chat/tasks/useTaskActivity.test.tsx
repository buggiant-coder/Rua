import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TaskRecord } from "@/api/types/task";
import { upsertTask, useTaskActivity } from "./useTaskActivity";

const taskApiMock = vi.hoisted(() => ({
  listTasks: vi.fn(),
  getTask: vi.fn(),
  cancelTask: vi.fn(),
  retryTask: vi.fn(),
  getFullResult: vi.fn(),
  activityPath: vi.fn(
    (
      target:
        | { scope: "agent"; agentId: string }
        | { scope: "project"; projectId: string },
      chatId: string,
    ) =>
      target.scope === "project"
        ? `/projects/${target.projectId}/chats/${chatId}/tasks/activity`
        : `/agents/${target.agentId}/chats/${chatId}/tasks/activity`,
  ),
}));

const conversationApiMock = vi.hoisted(() => ({
  getAcpPermissionSnapshot: vi.fn(),
  decideAcpPermission: vi.fn(),
}));

vi.mock("@/api/modules/task", () => ({
  taskApi: taskApiMock,
}));

vi.mock("@/api/modules/conversation", () => ({
  conversationApi: conversationApiMock,
}));

vi.mock("@/api/config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));

vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({}),
}));

function record(): TaskRecord {
  return {
    task_id: "task-1",
    spec: {
      owner: {
        chat_address: {
          scope: { kind: "agent", agent_id: "agent-a" },
          chat_id: "chat-a",
        },
      },
      instruction: "Run fixture",
      runner: { type: "sub_agent" },
      context_refs: [],
      depends_on: [],
      origin: {},
    },
    state: "running",
    attempt_no: 1,
    created_at: "2026-07-27T00:00:00Z",
    updated_at: "2026-07-27T00:00:00Z",
    attempts: [],
    waiting_on: [],
    blocked_by: [],
  };
}

describe("useTaskActivity", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    taskApiMock.listTasks.mockResolvedValue([]);
    conversationApiMock.getAcpPermissionSnapshot.mockResolvedValue({
      pending: [],
    });
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
  });

  it("reconnects after the SSE reader closes without restarting a Task", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          `event: snapshot\ndata: ${JSON.stringify({
            type: "snapshot",
            tasks: [],
          })}\n\n`,
          { headers: { "Content-Type": "text/event-stream" } },
        ),
      ),
    );

    const { unmount } = renderHook(() =>
      useTaskActivity({
        open: true,
        target: { scope: "agent", agentId: "agent-a" },
        chatId: "chat-a",
        authorizationAgentId: "agent-a",
      }),
    );

    await waitFor(
      () => {
        expect(fetch).toHaveBeenCalledTimes(2);
      },
      { timeout: 2_500 },
    );
    expect(taskApiMock.listTasks).toHaveBeenCalledWith(
      { scope: "agent", agentId: "agent-a" },
      "chat-a",
      { limit: 200 },
    );
    expect(taskApiMock.retryTask).not.toHaveBeenCalled();
    unmount();
  });

  it("refreshes the authoritative Task record for lifecycle events", async () => {
    const updated = record();
    taskApiMock.getTask.mockResolvedValue(updated);
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          `event: task\ndata: ${JSON.stringify({
            task_id: "task-1",
            state: "running",
          })}\n\n`,
          { headers: { "Content-Type": "text/event-stream" } },
        ),
      ),
    );

    const { result, unmount } = renderHook(() =>
      useTaskActivity({
        open: true,
        target: { scope: "agent", agentId: "agent-a" },
        chatId: "chat-a",
        authorizationAgentId: "agent-a",
      }),
    );

    await waitFor(() =>
      expect(taskApiMock.getTask).toHaveBeenCalledWith(
        { scope: "agent", agentId: "agent-a" },
        "chat-a",
        "task-1",
      ),
    );
    await waitFor(() => expect(result.current.tasks).toEqual([updated]));
    unmount();
  });

  it("coalesces repeated events for one Task while its record is being read", async () => {
    let controller!: ReadableStreamDefaultController<Uint8Array>;
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          new ReadableStream({
            start(nextController) {
              controller = nextController;
            },
          }),
          { headers: { "Content-Type": "text/event-stream" } },
        ),
      ),
    );
    let resolveFirst!: (task: TaskRecord) => void;
    taskApiMock.getTask.mockReturnValueOnce(
      new Promise<TaskRecord>((resolve) => {
        resolveFirst = resolve;
      }),
    );
    const latest = { ...record(), state: "completed" as const };
    taskApiMock.getTask.mockResolvedValue(latest);
    const { result, unmount } = renderHook(() =>
      useTaskActivity({
        open: true,
        target: { scope: "agent", agentId: "agent-a" },
        chatId: "chat-a",
        authorizationAgentId: "agent-a",
      }),
    );
    try {
      await waitFor(() => expect(result.current.connection).toBe("connected"));
      await act(async () => {
        const events = Array.from(
          { length: 20 },
          (_, sequence) =>
            `event: task\ndata: ${JSON.stringify({
              task_id: "task-1",
              sequence,
            })}\n\n`,
        ).join("");
        controller.enqueue(new TextEncoder().encode(events));
      });
      expect(taskApiMock.getTask).toHaveBeenCalledTimes(1);
      await act(async () => resolveFirst(record()));
      await waitFor(() => expect(taskApiMock.getTask).toHaveBeenCalledTimes(2));
      await waitFor(() =>
        expect(result.current.tasks[0]?.state).toBe("completed"),
      );
    } finally {
      unmount();
      controller.close();
    }
  });

  it("does not schedule a dirty Task read after switching to another Chat", async () => {
    let controller!: ReadableStreamDefaultController<Uint8Array>;
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          new ReadableStream({
            start(value) {
              controller = value;
            },
          }),
          { headers: { "Content-Type": "text/event-stream" } },
        ),
      ),
    );
    let resolveTask!: (task: TaskRecord) => void;
    taskApiMock.getTask.mockReturnValueOnce(
      new Promise<TaskRecord>((resolve) => {
        resolveTask = resolve;
      }),
    );
    const { result, rerender, unmount } = renderHook(
      ({ chatId, open }) =>
        useTaskActivity({
          open,
          target: { scope: "agent", agentId: "agent-a" },
          chatId,
          authorizationAgentId: "agent-a",
        }),
      { initialProps: { chatId: "chat-a", open: true } },
    );
    try {
      await waitFor(() => expect(result.current.connection).toBe("connected"));
      await act(async () =>
        controller.enqueue(
          new TextEncoder().encode(
            'event: task\ndata: {"task_id":"task-1"}\n\nevent: task\ndata: {"task_id":"task-1"}\n\n',
          ),
        ),
      );
      expect(taskApiMock.getTask).toHaveBeenCalledTimes(1);
      rerender({ chatId: "chat-b", open: false });
      await act(async () => resolveTask(record()));
      expect(taskApiMock.getTask).toHaveBeenCalledTimes(1);
      expect(result.current.tasks).toEqual([]);
    } finally {
      unmount();
      controller.close();
    }
  });

  it("preserves Task order without sorting unchanged creation timestamps", () => {
    const tasks = [
      { ...record(), task_id: "new", created_at: "2026-07-28T00:00:00Z" },
      record(),
    ];
    const updated = { ...record(), state: "completed" as const };
    const sort = vi.spyOn(Array.prototype, "sort");
    try {
      const next = upsertTask(tasks, updated);
      expect(sort).not.toHaveBeenCalled();
      expect(next.map((task) => task.task_id)).toEqual(["new", "task-1"]);
      expect(next[1].state).toBe("completed");
      expect(tasks[1].state).toBe("running");
    } finally {
      sort.mockRestore();
    }
    expect(
      upsertTask(tasks, { ...updated, created_at: "2026-07-29T00:00:00Z" })[0]
        .task_id,
    ).toBe("task-1");
  });

  it("uses the Project target for Task activity", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          `event: snapshot\ndata: ${JSON.stringify({
            type: "snapshot",
            tasks: [],
          })}\n\n`,
          { headers: { "Content-Type": "text/event-stream" } },
        ),
      ),
    );
    const target = {
      scope: "project" as const,
      projectId: "project-a",
    };

    const { unmount } = renderHook(() =>
      useTaskActivity({
        open: true,
        target,
        chatId: "chat-a",
        authorizationAgentId: "agent-a",
      }),
    );

    await waitFor(() =>
      expect(taskApiMock.listTasks).toHaveBeenCalledWith(target, "chat-a", {
        limit: 200,
      }),
    );
    expect(taskApiMock.activityPath).toHaveBeenCalledWith(target, "chat-a");
    unmount();
  });

  it("uses the current Agent to authorize a Project Task retry", async () => {
    const retried = record();
    taskApiMock.retryTask.mockResolvedValue(retried);
    const target = {
      scope: "project" as const,
      projectId: "project-a",
    };

    const { result } = renderHook(() =>
      useTaskActivity({
        open: false,
        target,
        chatId: "chat-a",
        authorizationAgentId: "agent-b",
      }),
    );

    await act(async () => {
      await result.current.retryTask("task-1");
    });

    expect(taskApiMock.retryTask).toHaveBeenCalledWith(
      target,
      "chat-a",
      "task-1",
      "agent-b",
    );
  });

  it("closes activity while hidden and refreshes before reconnecting", async () => {
    const activitySignals: AbortSignal[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((_url: RequestInfo | URL, init?: RequestInit) => {
        const signal = init?.signal as AbortSignal;
        activitySignals.push(signal);
        let controller: ReadableStreamDefaultController<Uint8Array>;
        const body = new ReadableStream<Uint8Array>({
          start(nextController) {
            controller = nextController;
            signal.addEventListener(
              "abort",
              () => controller.error(new DOMException("Aborted", "AbortError")),
              { once: true },
            );
          },
        });
        return Promise.resolve(
          new Response(body, {
            headers: { "Content-Type": "text/event-stream" },
          }),
        );
      }),
    );

    const { unmount } = renderHook(() =>
      useTaskActivity({
        open: true,
        target: { scope: "agent", agentId: "agent-a" },
        chatId: "chat-a",
        authorizationAgentId: "agent-a",
      }),
    );
    await waitFor(() => expect(activitySignals).toHaveLength(1));

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    });
    act(() => document.dispatchEvent(new Event("visibilitychange")));

    expect(activitySignals[0].aborted).toBe(true);

    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    });
    act(() => document.dispatchEvent(new Event("visibilitychange")));

    await waitFor(() => expect(activitySignals).toHaveLength(2));
    expect(taskApiMock.listTasks).toHaveBeenCalledTimes(2);
    unmount();
  });
});
