import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { chatApi } from "@/api/modules/chat";
import { projectChatsApi } from "@/api/modules/projectChats";
import { projectsApi } from "@/api/modules/projects";
import type { ChatSpec } from "@/api/types";
import { useAgentStore } from "@/stores/agentStore";
import {
  loadDraftChatId,
  resolveOrCreateDraftChatId,
} from "@/pages/Chat/chatNavigation";
import {
  createProjectChat,
  projectChatExecutionAgent,
  setProjectLastSelectedAgent,
} from "@/pages/Projects/projectChatNavigation";
import {
  beginConversationMetadataUpdate,
  invalidateConversationNavigation,
  registerNewConversationAction,
} from "../conversationNavigationEvents";
import { loadDraftConversationId } from "@/pages/Chat/conversationLifecycle";
import ConversationNavigation from "./index";
import { registerProjectNavigationAgent } from "../conversationNavigationAgent";

vi.mock("@/api/modules/chat", () => ({
  chatApi: {
    listChats: vi.fn(),
    searchChats: vi.fn(),
    updateChat: vi.fn(),
    getMetadata: vi.fn(),
  },
}));
vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: {
    listChats: vi.fn(),
    searchChats: vi.fn(),
    updateChat: vi.fn(),
    getMetadata: vi.fn(),
  },
}));
vi.mock("@/api/modules/projects", () => ({
  projectsApi: { list: vi.fn(), getSidebar: vi.fn(), updateSidebar: vi.fn() },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, string>) => {
      const value =
        (
          {
            "navigation.conversations": "Conversations",
            "navigation.independentChats": "Independent chats",
            "navigation.newConversation": "New chat",
            "navigation.search": "Search chats",
            "navigation.searchScope": "Search scope",
            "navigation.newInGroup": "New in {{name}}",
            "navigation.groupActions": "Actions for {{name}}",
            "navigation.conversationActions": "Actions for {{name}}",
            "navigation.manageInGroup": "Manage {{name}}",
            "navigation.currentConversation": "Current conversation",
            "navigation.loadFailed": "Unable to load",
            "navigation.manageProjects": "Manage projects",
            "navigation.projectActions": "Project actions",
            "navigation.noConversations": "No chats yet",
            "projects.chat.loadMore": "Load more",
            "projects.archived": "Archived",
            "common.retry": "Retry",
            "common.loading": "Loading",
          } as Record<string, string>
        )[key] || key;
      return value.replace(
        /{{(\w+)}}/g,
        (_, name: string) => options?.[name] || name,
      );
    },
  }),
}));

function agent(id: string) {
  return {
    id,
    name: `Agent ${id}`,
    description: "",
    workspace_dir: "",
    enabled: true,
  };
}
function project(id: string) {
  return {
    project_id: id,
    display_name: `Project ${id}`,
    resource_device_id: "local",
    display_path: `/${id}`,
    lifecycle: "active" as const,
    created_at: "",
    updated_at: "",
  };
}
function chat(id: string, name = id, pinned = false): ChatSpec {
  return {
    id,
    name,
    pinned,
    user_id: "",
    channel: "console",
    created_at: null,
    updated_at: null,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => {
    resolve = yes;
  });
  return { promise, resolve };
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  sessionStorage.clear();
  useAgentStore.setState({
    selectedAgent: "alpha",
    agents: [agent("alpha"), agent("beta")],
    catalogLoaded: true,
    catalogLoading: false,
    catalogError: null,
  });
  vi.mocked(projectsApi.updateSidebar).mockImplementation(
    async (_revision, action) => ({
      revision: "saved",
      sort: action.action === "set_sort" ? action.sort : "last_activity_at",
      items: {},
    }),
  );
  vi.mocked(projectsApi.getSidebar).mockResolvedValue({
    revision: "empty",
    sort: "last_activity_at",
    items: {},
  });
  vi.mocked(projectsApi.list).mockResolvedValue([
    project("one"),
    project("two"),
  ]);
  vi.mocked(chatApi.listChats).mockImplementation(async (_params, id) => ({
    data: [chat("same", `${id} chat`)],
    total: 1,
  }));
  vi.mocked(projectChatsApi.listChats).mockImplementation(async (id) => ({
    data: [chat("same", `${id} chat`)],
    total: 1,
  }));
  vi.mocked(chatApi.getMetadata).mockRejectedValue(new Error("Chat not found"));
  vi.mocked(projectChatsApi.getMetadata).mockRejectedValue(
    new Error("Chat not found"),
  );
  vi.mocked(chatApi.searchChats).mockResolvedValue({ results: [] });
  vi.mocked(projectChatsApi.searchChats).mockResolvedValue({ results: [] });
});

describe("ConversationNavigation", () => {
  it("refreshes projects received from another device when returning to the foreground", async () => {
    vi.mocked(projectsApi.list).mockResolvedValue([]);
    render(
      <ConversationNavigation
        location={{ pathname: "/agents/alpha/chats/same", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await waitFor(() => expect(projectsApi.list).toHaveBeenCalledTimes(1));
    vi.mocked(projectsApi.list).mockResolvedValue([project("replicated")]);
    act(() => window.dispatchEvent(new Event("rua:foreground")));
    expect(
      await screen.findByRole("button", { name: "Project replicated", exact: true }),
    ).toBeInTheDocument();
  });

  it("refreshes projects received while the visible navigation stays mounted", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      vi.mocked(projectsApi.list).mockResolvedValue([]);
      const view = render(
        <ConversationNavigation
          location={{ pathname: "/agents/alpha/chats/same", search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      await waitFor(() => expect(projectsApi.list).toHaveBeenCalledTimes(1));
      vi.mocked(projectsApi.list).mockResolvedValue([project("replicated")]);
      act(() => vi.advanceTimersByTime(10000));
      expect(
        await screen.findByRole("button", { name: "Project replicated", exact: true }),
      ).toBeInTheDocument();
      view.unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it("coalesces foreground and polling refreshes while a project request is pending", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const pending = deferred<ReturnType<typeof project>[]>();
      vi.mocked(projectsApi.list).mockReturnValueOnce(pending.promise);
      const view = render(
        <ConversationNavigation
          location={{ pathname: "/agents/alpha/chats/same", search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      await waitFor(() => expect(projectsApi.list).toHaveBeenCalledTimes(1));
      act(() => {
        vi.advanceTimersByTime(30000);
        window.dispatchEvent(new Event("rua:foreground"));
        window.dispatchEvent(new Event("focus"));
      });
      expect(projectsApi.list).toHaveBeenCalledTimes(1);
      await act(async () => pending.resolve([project("one")]));
      await waitFor(() => expect(projectsApi.list).toHaveBeenCalledTimes(2));
      view.unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it("refreshes conversations replicated into an already expanded project", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      vi.mocked(projectChatsApi.listChats).mockResolvedValue({ data: [], total: 0 });
      const view = render(
        <ConversationNavigation
          location={{ pathname: "/projects/one/chats/same", search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      await waitFor(() => expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1));
      vi.mocked(projectChatsApi.listChats).mockResolvedValue({
        data: [chat("remote", "Synced conversation")], total: 1,
      });
      act(() => vi.advanceTimersByTime(10000));
      expect(await screen.findByRole("button", { name: "Synced conversation", exact: true })).toBeInTheDocument();
      view.unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it.each(["/projects/one/chats/same", "/agents/alpha/chats/same"])(
    "keeps independent chats collapsed across More routes and remounts from %s",
    async (pathname) => {
      const onNavigate = vi.fn();
      const view = render(
        <ConversationNavigation
          location={{ pathname, search: "" }}
          onNavigate={onNavigate}
        />,
      );
      await screen.findByRole("button", {
        name: pathname.startsWith("/projects") ? "one chat" : "alpha chat",
      });
      await screen.findByRole("button", { name: "Project one", exact: true });
      fireEvent.click(
        screen.getByRole("button", { name: "Independent chats" }),
      );
      const expectCollapsed = () =>
        expect(
          screen.getByRole("button", { name: "Independent chats" }),
        ).toHaveAttribute("aria-expanded", "false");
      expectCollapsed();
      view.rerender(
        <ConversationNavigation
          location={{ pathname: "/settings", search: "" }}
          onNavigate={onNavigate}
        />,
      );
      expectCollapsed();
      view.rerender(
        <ConversationNavigation
          location={{ pathname, search: "" }}
          onNavigate={onNavigate}
        />,
      );
      expectCollapsed();
      view.unmount();
      render(
        <ConversationNavigation
          location={{ pathname, search: "" }}
          onNavigate={onNavigate}
        />,
      );
      await screen.findByRole("button", { name: "Project one", exact: true });
      await act(async () => {});
      expectCollapsed();
    },
  );

  it("reveals a newly opened conversation but not the same conversation after More", async () => {
    const view = render(
      <ConversationNavigation
        location={{ pathname: "/agents/alpha/chats/same", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByRole("button", { name: "alpha chat" });
    fireEvent.click(screen.getByRole("button", { name: "Independent chats" }));
    view.rerender(
      <ConversationNavigation
        location={{ pathname: "/agents/alpha/chats/another", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    expect(
      screen.getByRole("button", { name: "Independent chats" }),
    ).toHaveAttribute("aria-expanded", "true");
    await act(async () => {});
  });

  it("retains the current archived row while refreshing its loaded group", async () => {
    vi.mocked(projectChatsApi.getMetadata).mockResolvedValue({
      ...chat("archive", "Open archive"),
      archived: true,
    });
    render(
      <ConversationNavigation
        location={{ pathname: "/projects/one/chats/archive", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    const archivedRow = await screen.findByRole("button", {
      name: /^Open archive/,
    });
    const pending = deferred<{ data: ChatSpec[]; total: number }>();
    vi.mocked(projectChatsApi.listChats).mockReturnValue(pending.promise);
    act(() =>
      invalidateConversationNavigation({
        scope: "project",
        projectId: "one",
        chatId: "same",
      }),
    );
    expect(screen.getByRole("button", { name: /^Open archive/ })).toBe(
      archivedRow,
    );
    await act(async () =>
      pending.resolve({ data: [chat("same", "one chat")], total: 1 }),
    );
    expect(screen.getByRole("button", { name: /^Open archive/ })).toBe(
      archivedRow,
    );
  });

  it("keeps loaded rows stable while a committed archive refreshes in the background", async () => {
    const archived = chat("archive", "Archive me");
    const survivor = chat("same", "Keep me");
    vi.mocked(projectChatsApi.listChats).mockResolvedValue({
      data: [archived, survivor],
      total: 2,
    });
    render(
      <ConversationNavigation
        location={{ pathname: "/projects/one/chats/same", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    const row = await screen.findByRole("button", { name: "Keep me" });
    await waitFor(() =>
      expect(screen.queryByText("Loading")).not.toBeInTheDocument(),
    );
    const listRefresh = deferred<{ data: ChatSpec[]; total: number }>();
    const projectRefresh = deferred<ReturnType<typeof project>[]>();
    vi.mocked(projectChatsApi.listChats).mockReturnValue(listRefresh.promise);
    vi.mocked(projectsApi.list)
      .mockClear()
      .mockReturnValue(projectRefresh.promise);
    act(() => {
      const mutation = beginConversationMetadataUpdate(
        { scope: "project", projectId: "one", chatId: "archive" },
        archived,
        { archived: true },
      );
      mutation.commit({ ...archived, archived: true });
    });
    expect(
      screen.queryByRole("button", { name: "Archive me" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Keep me" })).toBe(row);
    expect(screen.queryByText("Loading")).not.toBeInTheDocument();
    await waitFor(() => expect(projectsApi.list).toHaveBeenCalledTimes(1));
    expect(screen.queryByText("Loading")).not.toBeInTheDocument();
    await act(async () => {
      listRefresh.resolve({ data: [survivor], total: 1 });
      projectRefresh.resolve([project("one"), project("two")]);
    });
    expect(screen.getByRole("button", { name: "Keep me" })).toBe(row);
  });

  it("loads only expanded scopes and navigates directly using the full address", async () => {
    const onNavigate = vi.fn();
    const { rerender } = render(
      <ConversationNavigation
        location={{ pathname: "/agents/alpha/chats/same", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    const activeChat = await screen.findByRole("button", {
      name: "alpha chat",
    });
    expect(activeChat).toHaveAttribute("aria-current", "page");
    expect(chatApi.listChats).toHaveBeenCalledTimes(1);
    expect(chatApi.listChats).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 0, limit: 5, view: "user" }),
      "alpha",
    );
    expect(projectChatsApi.listChats).not.toHaveBeenCalled();
    fireEvent.click(
      await screen.findByRole("button", { name: "Project one", exact: true }),
    );
    expect(onNavigate).not.toHaveBeenCalled();
    expect(projectChatsApi.listChats).toHaveBeenCalledWith(
      "one",
      expect.objectContaining({ offset: 0, limit: 5, view: "user" }),
    );
    fireEvent.click(await screen.findByRole("button", { name: "one chat" }));
    expect(onNavigate).toHaveBeenLastCalledWith("/projects/one/chats/same");
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);
    rerender(
      <ConversationNavigation
        location={{ pathname: "/projects/one/chats/same", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    expect(screen.getByRole("button", { name: "one chat" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(
      screen.queryByRole("button", { name: "alpha chat" }),
    ).not.toBeInTheDocument();
    rerender(
      <ConversationNavigation
        location={{ pathname: "/agents/beta/chats/same", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "Agent beta", exact: true }),
    ).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "beta chat" }));
    expect(onNavigate).toHaveBeenLastCalledWith("/agents/beta/chats/same");
  });

  it("follows the visible Project Agent without changing the standalone Agent preference", async () => {
    const onNavigate = vi.fn();
    const releaseA = registerProjectNavigationAgent({
      projectId: "one",
      chatId: "same",
      agentId: "beta",
    });
    const view = render(
      <ConversationNavigation
        location={{ pathname: "/projects/one/chats/same", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    await screen.findByRole("button", { name: "beta chat" });
    expect(
      screen.queryByRole("button", { name: "alpha chat" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Agent beta", exact: true }),
    ).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "New in Independent chats" }),
    );
    expect(onNavigate.mock.lastCall?.[0]).toMatch(/^\/agents\/beta\/chats\//);
    let releaseB!: () => void;
    act(() => {
      releaseB = registerProjectNavigationAgent({
        projectId: "one",
        chatId: "same",
        agentId: "alpha",
      });
    });
    await screen.findByRole("button", { name: "alpha chat" });
    expect(
      screen.queryByRole("button", { name: "beta chat" }),
    ).not.toBeInTheDocument();
    expect(useAgentStore.getState().selectedAgent).toBe("alpha");
    act(() => releaseA());
    expect(screen.getByRole("button", { name: "alpha chat" })).toBeVisible();
    view.rerender(
      <ConversationNavigation
        location={{ pathname: "/projects/two/chats/same", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "alpha chat" }),
    ).not.toBeInTheDocument();
    view.unmount();
    releaseB();
  });

  it("keeps the loaded standalone group through a same-Agent Project route handoff", async () => {
    const props = { onNavigate: vi.fn() };
    const view = render(
      <ConversationNavigation
        {...props}
        location={{ pathname: "/agents/alpha/chats/same", search: "" }}
      />,
    );
    const standaloneRow = await screen.findByRole("button", {
      name: "alpha chat",
    });
    const loadCount = vi.mocked(chatApi.listChats).mock.calls.length;
    view.rerender(
      <ConversationNavigation
        {...props}
        location={{ pathname: "/projects/one/chats/same", search: "" }}
      />,
    );
    expect(standaloneRow.isConnected).toBe(true);
    expect(standaloneRow).not.toBeVisible();
    let release!: () => void;
    act(() => {
      release = registerProjectNavigationAgent({
        projectId: "one",
        chatId: "same",
        agentId: "alpha",
      });
    });
    expect(screen.getByRole("button", { name: "alpha chat" })).toBe(
      standaloneRow,
    );
    expect(standaloneRow).toBeVisible();
    expect(chatApi.listChats).toHaveBeenCalledTimes(loadCount);
    view.unmount();
    release();
  });

  it("opens the current project without inventing a saved row absent from its active list", async () => {
    render(
      <ConversationNavigation
        location={{ pathname: "/projects/two/chats/beyond-page", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByRole("button", { name: "two chat" });
    expect(
      screen.getByRole("button", { name: "Project two", exact: true }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(
      screen.queryByRole("button", { name: "Current conversation" }),
    ).not.toBeInTheDocument();
    expect(chatApi.listChats).not.toHaveBeenCalled();
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);
  });

  it.each(["agent", "project"] as const)(
    "highlights only the open archived %s chat without adding it to the active list",
    async (kind) => {
      const api = kind === "project" ? projectChatsApi : chatApi;
      vi.mocked(api.getMetadata).mockResolvedValue({
        ...chat("archived-chat", "Saved archived chat"),
        archived: true,
      });
      const base =
        kind === "project" ? "/projects/one/chats" : "/agents/alpha/chats";
      const { container, rerender } = render(
        <ConversationNavigation
          location={{ pathname: `${base}/archived-chat`, search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      const active = await screen.findByRole("button", {
        name: /^Saved archived chat/,
      });
      expect(active).toHaveAttribute("aria-current", "page");
      expect(within(active).getByText("Archived")).toBeVisible();
      expect(container.querySelectorAll('[aria-current="page"]')).toHaveLength(
        1,
      );
      expect(api.listChats).toHaveBeenCalledTimes(1);
      expect(api.updateChat).not.toHaveBeenCalled();
      rerender(
        <ConversationNavigation
          location={{ pathname: `${base}/same`, search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      expect(screen.queryByText("Saved archived chat")).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", {
          name: kind === "project" ? "one chat" : "alpha chat",
        }),
      ).toHaveAttribute("aria-current", "page");
      expect(api.getMetadata).toHaveBeenCalledTimes(1);
    },
  );

  it.each(["agent", "project"] as const)(
    "delegates sidebar creation to the visible %s page without creating another draft",
    async (kind) => {
      const address =
        kind === "agent"
          ? { scope: "agent" as const, agentId: "alpha", chatId: "same" }
          : { scope: "project" as const, projectId: "one", chatId: "same" };
      const action = vi.fn();
      const unregister = registerNewConversationAction(address, action);
      const onNavigate = vi.fn();
      try {
        render(
          <ConversationNavigation
            location={{
              pathname:
                kind === "agent"
                  ? "/agents/alpha/chats/same"
                  : "/projects/one/chats/same",
              search: "",
            }}
            onNavigate={onNavigate}
          />,
        );
        await act(async () => {
          fireEvent.click(
            screen.getByRole("button", { name: "New chat", exact: true }),
          );
        });
        const draftId =
          kind === "agent"
            ? loadDraftChatId("alpha")
            : loadDraftConversationId("project:one");
        expect(action).toHaveBeenCalledOnce();
        expect(draftId).toBeNull();
        expect(onNavigate).not.toHaveBeenCalled();
      } finally {
        unregister();
      }
    },
  );

  it.each(["agent", "project"] as const)(
    "does not show late metadata from an archived %s chat after navigating away",
    async (kind) => {
      const api = kind === "project" ? projectChatsApi : chatApi;
      const response = deferred<ChatSpec>();
      vi.mocked(api.getMetadata).mockReturnValue(response.promise);
      const base =
        kind === "project" ? "/projects/one/chats" : "/agents/alpha/chats";
      const { rerender } = render(
        <ConversationNavigation
          location={{ pathname: `${base}/archived-chat`, search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      await waitFor(() => expect(api.getMetadata).toHaveBeenCalledOnce());
      const options = vi.mocked(api.getMetadata).mock.calls[0][2];
      rerender(
        <ConversationNavigation
          location={{ pathname: `${base}/same`, search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      expect(options?.signal?.aborted).toBe(true);
      await act(async () =>
        response.resolve({
          ...chat("archived-chat", "Stale archived chat"),
          archived: true,
        }),
      );
      expect(screen.queryByText("Stale archived chat")).not.toBeInTheDocument();
    },
  );

  it.each(["agent", "project"] as const)(
    "does not request metadata or invent a row for a lazy %s chat",
    async (kind) => {
      const api = kind === "project" ? projectChatsApi : chatApi;
      const chatId =
        kind === "project"
          ? createProjectChat("one")
          : resolveOrCreateDraftChatId("alpha");
      render(
        <ConversationNavigation
          location={{
            pathname: `${
              kind === "project" ? "/projects/one/chats" : "/agents/alpha/chats"
            }/${chatId}`,
            search: "",
          }}
          onNavigate={vi.fn()}
        />,
      );
      await screen.findByRole("button", {
        name: kind === "project" ? "one chat" : "alpha chat",
      });
      expect(api.getMetadata).not.toHaveBeenCalled();
      expect(screen.queryByText("Archived")).not.toBeInTheDocument();
    },
  );

  it.each(["agent", "project"] as const)(
    "leaves an empty %s group empty for missing or active metadata",
    async (kind) => {
      const api = kind === "project" ? projectChatsApi : chatApi;
      vi.mocked(api.listChats).mockResolvedValue({ data: [], total: 0 });
      const base =
        kind === "project" ? "/projects/one/chats" : "/agents/alpha/chats";
      const { container, rerender } = render(
        <ConversationNavigation
          location={{ pathname: `${base}/missing`, search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      await waitFor(() => expect(api.getMetadata).toHaveBeenCalledOnce());
      expect(
        screen
          .getAllByText("No chats yet")
          .filter((element) => !element.closest("[hidden]")),
      ).toHaveLength(1);
      expect(container.querySelector('[aria-current="page"]')).toBeNull();
      vi.mocked(api.getMetadata).mockResolvedValue({
        ...chat("active", "Unlisted active chat"),
        archived: false,
      });
      rerender(
        <ConversationNavigation
          location={{ pathname: `${base}/active`, search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      await waitFor(() => expect(api.getMetadata).toHaveBeenCalledTimes(2));
      expect(
        screen.queryByText("Unlisted active chat"),
      ).not.toBeInTheDocument();
      expect(
        screen
          .getAllByText("No chats yet")
          .filter((element) => !element.closest("[hidden]")),
      ).toHaveLength(1);
    },
  );

  it("does not overwrite a newer restore update with stale archive metadata", async () => {
    const snapshot = { ...chat("archive", "Archived chat"), archived: true };
    vi.mocked(projectChatsApi.getMetadata).mockResolvedValueOnce(snapshot);
    render(
      <ConversationNavigation
        location={{ pathname: "/projects/one/chats/archive", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByText("Archived chat");
    const response = deferred<ChatSpec>();
    vi.mocked(projectChatsApi.getMetadata).mockReturnValue(response.promise);
    const address = {
      scope: "project" as const,
      projectId: "one",
      chatId: "archive",
    };
    act(() => invalidateConversationNavigation(address));
    await waitFor(() =>
      expect(
        vi.mocked(projectChatsApi.getMetadata).mock.calls.length,
      ).toBeGreaterThan(1),
    );
    act(() => {
      beginConversationMetadataUpdate(address, snapshot, { archived: false });
    });
    await act(async () => response.resolve(snapshot));
    expect(screen.queryByText("Archived chat")).not.toBeInTheDocument();
  });

  it("renders the selected Agent and leaves current creation to its page, reusing other scoped drafts", async () => {
    const onNavigate = vi.fn();
    render(
      <ConversationNavigation
        location={{ pathname: "/agents/alpha/chats/same", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    await screen.findByRole("button", { name: "alpha chat" });
    expect(
      screen.queryByRole("button", { name: "Agent alpha", exact: true }),
    ).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "New chat", exact: true }),
    );
    const draftId = loadDraftChatId("alpha");
    expect(draftId).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "New chat", exact: true }),
    );
    expect(onNavigate).not.toHaveBeenCalled();
    expect(loadDraftChatId("alpha")).toBe(draftId);
    setProjectLastSelectedAgent("one", "beta");
    fireEvent.click(screen.getByRole("button", { name: "New in Project one" }));
    const firstProjectPath = onNavigate.mock.lastCall?.[0] as string;
    fireEvent.click(screen.getByRole("button", { name: "New in Project one" }));
    expect(onNavigate).toHaveBeenLastCalledWith(firstProjectPath);
    expect(
      projectChatExecutionAgent("one", firstProjectPath.split("/").pop()!),
    ).toBe("beta");
    expect(
      screen.queryByRole("button", { name: "Manage Agent alpha" }),
    ).not.toBeInTheDocument();
    const projectActions = screen.getByRole("button", {
      name: "Project actions",
    });
    expect(projectActions).toHaveTextContent("");
    fireEvent.click(projectActions);
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "Manage projects" }),
    );
    expect(onNavigate).toHaveBeenLastCalledWith("/projects?overview=1");
  });

  it.each(["agent", "project"] as const)(
    "paginates %s chats past repeated pins and hides Load more when complete",
    async (kind) => {
      const list =
        kind === "project" ? projectChatsApi.listChats : chatApi.listChats;
      vi.mocked(list)
        .mockResolvedValueOnce({
          data: [chat("pin", "Pinned chat", true), chat("first", "First chat")],
          total: 3,
        })
        .mockResolvedValueOnce({
          data: [chat("pin", "Pinned chat", true), chat("next", "Next chat")],
          total: 3,
        });
      render(
        <ConversationNavigation
          location={{
            pathname:
              kind === "project"
                ? "/projects/one/chats/first"
                : "/agents/alpha/chats/first",
            search: "",
          }}
          onNavigate={vi.fn()}
        />,
      );
      fireEvent.click(await screen.findByRole("button", { name: "Load more" }));
      await screen.findByRole("button", { name: "Next chat" });
      expect(list).toHaveBeenLastCalledWith(
        ...(kind === "project"
          ? ["one", expect.objectContaining({ offset: 1, limit: 20 })]
          : [expect.objectContaining({ offset: 1, limit: 20 }), "alpha"]),
      );
      expect(screen.getAllByTitle("Pinned chat")).toHaveLength(1);
      expect(
        screen.queryByRole("button", { name: "Load more" }),
      ).not.toBeInTheDocument();
    },
  );

  it.each(["agent", "project"] as const)(
    "omits Load more for a fully loaded %s list",
    async (kind) => {
      render(
        <ConversationNavigation
          location={{
            pathname:
              kind === "project"
                ? "/projects/one/chats/same"
                : "/agents/alpha/chats/same",
            search: "",
          }}
          onNavigate={vi.fn()}
        />,
      );
      await screen.findByRole("button", {
        name: kind === "project" ? "one chat" : "alpha chat",
      });
      expect(
        screen.queryByRole("button", { name: "Load more" }),
      ).not.toBeInTheDocument();
    },
  );

  it("resumes pagination after collapsing a group during an in-flight append", async () => {
    const interrupted = deferred<{ data: ChatSpec[]; total: number }>();
    vi.mocked(projectChatsApi.listChats)
      .mockResolvedValueOnce({ data: [chat("first", "First chat")], total: 2 })
      .mockReturnValueOnce(interrupted.promise)
      .mockResolvedValueOnce({ data: [chat("first", "First chat")], total: 2 })
      .mockResolvedValueOnce({ data: [chat("next", "Next chat")], total: 2 });
    render(
      <ConversationNavigation
        location={{ pathname: "/projects/one/chats/first", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Load more" }));
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenCalledTimes(2),
    );
    const group = screen.getByRole("button", {
      name: "Project one",
      exact: true,
    });
    fireEvent.click(group);
    fireEvent.click(group);
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenCalledTimes(3),
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Load more" })).toBeEnabled(),
    );
    await act(async () =>
      interrupted.resolve({
        data: [chat("stale", "Interrupted append")],
        total: 2,
      }),
    );
    expect(
      screen.queryByRole("button", { name: "Interrupted append" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    await screen.findByRole("button", { name: "Next chat" });
    expect(projectChatsApi.listChats).toHaveBeenLastCalledWith(
      "one",
      expect.objectContaining({ offset: 1 }),
    );
    expect(
      screen.queryByRole("button", { name: "Load more" }),
    ).not.toBeInTheDocument();
  });

  it("rejects an obsolete response after invalidation and reloads only the matching scope", async () => {
    const stale = deferred<{ data: ChatSpec[]; total: number }>();
    vi.mocked(projectChatsApi.listChats)
      .mockReturnValueOnce(stale.promise)
      .mockResolvedValue({ data: [chat("latest", "Fresh title")], total: 1 });
    render(
      <ConversationNavigation
        location={{ pathname: "/projects/one/chats/latest", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1),
    );
    act(() =>
      invalidateConversationNavigation({
        scope: "project",
        projectId: "two",
        chatId: "other",
      }),
    );
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);
    act(() =>
      invalidateConversationNavigation({
        scope: "project",
        projectId: "one",
        chatId: "latest",
      }),
    );
    await screen.findByRole("button", { name: "Fresh title" });
    await act(async () =>
      stale.resolve({ data: [chat("stale", "Stale title")], total: 1 }),
    );
    expect(
      screen.queryByRole("button", { name: "Stale title" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Fresh title" })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  it("keeps load failures visible and retries instead of reporting an empty list", async () => {
    vi.mocked(projectChatsApi.listChats)
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValue({ data: [chat("recovered", "Recovered")], total: 1 });
    render(
      <ConversationNavigation
        location={{ pathname: "/projects/one/chats/recovered", search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Unable to load");
    expect(
      screen
        .queryAllByText("No chats yet")
        .filter((element) => !element.closest("[hidden]")),
    ).toHaveLength(0);
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    await screen.findByRole("button", { name: "Recovered" });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("searches the selected complete scope and discards results from the previous scope", async () => {
    const oldSearch = deferred<{ results: [] }>();
    vi.mocked(chatApi.searchChats).mockReturnValueOnce(oldSearch.promise);
    vi.mocked(projectChatsApi.searchChats).mockResolvedValue({
      results: [
        {
          chat_id: "found/chat",
          chat_name: "Found in project",
          card_index: 4,
          role: "user",
          snippet: "Matching message",
        },
      ],
    });
    const onNavigate = vi.fn();
    render(
      <ConversationNavigation
        location={{ pathname: "/agents/alpha/chats/same", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    await screen.findByRole("button", { name: "alpha chat" });
    fireEvent.click(screen.getByRole("button", { name: "Search chats" }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Search chats" }), {
      target: { value: "needle" },
    });
    await waitFor(() =>
      expect(chatApi.searchChats).toHaveBeenCalledWith("needle", "alpha"),
    );
    const searchScope = screen.getByRole("combobox", { name: "Search scope" });
    expect(searchScope.tagName).not.toBe("SELECT");
    fireEvent.click(searchScope);
    fireEvent.click(await screen.findByRole("option", { name: "Project two" }));
    await screen.findByRole("button", { name: /Found in project/ });
    await act(async () => oldSearch.resolve({ results: [] }));
    fireEvent.click(screen.getByRole("button", { name: /Found in project/ }));
    expect(projectChatsApi.searchChats).toHaveBeenCalledWith("two", "needle");
    expect(onNavigate).toHaveBeenLastCalledWith(
      "/projects/two/chats/found%2Fchat",
    );
    expect(screen.queryByRole("searchbox")).not.toBeInTheDocument();
    expect(
      JSON.parse(sessionStorage.getItem("rua_search_scroll_target")!),
    ).toMatchObject({
      conversationId: "found/chat",
      ownerKey: "project:two",
      cardIndex: 4,
    });
  });

  it("does not present a lazy draft as a saved conversation", async () => {
    const draftId = resolveOrCreateDraftChatId("alpha");
    render(
      <ConversationNavigation
        location={{ pathname: `/agents/alpha/chats/${draftId}`, search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByRole("button", { name: "alpha chat" });
    expect(
      screen.queryByRole("button", { name: "Current conversation" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "alpha chat" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("changes the selected chat without scrolling the sidebar after crossing projects", async () => {
    const scrollIntoView = vi.fn();
    const originalScroll = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scrollIntoView;
    try {
      const props = { onNavigate: vi.fn() };
      const view = render(
        <ConversationNavigation
          {...props}
          location={{ pathname: "/projects/one/chats/same", search: "" }}
        />,
      );
      await screen.findByRole("button", { name: "one chat" });
      view.rerender(
        <ConversationNavigation
          {...props}
          location={{ pathname: "/projects/two/chats/same", search: "" }}
        />,
      );
      await screen.findByRole("button", { name: "two chat" });
      await act(async () => {
        await new Promise<void>((resolve) =>
          window.requestAnimationFrame(() => resolve()),
        );
      });
      expect(scrollIntoView).not.toHaveBeenCalled();
      expect(screen.getByRole("button", { name: "two chat" })).toHaveAttribute(
        "aria-current",
        "page",
      );
      expect(
        screen.getByRole("button", { name: "Project two", exact: true }),
      ).toHaveAttribute("aria-expanded", "true");
    } finally {
      HTMLElement.prototype.scrollIntoView = originalScroll;
    }
  });

  it("restores expanded groups on remount without fetching collapsed siblings", async () => {
    const props = {
      location: { pathname: "/agents/alpha/chats/same", search: "" },
      onNavigate: vi.fn(),
    };
    const view = render(<ConversationNavigation {...props} />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Project two", exact: true }),
    );
    await screen.findByRole("button", { name: "two chat" });
    view.unmount();
    vi.mocked(projectChatsApi.listChats).mockClear();
    render(<ConversationNavigation {...props} />);
    await screen.findByRole("button", { name: "two chat" });
    expect(
      screen.getByRole("button", { name: "Project two", exact: true }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);
    expect(projectChatsApi.listChats).toHaveBeenCalledWith(
      "two",
      expect.anything(),
    );
  });
});

it("moves a project with its drag handle, saves manual order and preserves the active route", async () => {
  const navigate = vi.fn();
  const next = {
    revision: "saved",
    sort: "manual" as const,
    items: {
      two: { project_id: "two", pinned: false, order: 0, pin_order: 0 },
      one: { project_id: "one", pinned: false, order: 1024, pin_order: 0 },
    },
  };
  vi.mocked(projectsApi.updateSidebar).mockImplementation(async () => {
    vi.mocked(projectsApi.getSidebar).mockResolvedValue(next);
    return next;
  });
  render(
    <ConversationNavigation
      location={{ pathname: "/projects/one/chats/same", search: "" }}
      onNavigate={navigate}
    />,
  );
  const first = await screen.findByRole("button", {
    name: "Project one",
    exact: true,
  });
  const second = await screen.findByRole("button", {
    name: "Project two",
    exact: true,
  });
  const handle = second;
  await waitFor(() => expect(handle).toHaveAttribute("draggable", "true"));
  const dataTransfer = {
    setData: vi.fn(),
    setDragImage: vi.fn(),
    effectAllowed: "",
  };
  fireEvent.dragStart(handle, { dataTransfer });
  fireEvent.dragOver(first.closest("section")!.parentElement!);
  fireEvent.drop(first.closest("section")!.parentElement!);
  await waitFor(() =>
    expect(projectsApi.updateSidebar).toHaveBeenCalledWith("empty", {
      action: "reorder",
      project_id: "two",
      ordered_project_ids: ["two", "one"],
    }),
  );
  await waitFor(() =>
    expect(
      second.compareDocumentPosition(first) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy(),
  );
  expect(navigate).not.toHaveBeenCalled();
});

it("pins through the project menu and leaves the current conversation open", async () => {
  const navigate = vi.fn();
  const next = {
    revision: "pinned",
    sort: "last_activity_at" as const,
    items: { two: { project_id: "two", pinned: true, order: 0, pin_order: 0 } },
  };
  vi.mocked(projectsApi.updateSidebar).mockImplementation(async () => {
    vi.mocked(projectsApi.getSidebar).mockResolvedValue(next);
    return next;
  });
  render(
    <ConversationNavigation
      location={{ pathname: "/projects/one/chats/same", search: "" }}
      onNavigate={navigate}
    />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "Actions for Project two" }),
  );
  fireEvent.click(
    await screen.findByRole("menuitem", { name: "navigation.pinProject" }),
  );
  await waitFor(() =>
    expect(projectsApi.updateSidebar).toHaveBeenCalledWith("empty", {
      action: "set_pinned",
      project_id: "two",
      pinned: true,
    }),
  );
  expect(await screen.findByText("navigation.pinnedProjects")).toBeVisible();
  expect(navigate).not.toHaveBeenCalled();
});

it("hides a project without navigating or changing its lifecycle", async () => {
  const navigate = vi.fn();
  const next = {
    revision: "hidden",
    sort: "manual" as const,
    items: {
      one: {
        project_id: "one",
        pinned: false,
        hidden: true,
        order: 0,
        pin_order: 0,
      },
    },
  };
  vi.mocked(projectsApi.updateSidebar).mockImplementation(async () => {
    vi.mocked(projectsApi.getSidebar).mockResolvedValue(next);
    return next;
  });
  render(
    <ConversationNavigation
      location={{ pathname: "/projects/one/chats/same", search: "" }}
      onNavigate={navigate}
    />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: "Actions for Project one" }),
  );
  fireEvent.click(
    await screen.findByRole("menuitem", { name: "navigation.hideProject" }),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Project one", exact: true }),
    ).not.toBeInTheDocument(),
  );
  expect(projectsApi.updateSidebar).toHaveBeenCalledWith("empty", {
    action: "set_hidden",
    project_id: "one",
    hidden: true,
  });
  expect(navigate).not.toHaveBeenCalled();
});

it("switches automatic order to manual when dragging into an empty section", async () => {
  const initial = {
    revision: "r1",
    sort: "name" as const,
    items: {},
    sections: { work: { section_id: "work", name: "Work", order: 0 } },
  };
  vi.mocked(projectsApi.getSidebar).mockResolvedValue(initial);
  vi.mocked(projectsApi.updateSidebar).mockImplementation(
    async (_revision, action) => {
      const next = {
        ...initial,
        revision: action.action === "move_to_section" ? "r2" : "r3",
        sort:
          action.action === "reorder" ? ("manual" as const) : ("name" as const),
        items: {
          one: {
            project_id: "one",
            pinned: false,
            order: 0,
            pin_order: 0,
            section_id: "work",
          },
        },
      };
      vi.mocked(projectsApi.getSidebar).mockResolvedValue(next);
      return next;
    },
  );
  render(
    <ConversationNavigation
      location={{ pathname: "/projects/one/chats/same", search: "" }}
      onNavigate={vi.fn()}
    />,
  );
  const project = await screen.findByRole("button", {
    name: "Project one",
    exact: true,
  });
  const handle = project;
  await waitFor(() => expect(handle).toHaveAttribute("draggable", "true"));
  fireEvent.dragStart(handle, {
    dataTransfer: {
      setData: vi.fn(),
      setDragImage: vi.fn(),
      effectAllowed: "",
    },
  });
  fireEvent.drop(screen.getByRole("region", { name: "Work" }));
  await waitFor(() =>
    expect(projectsApi.updateSidebar).toHaveBeenCalledWith("r2", {
      action: "reorder",
      project_id: "one",
      ordered_project_ids: ["two", "one"],
    }),
  );
});
