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
import type { ChatSearchResponse, ChatSpec } from "@/api/types";
import { useAgentStore } from "@/stores/agentStore";
import { openChatDraft } from "@/pages/Chat/chatNavigation";
import { projectChatExecutionAgent } from "@/pages/Projects/projectChatNavigation";
import {
  beginConversationMetadataUpdate,
  invalidateConversationNavigation,
  registerNewConversationAction,
} from "../conversationNavigationEvents";
import { readDraftChatContext } from "@/pages/Chat/chatContext";
import { loadDraftConversationId } from "@/pages/Chat/conversationLifecycle";
import ConversationNavigation from "./index";

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
            "navigation.discovery": "Chat discovery",
            "navigation.projectFilter": "Project filter",
            "navigation.agentFilter": "Agent filter",
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
    resources: { revision: 1, directories: [] },
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
const savedHeaders = new Map<string, ChatSpec>();
const emptySearch: ChatSearchResponse = { results: [], offset: 0, limit: 20, total: 0, next_offset: null };
const standaloneMetadata = vi.fn<typeof chatApi.getMetadata>();

function savedPath(
  id: string,
  projectId: string | null = null,
  agentId = "alpha",
) {
  savedHeaders.set(id, {
    ...chat(id),
    agent_id: agentId,
    project_id: projectId,
  });
  return `/chats/${encodeURIComponent(id)}`;
}

function archiveApi(kind: "agent" | "project") {
  return kind === "project"
    ? projectChatsApi
    : { ...chatApi, getMetadata: standaloneMetadata };
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
  savedHeaders.clear();
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
  vi.mocked(chatApi.listChats).mockResolvedValue({
    data: ["alpha", "beta"].map((id) => ({
      ...chat(`${id}-current`, `${id} chat`),
      agent_id: id,
      project_id: null,
    })),
    total: 2,
  });
  vi.mocked(projectChatsApi.listChats).mockImplementation(async (id) => ({
    data: [chat(`${id}-current`, `${id} chat`)],
    total: 1,
  }));
  standaloneMetadata.mockReset().mockRejectedValue(new Error("Chat not found"));
  vi.mocked(chatApi.getMetadata).mockImplementation(
    async (id, agentId, options) => {
      if (options?.signal) return standaloneMetadata(id, agentId, options);
      const header = savedHeaders.get(id);
      if (!header) throw new Error("Chat not found");
      return header;
    },
  );
  vi.mocked(projectChatsApi.getMetadata).mockRejectedValue(
    new Error("Chat not found"),
  );
  vi.mocked(chatApi.searchChats).mockResolvedValue(emptySearch);
  vi.mocked(projectChatsApi.searchChats).mockResolvedValue(emptySearch);
});

describe("ConversationNavigation", () => {
  it("refreshes projects received from another device when returning to the foreground", async () => {
    vi.mocked(projectsApi.list).mockResolvedValue([]);
    render(
      <ConversationNavigation
        location={{ pathname: savedPath("alpha-current"), search: "" }}
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
          location={{ pathname: savedPath("alpha-current"), search: "" }}
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
          location={{ pathname: savedPath("alpha-current"), search: "" }}
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
          location={{ pathname: savedPath("one-current", "one"), search: "" }}
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

  it.each(["project", "standalone"] as const)(
    "keeps independent chats collapsed across More routes and remounts from %s",
    async (origin) => {
      const pathname =
        origin === "project"
          ? savedPath("one-current", "one")
          : savedPath("alpha-current");
      const onNavigate = vi.fn();
      const view = render(
        <ConversationNavigation
          location={{ pathname, search: "" }}
          onNavigate={onNavigate}
        />,
      );
      await screen.findByRole("button", {
        name: origin === "project" ? "one chat" : "alpha chat",
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
        location={{ pathname: savedPath("alpha-current"), search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByRole("button", { name: "alpha chat" });
    fireEvent.click(screen.getByRole("button", { name: "Independent chats" }));
    view.rerender(
      <ConversationNavigation
        location={{ pathname: savedPath("another"), search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Independent chats" }),
      ).toHaveAttribute("aria-expanded", "true"),
    );
  });

  it("retains the current archived row while refreshing its loaded group", async () => {
    vi.mocked(projectChatsApi.getMetadata).mockResolvedValue({
      ...chat("archive", "Open archive"),
      archived: true,
    });
    render(
      <ConversationNavigation
        location={{ pathname: savedPath("archive", "one"), search: "" }}
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
        chatId: "one-current",
      }),
    );
    expect(screen.getByRole("button", { name: /^Open archive/ })).toBe(
      archivedRow,
    );
    await act(async () =>
      pending.resolve({ data: [chat("one-current", "one chat")], total: 1 }),
    );
    expect(screen.getByRole("button", { name: /^Open archive/ })).toBe(
      archivedRow,
    );
  });

  it("keeps loaded rows stable while a committed archive refreshes in the background", async () => {
    const archived = {
      ...chat("archive", "Archive me"),
      agent_id: "alpha",
      project_id: "one",
    };
    const survivor = chat("one-current", "Keep me");
    vi.mocked(projectChatsApi.listChats).mockResolvedValue({
      data: [archived, survivor],
      total: 2,
    });
    render(
      <ConversationNavigation
        location={{ pathname: savedPath("one-current", "one"), search: "" }}
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
        { chatId: "archive" },
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

  it("loads only expanded scopes and navigates directly using the independent Chat ID", async () => {
    const onNavigate = vi.fn();
    const { rerender } = render(
      <ConversationNavigation
        location={{ pathname: savedPath("alpha-current"), search: "" }}
        onNavigate={onNavigate}
      />,
    );
    const activeChat = await screen.findByRole("button", {
      name: "alpha chat",
    });
    expect(activeChat).toHaveAttribute("aria-current", "page");
    expect(chatApi.listChats).toHaveBeenCalledTimes(1);
    expect(chatApi.listChats).toHaveBeenCalledWith(
      expect.objectContaining({
        offset: 0,
        limit: 5,
        view: "user",
        without_project: true,
      }),
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
    expect(onNavigate).toHaveBeenLastCalledWith("/chats/one-current");
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);
    rerender(
      <ConversationNavigation
        location={{
          pathname: savedPath("one-current", "one", "beta"),
          search: "",
        }}
        onNavigate={onNavigate}
      />,
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "one chat" })).toHaveAttribute(
        "aria-current",
        "page",
      ),
    );
    expect(
      screen.getByRole("button", { name: "alpha chat" }),
    ).not.toHaveAttribute("aria-current");
    expect(screen.getByRole("button", { name: "beta chat" })).toBeVisible();
    rerender(
      <ConversationNavigation
        location={{
          pathname: savedPath("beta-current", null, "beta"),
          search: "",
        }}
        onNavigate={onNavigate}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "Agent beta", exact: true }),
    ).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "beta chat" }));
    expect(onNavigate).toHaveBeenLastCalledWith("/chats/beta-current");
  });

  it("keeps both standalone Agents visible and uses only the current selector for New chat", async () => {
    const onNavigate = vi.fn();
    const view = render(
      <ConversationNavigation
        location={{
          pathname: savedPath("one-current", "one", "beta"),
          search: "",
        }}
        onNavigate={onNavigate}
      />,
    );
    await screen.findByRole("button", { name: "one chat" });
    const alpha = screen.getByRole("button", { name: "alpha chat" });
    const beta = screen.getByRole("button", { name: "beta chat" });
    expect(alpha).toBeVisible();
    expect(beta).toBeVisible();
    const create = screen.getByRole("button", {
      name: "New in Independent chats",
    });
    fireEvent.click(create);
    const id = loadDraftConversationId("no-project")!;
    expect(readDraftChatContext(id)).toEqual({
      agentId: "alpha",
      projectId: null,
    });
    expect(onNavigate).toHaveBeenLastCalledWith(`/chats/${id}`);
    act(() => useAgentStore.getState().setSelectedAgent("beta"));
    fireEvent.click(create);
    expect(onNavigate).toHaveBeenLastCalledWith(`/chats/${id}`);
    expect(readDraftChatContext(id)).toEqual({
      agentId: "beta",
      projectId: null,
    });
    view.rerender(
      <ConversationNavigation
        location={{ pathname: savedPath("two-current", "two"), search: "" }}
        onNavigate={onNavigate}
      />,
    );
    await screen.findByRole("button", { name: "two chat" });
    expect(screen.getByRole("button", { name: "alpha chat" })).toBe(alpha);
    expect(screen.getByRole("button", { name: "beta chat" })).toBe(beta);
    expect(chatApi.listChats).toHaveBeenCalledTimes(1);
  });

  it("keeps the standalone group loaded while resolving a Project Chat header", async () => {
    const props = { onNavigate: vi.fn() };
    const view = render(
      <ConversationNavigation
        {...props}
        location={{ pathname: savedPath("alpha-current"), search: "" }}
      />,
    );
    const standaloneRow = await screen.findByRole("button", {
      name: "alpha chat",
    });
    const loadCount = vi.mocked(chatApi.listChats).mock.calls.length;
    const nextHeader = deferred<ChatSpec>();
    vi.mocked(chatApi.getMetadata).mockReturnValueOnce(nextHeader.promise);
    view.rerender(
      <ConversationNavigation
        {...props}
        location={{ pathname: savedPath("one-current", "one"), search: "" }}
      />,
    );
    expect(standaloneRow.isConnected).toBe(true);
    expect(standaloneRow).toBeVisible();
    await act(async () => nextHeader.resolve(savedHeaders.get("one-current")!));
    expect(screen.getByRole("button", { name: "alpha chat" })).toBe(
      standaloneRow,
    );
    expect(standaloneRow).toBeVisible();
    expect(chatApi.listChats).toHaveBeenCalledTimes(loadCount);
    view.unmount();
  });

  it("opens the current project without inventing a saved row absent from its active list", async () => {
    localStorage.setItem(
      "rua_navigation_expanded_groups",
      JSON.stringify(["project:two"]),
    );
    render(
      <ConversationNavigation
        location={{ pathname: savedPath("beyond-page", "two"), search: "" }}
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
      const api = archiveApi(kind);
      vi.mocked(api.getMetadata).mockResolvedValue({
        ...chat("archived-chat", "Saved archived chat"),
        archived: true,
      });
      const projectId = kind === "project" ? "one" : null;
      const { container, rerender } = render(
        <ConversationNavigation
          location={{
            pathname: savedPath("archived-chat", projectId),
            search: "",
          }}
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
          location={{
            pathname: savedPath(
              kind === "project" ? "one-current" : "alpha-current",
              projectId,
            ),
            search: "",
          }}
          onNavigate={vi.fn()}
        />,
      );
      expect(screen.queryByText("Saved archived chat")).not.toBeInTheDocument();
      await waitFor(() =>
        expect(
          screen.getByRole("button", {
            name: kind === "project" ? "one chat" : "alpha chat",
          }),
        ).toHaveAttribute("aria-current", "page"),
      );
      expect(api.getMetadata).toHaveBeenCalledTimes(1);
    },
  );

  it.each(["agent", "project"] as const)(
    "delegates sidebar creation to the visible %s page without creating another draft",
    async (kind) => {
      const address = { chatId: "saved-chat" };
      const pathname = savedPath(
        address.chatId,
        kind === "project" ? "one" : null,
      );
      const action = vi.fn();
      const unregister = registerNewConversationAction(address, action);
      const onNavigate = vi.fn();
      try {
        render(
          <ConversationNavigation
            location={{
              pathname,
              search: "",
            }}
            onNavigate={onNavigate}
          />,
        );
        if (kind === "project") {
          await waitFor(() =>
            expect(
              screen.getByRole("button", { name: "New chat", exact: true }),
            ).toHaveTextContent("Project one"),
          );
        }
        await act(async () => {
          fireEvent.click(
            screen.getByRole("button", { name: "New chat", exact: true }),
          );
        });
        const draftId =
          kind === "agent"
            ? loadDraftConversationId("no-project")
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
      const api = archiveApi(kind);
      const response = deferred<ChatSpec>();
      vi.mocked(api.getMetadata).mockReturnValue(response.promise);
      const projectId = kind === "project" ? "one" : null;
      const { rerender } = render(
        <ConversationNavigation
          location={{
            pathname: savedPath("archived-chat", projectId),
            search: "",
          }}
          onNavigate={vi.fn()}
        />,
      );
      await waitFor(() => expect(api.getMetadata).toHaveBeenCalledOnce());
      const options = vi.mocked(api.getMetadata).mock.calls[0][2];
      rerender(
        <ConversationNavigation
          location={{
            pathname: savedPath(
              kind === "project" ? "one-current" : "alpha-current",
              projectId,
            ),
            search: "",
          }}
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
      const api = archiveApi(kind);
      const chatId =
        kind === "project" ? openChatDraft("one") : openChatDraft();
      render(
        <ConversationNavigation
          location={{
            pathname: `/chats/${chatId}`,
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
      const api = archiveApi(kind);
      vi.mocked(api.listChats).mockResolvedValue({ data: [], total: 0 });
      const projectId = kind === "project" ? "one" : null;
      const { container, rerender } = render(
        <ConversationNavigation
          location={{ pathname: savedPath("missing", projectId), search: "" }}
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
          location={{ pathname: savedPath("active", projectId), search: "" }}
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
        location={{ pathname: savedPath("archive", "one"), search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByText("Archived chat");
    const response = deferred<ChatSpec>();
    vi.mocked(projectChatsApi.getMetadata).mockReturnValue(response.promise);
    const address = {
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

  it("resumes each sidebar draft and applies the global Agent at the time of the click", async () => {
    const onNavigate = vi.fn();
    render(
      <ConversationNavigation
        location={{ pathname: "/settings", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    await screen.findByRole("button", { name: "alpha chat" });
    fireEvent.click(
      screen.getByRole("button", { name: "New chat", exact: true }),
    );
    const id = loadDraftConversationId("no-project");
    expect(onNavigate).toHaveBeenLastCalledWith(`/chats/${id}`);
    fireEvent.click(screen.getByRole("button", { name: "New in Project one" }));
    const firstProjectPath = onNavigate.mock.lastCall?.[0] as string;
    const projectDraftId = firstProjectPath.split("/").pop()!;
    expect(projectDraftId).not.toBe(id);
    expect(projectChatExecutionAgent("one", projectDraftId)).toBe("alpha");
    act(() => useAgentStore.getState().setSelectedAgent("beta"));
    fireEvent.click(screen.getByRole("button", { name: "New in Project one" }));
    expect(onNavigate).toHaveBeenLastCalledWith(firstProjectPath);
    expect(projectChatExecutionAgent("one", projectDraftId)).toBe("beta");
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
    "counts pinned %s chats in the page and hides Load more when complete",
    async (kind) => {
      const list =
        kind === "project" ? projectChatsApi.listChats : chatApi.listChats;
      vi.mocked(list)
        .mockResolvedValueOnce({
          data: [chat("pin", "Pinned chat", true), chat("first", "First chat")],
          total: 3,
        })
        .mockResolvedValueOnce({
          data: [chat("next", "Next chat")],
          total: 3,
        });
      render(
        <ConversationNavigation
          location={{
            pathname:
              kind === "project"
                ? savedPath("first", "one")
                : savedPath("first"),
            search: "",
          }}
          onNavigate={vi.fn()}
        />,
      );
      fireEvent.click(await screen.findByRole("button", { name: "Load more" }));
      await screen.findByRole("button", { name: "Next chat" });
      expect(list).toHaveBeenLastCalledWith(
        ...(kind === "project"
          ? ["one", expect.objectContaining({ offset: 2, limit: 20 })]
          : [
              expect.objectContaining({
                offset: 2,
                limit: 20,
                without_project: true,
              }),
            ]),
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
                ? savedPath("one-current", "one")
                : savedPath("alpha-current"),
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
        location={{ pathname: savedPath("first", "one"), search: "" }}
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
        location={{ pathname: savedPath("latest", "one"), search: "" }}
        onNavigate={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1),
    );
    act(() =>
      invalidateConversationNavigation({ chatId: "other" }, false, "two"),
    );
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);
    act(() =>
      invalidateConversationNavigation({ chatId: "latest" }, false, "one"),
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
        location={{ pathname: savedPath("recovered", "one"), search: "" }}
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

  it("discards stale global search results after applying Project and Agent filters", async () => {
    const oldSearch = deferred<ChatSearchResponse>();
    vi.mocked(chatApi.searchChats).mockReturnValueOnce(oldSearch.promise);
    vi.mocked(chatApi.searchChats).mockResolvedValue({
      ...emptySearch,
      total: 1,
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
        location={{ pathname: savedPath("alpha-current"), search: "" }}
        onNavigate={onNavigate}
      />,
    );
    await screen.findByRole("button", { name: "alpha chat" });
    fireEvent.click(screen.getByRole("button", { name: "Search chats" }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Search chats" }), {
      target: { value: "needle" },
    });
    await waitFor(() =>
      expect(chatApi.searchChats).toHaveBeenCalledWith(
        "needle",
        expect.objectContaining({
          offset: 0,
          limit: 20,
          view: "user",
          archived: false,
        }),
      ),
    );
    fireEvent.change(screen.getByRole("combobox", { name: "Project filter" }), {
      target: { value: "project:two" },
    });
    fireEvent.change(screen.getByRole("combobox", { name: "Agent filter" }), {
      target: { value: "beta" },
    });
    await screen.findByRole("button", { name: /Found in project/ });
    await act(async () => oldSearch.resolve(emptySearch));
    fireEvent.click(screen.getByRole("button", { name: /Found in project/ }));
    expect(chatApi.searchChats).toHaveBeenLastCalledWith(
      "needle",
      expect.objectContaining({
        project_id: "two",
        agent_id: "beta",
        view: "user",
        archived: false,
      }),
    );
    expect(onNavigate).toHaveBeenLastCalledWith("/chats/found%2Fchat");
    expect(screen.queryByRole("searchbox")).not.toBeInTheDocument();
    expect(
      JSON.parse(sessionStorage.getItem("rua_search_scroll_target")!),
    ).toMatchObject({
      conversationId: "found/chat",
      ownerKey: "chat",
      cardIndex: 4,
    });
  });

  it("discovers archived Projects through their fixed filter and keeps their Chats read-only", async () => {
    const archived = { ...project("done"), lifecycle: "archived" as const };
    vi.mocked(projectsApi.list).mockImplementation(async (lifecycle) =>
      lifecycle === "all" ? [project("one"), archived] : [project("one")],
    );
    vi.mocked(chatApi.listChats).mockImplementation(async (params) => {
      const data =
        params?.project_id === "done"
          ? [{ ...chat("saved"), project_id: "done" }]
          : [];
      return { data, total: data.length };
    });
    const onNavigate = vi.fn();
    render(
      <ConversationNavigation
        location={{ pathname: "/settings", search: "" }}
        onNavigate={onNavigate}
      />,
    );
    await screen.findByRole("button", { name: "Project one" });
    expect(
      screen.queryByRole("button", { name: "Project done" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Search chats" }));
    const region = screen.getByRole("region", { name: "Chat discovery" });
    const filter = within(region).getByLabelText("Project filter");
    expect(
      within(filter).getByRole("option", { name: /Project done/ }),
    ).toBeInTheDocument();
    fireEvent.change(filter, { target: { value: "project:done" } });
    const row = await within(region).findByTitle("saved");
    fireEvent.click(
      within(region).getByRole("button", { name: /Actions for/ }),
    );
    await screen.findByRole("menuitem", {name: "navigation.manageConversations"});
    expect(
      screen.queryByRole("menuitem", { name: "common.rename" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("menuitem", { name: "projects.chat.archive" }),
    ).not.toBeInTheDocument();
    fireEvent.click(row);
    expect(onNavigate).toHaveBeenCalledWith("/chats/saved");
  });

  it.each([
    "during append",
    "before append",
    "when returning during append",
  ] as const)(
    "reconciles the filtered discovery pages when a refresh starts %s",
    async (timing) => {
      const deep = timing === "during append";
      const count = deep ? 248 : 46;
      const rows = Array.from({ length: count }, (_, n) => ({
        ...chat(`row-${n}`),
        agent_id: "alpha",
        project_id: "one",
      }));
      rows.push({
        ...chat("other-project"),
        agent_id: "alpha",
        project_id: "two",
      });
      const stale = deferred<{ data: ChatSpec[]; total: number }>();
      let delayNext = false;
      const requests: { offset: number; limit: number }[] = [];
      vi.mocked(chatApi.listChats).mockImplementation(async (params = {}) => {
        if (!params.agent_id || !params.project_id)
          return { data: [], total: 0 };
        const { offset = 0, limit = 20 } = params;
        requests.push({ offset, limit });
        if (delayNext) {
          delayNext = false;
          return stale.promise;
        }
        const matching = rows.filter(
          (row) =>
            row.project_id === params.project_id &&
            row.agent_id === params.agent_id,
        );
        return {
          data: matching.slice(offset, offset + limit),
          total: matching.length,
        };
      });
      render(
        <ConversationNavigation
          location={{ pathname: "/settings", search: "" }}
          onNavigate={vi.fn()}
        />,
      );
      await screen.findByRole("button", { name: "Project one", exact: true });
      fireEvent.click(screen.getByRole("button", { name: "Search chats" }));
      const region = screen.getByRole("region", { name: "Chat discovery" });
      fireEvent.change(within(region).getByLabelText("Project filter"), {
        target: { value: "project:one" },
      });
      fireEvent.change(within(region).getByLabelText("Agent filter"), {
        target: { value: "alpha" },
      });
      await within(region).findByTitle("row-19");

      if (deep) {
        for (let last = 39; last < 220; last += 20) {
          fireEvent.click(within(region).getByText("Load more"));
          await within(region).findByTitle(`row-${last}`);
        }
      }
      delayNext = true;
      if (timing !== "before append") {
        fireEvent.click(within(region).getByText("Load more"));
        rows[0].agent_id = "beta";
        if (deep)
          act(() =>
            invalidateConversationNavigation({ chatId: "row-0" }, false, "one"),
          );
        else act(() => window.dispatchEvent(new Event("rua:foreground")));
      } else {
        act(() => window.dispatchEvent(new Event("rua:foreground")));
        rows[0].agent_id = "beta";
        fireEvent.click(within(region).getByText("Load more"));
      }
      await act(async () => stale.resolve({ data: [], total: 0 }));
      const expected = deep ? 240 : 40;
      await within(region).findByTitle(`row-${expected}`);
      expect(within(region).queryByTitle("row-0")).not.toBeInTheDocument();
      expect(
        within(region).queryByTitle("other-project"),
      ).not.toBeInTheDocument();
      expect(region.querySelectorAll("[data-chat-id]")).toHaveLength(expected);
      expect(requests.every((request) => request.limit <= 200)).toBe(true);
      if (deep) expect(requests).toContainEqual({ offset: 200, limit: 40 });
    },
  );

  it("does not present a lazy draft as a saved conversation", async () => {
    const draftId = openChatDraft();
    render(
      <ConversationNavigation
        location={{ pathname: `/chats/${draftId}`, search: "" }}
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
          location={{ pathname: savedPath("one-current", "one"), search: "" }}
        />,
      );
      await screen.findByRole("button", { name: "one chat" });
      view.rerender(
        <ConversationNavigation
          {...props}
          location={{ pathname: savedPath("two-current", "two"), search: "" }}
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
      location: { pathname: savedPath("alpha-current"), search: "" },
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
      location={{ pathname: savedPath("one-current", "one"), search: "" }}
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
      location={{ pathname: savedPath("one-current", "one"), search: "" }}
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
      location={{ pathname: savedPath("one-current", "one"), search: "" }}
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
      location={{ pathname: savedPath("one-current", "one"), search: "" }}
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
