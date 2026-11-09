import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ChatSpec,
  ConversationBackgroundWorkView,
} from "@/api/types/chat";
import type { ConversationApiTarget } from "@/api/modules/conversation";
import { useCollaborationActivity } from "./useCollaborationActivity";

const api = vi.hoisted(() => ({
  agents: vi.fn(),
  chats: vi.fn(),
  projects: vi.fn(),
}));
vi.mock("@/api/modules/agents", () => ({
  agentsApi: { listAgents: api.agents },
}));
vi.mock("@/api/modules/chat", () => ({ chatApi: { listChats: api.chats } }));
vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: { listChats: api.projects },
}));
const target: ConversationApiTarget = {
  scope: "project",
  projectId: "project-a",
};
const child = (id: string, sourceId = "source-a"): ChatSpec => ({
  id,
  name: id,
  session_id: id,
  user_id: "user",
  channel: "console",
  created_at: null,
  updated_at: null,
  status: "idle",
  collaboration: {
    source: {
      scope: { kind: "project", project_id: "project-a" },
      chat_id: sourceId,
    },
    executor: { kind: "acp", runner: "codex" },
    executor_agent_id: "agent-a",
    configuration: {
      desired_source_epoch: "1",
      applied_source_epoch: "1",
      status: "ready",
    },
  },
});

describe("useCollaborationActivity", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    api.projects.mockResolvedValue({ data: [], total: 0 });
  });

  it("loads every page without overriding idle Chats with result delivery state", async () => {
    api.projects.mockImplementation(async (_project, params) => ({
      data: [child(params.offset === 0 ? "first" : "second")],
      total: 2,
    }));
    const pending: ConversationBackgroundWorkView = {
      suppressed: false,
      items: [
        {
          kind: "collaboration",
          id: "request-1",
          status: "result_ready",
          chat: {
            scope: { kind: "project", project_id: "project-a" },
            chat_id: "first",
          },
        },
      ],
    };
    const { result, rerender } = renderHook(
      ({ backgroundWork }) =>
        useCollaborationActivity({
          open: true,
          target,
          chatId: "source-a",
          backgroundWork,
        }),
      { initialProps: { backgroundWork: pending } },
    );
    await waitFor(() => expect(result.current.collaborations).toHaveLength(2));
    expect(api.projects).toHaveBeenCalledWith(
      "project-a",
      expect.objectContaining({ offset: 1 }),
    );
    expect(
      result.current.collaborations.find((row) => row.chat?.id === "first")
        ?.pending,
    ).toBe(false);
    rerender({ backgroundWork: { suppressed: false, items: [] } });
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.collaborations.every((row) => !row.pending)).toBe(
      true,
    );
    expect(
      result.current.collaborations.every((row) => row.status === "idle"),
    ).toBe(true);
  });

  it("does not leak a late response into another source chat", async () => {
    let finishOld!: (page: { data: ChatSpec[]; total: number }) => void;
    api.projects.mockImplementation(async (_project, params) =>
      params.source_chat_id === "source-a"
        ? new Promise((resolve) => {
            finishOld = resolve;
          })
        : { data: [child("new-child", "source-b")], total: 1 },
    );
    const { result, rerender } = renderHook(
      ({ chatId }) => useCollaborationActivity({ open: true, target, chatId }),
      { initialProps: { chatId: "source-a" } },
    );
    rerender({ chatId: "source-b" });
    await waitFor(() =>
      expect(result.current.collaborations[0]?.chat?.id).toBe("new-child"),
    );
    await act(async () => finishOld({ data: [child("old-child")], total: 1 }));
    expect(result.current.collaborations.map((row) => row.chat?.id)).toEqual([
      "new-child",
    ]);
  });

  it("refreshes only while open and clears its timer on close", async () => {
    vi.useFakeTimers();
    try {
      const { rerender, unmount } = renderHook(
        ({ open }) =>
          useCollaborationActivity({ open, target, chatId: "source-a" }),
        { initialProps: { open: true } },
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(api.projects).toHaveBeenCalledTimes(1);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      expect(api.projects).toHaveBeenCalledTimes(2);
      rerender({ open: false });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(10000);
      });
      expect(api.projects).toHaveBeenCalledTimes(2);
      unmount();
    } finally {
      vi.useRealTimers();
    }
  });
});
