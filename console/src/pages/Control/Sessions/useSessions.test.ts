import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Session } from "./components/constants";
import { useSessions } from "./useSessions";
import { projectChatsApi } from "@/api/modules/projectChats";
import { chatApi } from "@/api/modules/chat";
import { useConversationGroup } from "@/layouts/ConversationNavigation/useConversationGroups";
import { subscribeConversationNavigation } from "@/layouts/conversationNavigationEvents";

vi.mock("@/api/modules/chat", () => ({
  chatApi: { listChats: vi.fn() },
}));

vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: {
    listChats: vi.fn(),
    updateChat: vi.fn(),
    deleteChat: vi.fn(),
  },
}));

const {
  selectedAgentRef,
  listSessionsMock,
  updateSessionMock,
  deleteSessionMock,
  batchDeleteSessionsMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  selectedAgentRef: { current: "agent-1" },
  listSessionsMock: vi.fn(),
  updateSessionMock: vi.fn(),
  deleteSessionMock: vi.fn(),
  batchDeleteSessionsMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
  },
  tMock: vi.fn((key: string, params?: { count?: number }) =>
    params?.count == null ? key : `${key}:${params.count}`,
  ),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: selectedAgentRef.current }),
}));

vi.mock("../../../api", () => ({
  default: {
    listChats: listSessionsMock,
    updateChat: updateSessionMock,
    deleteChat: deleteSessionMock,
    batchDeleteChats: batchDeleteSessionsMock,
  },
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
}));

const session = (id: string, pinned = false): Session => ({
  id,
  user_id: `user-${id}`,
  channel: "console",
  name: `Session ${id}`,
  created_at: "2026-06-18T00:00:00Z",
  updated_at: "2026-06-18T00:00:00Z",
  status: "idle",
  pinned,
  meta: {},
});

describe("useSessions", () => {
  it.each(["agent", "project"] as const)(
    "does not invalidate navigation when %s deletion fails",
    async (kind) => {
      vi.mocked(projectChatsApi.listChats).mockResolvedValue({
        data: [],
        total: 0,
      });
      deleteSessionMock.mockRejectedValue(new Error("denied"));
      batchDeleteSessionsMock.mockRejectedValue(new Error("denied"));
      vi.mocked(projectChatsApi.deleteChat).mockRejectedValue(
        new Error("denied"),
      );
      const { result } = renderHook(() =>
        useSessions(undefined, kind === "project" ? "project-1" : undefined),
      );
      await waitFor(() => expect(result.current.loading).toBe(false));
      const listener = vi.fn();
      const unsubscribe = subscribeConversationNavigation(listener);
      try {
        await act(async () => {
          expect(await result.current.deleteSession("a")).toBe(false);
          expect(await result.current.batchDeleteSessions(["a", "b"])).toBe(
            false,
          );
        });
        expect(listener).not.toHaveBeenCalled();
      } finally {
        unsubscribe();
      }
    },
  );
  it.each(["agent", "project"] as const)(
    "refreshes a mounted %s sidebar after history deletion without reloading other scopes",
    async (kind) => {
      let chats = [session("a"), session("b")];
      const list = vi.fn(async () => ({ data: chats, total: chats.length }));
      const remove = async () => {
        chats = chats.filter((chat) => chat.id !== "a");
      };
      listSessionsMock.mockImplementation(list);
      vi.mocked(chatApi.listChats).mockImplementation(
        async (_params, agentId) =>
          agentId === "other"
            ? { data: [session("unrelated")], total: 1 }
            : list(),
      );
      vi.mocked(projectChatsApi.listChats).mockImplementation(list);
      deleteSessionMock.mockImplementation(remove);
      vi.mocked(projectChatsApi.deleteChat).mockImplementation(async () => {
        await remove();
        return { deleted: true, chat_id: "a", was_busy: false };
      });
      const { result } = renderHook(() => ({
        history: useSessions(
          undefined,
          kind === "project" ? "project-1" : undefined,
        ),
        sidebar: useConversationGroup(
          { kind, id: kind === "agent" ? "agent-1" : "project-1" },
          true,
        ),
        other: useConversationGroup({ kind: "agent", id: "other" }, true),
      }));
      await waitFor(() => expect(result.current.sidebar.chats).toHaveLength(2));
      await waitFor(() => expect(result.current.other.chats).toHaveLength(1));
      const otherCalls = () =>
        vi
          .mocked(chatApi.listChats)
          .mock.calls.filter(([, id]) => id === "other").length;
      const before = otherCalls();
      await act(async () => {
        expect(await result.current.history.deleteSession("a")).toBe(true);
      });
      await waitFor(() =>
        expect(result.current.sidebar.chats.map((chat) => chat.id)).toEqual([
          "b",
        ]),
      );
      expect(otherCalls()).toBe(before);
    },
  );

  it.each(["agent", "project"] as const)(
    "refreshes a collapsed %s sidebar after history batch deletion",
    async (kind) => {
      let chats = [session("a"), session("b")];
      const list = async () => ({ data: chats, total: chats.length });
      listSessionsMock.mockImplementation(list);
      vi.mocked(chatApi.listChats).mockImplementation(list);
      vi.mocked(projectChatsApi.listChats).mockImplementation(list);
      batchDeleteSessionsMock.mockImplementation(async () => {
        chats = [];
        return { deleted_count: 2 };
      });
      vi.mocked(projectChatsApi.deleteChat).mockImplementation(
        async (_project, id) => {
          if (id === "b") throw new Error("busy");
          chats = chats.filter((chat) => chat.id !== id);
          return { deleted: true, chat_id: id, was_busy: false };
        },
      );
      const { result, rerender } = renderHook(
        ({ expanded }) => ({
          history: useSessions(
            undefined,
            kind === "project" ? "project-1" : undefined,
          ),
          sidebar: useConversationGroup(
            { kind, id: kind === "agent" ? "agent-1" : "project-1" },
            expanded,
          ),
        }),
        { initialProps: { expanded: true } },
      );
      await waitFor(() => expect(result.current.sidebar.chats).toHaveLength(2));
      rerender({ expanded: false });
      await act(async () => {
        expect(
          await result.current.history.batchDeleteSessions(["a", "b"]),
        ).toBe(kind === "agent");
      });
      rerender({ expanded: true });
      await waitFor(() =>
        expect(result.current.sidebar.chats.map((chat) => chat.id)).toEqual(
          kind === "agent" ? [] : ["b"],
        ),
      );
    },
  );

  it("loads and mutates Project chats without using the Agent namespace", async () => {
    vi.mocked(projectChatsApi.listChats).mockResolvedValue({
      data: [session("p")],
      total: 21,
    });
    vi.mocked(projectChatsApi.updateChat).mockResolvedValue(session("p"));
    vi.mocked(projectChatsApi.deleteChat).mockResolvedValue({
      deleted: true,
      chat_id: "p",
      was_busy: false,
    });
    const seed = { agentId: "agent-1", sessions: [session("seed")], total: 1 };
    const { result } = renderHook(() => useSessions(seed, "project-1"));
    await waitFor(() => expect(result.current.sessions[0]?.id).toBe("p"));
    expect(listSessionsMock).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.fetchSessions(2);
    });
    expect(projectChatsApi.listChats).toHaveBeenLastCalledWith("project-1", {
      offset: 10,
      limit: 10,
    });
    await act(async () => {
      await result.current.updateSession("p", { name: "New" });
    });
    expect(projectChatsApi.updateChat).toHaveBeenCalledWith("project-1", "p", {
      name: "New",
    });
    await act(async () => {
      await result.current.deleteSession("p");
    });
    expect(projectChatsApi.deleteChat).toHaveBeenCalledWith("project-1", "p");
    expect(updateSessionMock).not.toHaveBeenCalled();
    expect(deleteSessionMock).not.toHaveBeenCalled();
  });

  it("refreshes after partially failed Project batch deletion", async () => {
    vi.mocked(projectChatsApi.listChats).mockResolvedValue({
      data: [session("p")],
      total: 1,
    });
    vi.mocked(projectChatsApi.deleteChat)
      .mockResolvedValueOnce({ deleted: true, chat_id: "a", was_busy: false })
      .mockRejectedValueOnce(new Error("busy"));
    const { result } = renderHook(() => useSessions(undefined, "project-1"));
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => {
      expect(await result.current.batchDeleteSessions(["a", "b"])).toBe(false);
    });
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(2);
    expect(batchDeleteSessionsMock).not.toHaveBeenCalled();
    expect(toastMock.error).toHaveBeenCalledWith("sessions.batchDeleteFailed");
  });

  it("ignores a late response from a previous scope", async () => {
    let resolveOld!: (value: { data: Session[]; total: number }) => void;
    vi.mocked(projectChatsApi.listChats).mockReturnValueOnce(
      new Promise((resolve) => {
        resolveOld = resolve;
      }),
    );
    const { result, rerender } = renderHook(
      ({ projectId }: { projectId?: string }) =>
        useSessions(undefined, projectId),
      { initialProps: { projectId: "project-1" } as { projectId?: string } },
    );
    rerender({ projectId: undefined });
    await waitFor(() => expect(result.current.sessions[0]?.id).toBe("a"));
    await act(async () => {
      resolveOld({ data: [session("stale")], total: 99 });
    });
    expect(result.current.sessions[0]?.id).toBe("a");
    expect(result.current.total).toBe(12);
  });

  it("does not reuse loader data from another Agent", async () => {
    const seed = { agentId: "other", sessions: [session("stale")], total: 1 };
    const { result } = renderHook(() => useSessions(seed));
    await waitFor(() => expect(result.current.sessions[0]?.id).toBe("a"));
    expect(listSessionsMock).toHaveBeenCalledWith(
      { offset: 0, limit: 10 },
      "agent-1",
    );
  });
  beforeEach(() => {
    vi.clearAllMocks();
    selectedAgentRef.current = "agent-1";
    listSessionsMock.mockResolvedValue({
      data: [session("a", true), session("b"), session("c")],
      total: 12,
    });
    updateSessionMock.mockImplementation((id: string, values: unknown) =>
      Promise.resolve({ ...session(id), ...(values as object) }),
    );
    deleteSessionMock.mockResolvedValue({});
    batchDeleteSessionsMock.mockResolvedValue({});
  });

  it("loads the first page through the selected Rust agent scope and derives unpinned total", async () => {
    const { result } = renderHook(() => useSessions());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listSessionsMock).toHaveBeenCalledWith(
      { offset: 0, limit: 10 },
      "agent-1",
    );
    expect(result.current.sessions.map((item) => item.id)).toEqual([
      "a",
      "b",
      "c",
    ]);
    expect(result.current.total).toBe(12);
    expect(result.current.unpinnedTotal).toBe(11);
    expect(result.current.page).toBe(1);
  });

  it("fetches requested pages with Rust offset/limit semantics", async () => {
    const { result } = renderHook(() => useSessions());
    await waitFor(() => expect(result.current.loading).toBe(false));

    listSessionsMock.mockResolvedValueOnce({
      data: [session("page-3")],
      total: 21,
    });
    await act(async () => {
      await result.current.fetchSessions(3);
    });

    expect(listSessionsMock).toHaveBeenLastCalledWith(
      { offset: 20, limit: 10 },
      "agent-1",
    );
    expect(result.current.page).toBe(3);
    expect(result.current.sessions.map((item) => item.id)).toEqual(["page-3"]);
  });

  it("updates locally, then reloads the current page after successful delete mutations", async () => {
    const { result } = renderHook(() => useSessions());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await expect(
        result.current.updateSession("b", { name: "Renamed" }),
      ).resolves.toBe(true);
    });
    expect(updateSessionMock).toHaveBeenCalledWith(
      "b",
      { name: "Renamed" },
      "agent-1",
    );
    expect(result.current.sessions.find((item) => item.id === "b")?.name).toBe(
      "Renamed",
    );
    expect(toastMock.success).toHaveBeenCalledWith("sessions.saveSuccess");

    listSessionsMock.mockResolvedValueOnce({
      data: [session("b"), session("c"), session("d")],
      total: 11,
    });
    await act(async () => {
      await expect(result.current.deleteSession("a")).resolves.toBe(true);
    });
    expect(deleteSessionMock).toHaveBeenCalledWith("a", "agent-1");
    expect(listSessionsMock).toHaveBeenLastCalledWith(
      { offset: 0, limit: 10 },
      "agent-1",
    );
    expect(result.current.sessions.map((item) => item.id)).toEqual([
      "b",
      "c",
      "d",
    ]);
    expect(result.current.total).toBe(11);

    listSessionsMock.mockResolvedValueOnce({
      data: [session("d")],
      total: 9,
    });
    await act(async () => {
      await expect(
        result.current.batchDeleteSessions(["b", "c"]),
      ).resolves.toBe(true);
    });
    expect(batchDeleteSessionsMock).toHaveBeenCalledWith(["b", "c"], "agent-1");
    expect(listSessionsMock).toHaveBeenLastCalledWith(
      { offset: 0, limit: 10 },
      "agent-1",
    );
    expect(result.current.sessions.map((item) => item.id)).toEqual(["d"]);
    expect(result.current.total).toBe(9);
    expect(toastMock.success).toHaveBeenCalledWith(
      "sessions.batchDeleteSuccess:2",
    );
  });

  it("reloads the previous page when deleting empties the current last page", async () => {
    const { result } = renderHook(() => useSessions());
    await waitFor(() => expect(result.current.loading).toBe(false));

    listSessionsMock.mockResolvedValueOnce({
      data: [session("page-3")],
      total: 21,
    });
    await act(async () => {
      await result.current.fetchSessions(3);
    });
    expect(result.current.page).toBe(3);

    listSessionsMock.mockResolvedValueOnce({
      data: [session("page-2-a"), session("page-2-b")],
      total: 20,
    });
    await act(async () => {
      await expect(result.current.deleteSession("page-3")).resolves.toBe(true);
    });

    expect(listSessionsMock).toHaveBeenLastCalledWith(
      { offset: 10, limit: 10 },
      "agent-1",
    );
    expect(result.current.page).toBe(2);
    expect(result.current.sessions.map((item) => item.id)).toEqual([
      "page-2-a",
      "page-2-b",
    ]);
  });

  it("clears list state on load failure without mutating via failed delete", async () => {
    listSessionsMock.mockRejectedValueOnce(new Error("down"));
    const { result } = renderHook(() => useSessions());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.sessions).toEqual([]);
    expect(result.current.total).toBe(0);
    expect(result.current.page).toBe(1);

    deleteSessionMock.mockRejectedValueOnce(new Error("delete failed"));
    await act(async () => {
      await expect(result.current.deleteSession("missing")).resolves.toBe(
        false,
      );
    });

    expect(toastMock.error).toHaveBeenCalledWith("sessions.deleteFailed");
    expect(result.current.sessions).toEqual([]);
    expect(result.current.total).toBe(0);
  });
});
