import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { projectChatsApi } from "@/api/modules/projectChats";
import { Modal } from "@/ui";

import ProjectChatsDrawer from "./ProjectChatsDrawer";

const translate = (key: string) => key;

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: translate }),
}));

vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: {
    listChats: vi.fn(),
    searchChats: vi.fn(),
    updateChat: vi.fn(),
    clearArchive: vi.fn(),
  },
}));

vi.mock("@/pages/Chat/ChatHistoryDrawer", () => ({
  default: ({
    chats,
    searchContent,
    searchValue,
    onSearchChange,
    onSortChange,
    onTagFilterChange,
    onSelect,
    projectArchive,
    onRename,
    onTogglePin,
    onCycleTag,
    conversationView,
    onConversationViewChange,
    onCollaborationFiltersChange,
  }: {
    chats: Array<{
      id: string;
      name: string;
      pinned?: boolean;
      tag?: "none" | "red" | "yellow" | "green";
    }>;
    searchContent?: ReactNode;
    searchValue: string;
    onSearchChange: (value: string) => void;
    onSortChange: (sort: {
      sort_by: "title" | "created_at" | "updated_at";
      sort_order: "asc" | "desc";
    }) => void;
    onTagFilterChange: (
      tag: "none" | "red" | "yellow" | "green" | null,
    ) => void;
    onSelect: (chat: { id: string; name: string }) => void;
    projectArchive: {
      onToggle: () => void;
      onChange: (chat: { id: string; name: string }) => void;
      onClear: () => void;
    };
    onRename: (chat: { id: string; name: string; pinned?: boolean }) => void;
    onTogglePin: (chat: { id: string; name: string; pinned?: boolean }) => void;
    onCycleTag: (chat: {
      id: string;
      name: string;
      tag?: "none" | "red" | "yellow" | "green";
    }) => void;
    conversationView: "user" | "collaboration";
    onConversationViewChange: (view: "user" | "collaboration") => void;
    onCollaborationFiltersChange: (filters: {
      sourceChatId: string;
      executor: string;
      status: string;
    }) => void;
  }) => (
    <div>
      <button onClick={projectArchive.onToggle}>archive-list</button>
      <button onClick={projectArchive.onClear}>clear-archive</button>
      <button onClick={() => onConversationViewChange("collaboration")}>
        collaboration-view
      </button>
      <span>{conversationView}</span>
      <button
        onClick={() =>
          onCollaborationFiltersChange({
            sourceChatId: "source-a",
            executor: "mock_agent",
            status: "running",
          })
        }
      >
        collaboration-filters
      </button>
      <input
        aria-label="search"
        value={searchValue}
        onChange={(event) => onSearchChange(event.target.value)}
      />
      <div data-testid="search-content">{searchContent}</div>
      <button
        type="button"
        onClick={() => onSortChange({ sort_by: "title", sort_order: "asc" })}
      >
        sort-title-asc
      </button>
      <button type="button" onClick={() => onTagFilterChange("red")}>
        filter-red
      </button>
      {chats.map((chat) => (
        <div key={chat.id}>
          <button onClick={() => projectArchive.onChange(chat)}>
            archive-{chat.id}
          </button>
          <button onClick={() => onSelect(chat)}>{chat.name}</button>
          <span>{chat.pinned ? "pinned" : "unpinned"}</span>
          <span>{chat.tag ?? "none"}</span>
          <button onClick={() => onTogglePin(chat)}>toggle-{chat.id}</button>
          <button onClick={() => onCycleTag(chat)}>tag-{chat.id}</button>
          <button onClick={() => onRename(chat)}>rename-{chat.id}</button>
        </div>
      ))}
    </div>
  ),
}));

vi.mock("@/pages/Chat/ChatSearchResults", () => ({
  default: ({ query }: { query: string }) => <div>Search: {query}</div>,
}));
vi.mock("@/ui", () => ({
  Modal: { confirm: vi.fn() },
  TextInput: ({ children }: { children?: ReactNode }) => <>{children}</>,
  toast: { error: vi.fn() },
}));

describe("ProjectChatsDrawer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(projectChatsApi.listChats).mockResolvedValue({
      data: [{ id: "chat-a", name: "Conversation A", pinned: false }],
      total: 1,
    });
  });

  it("archives without navigating away and restores from the archive", async () => {
    const removed = vi.fn();
    vi.mocked(projectChatsApi.updateChat).mockResolvedValue({
      id: "chat-a",
      archived: true,
    } as never);
    render(
      <ProjectChatsDrawer
        projectId="project-a"
        currentChatId="chat-a"
        currentChatRunning={false}
        onClose={vi.fn()}
        onSelectChat={vi.fn()}
        onChatUpdated={vi.fn()}
        onChatDeleted={removed}
      />,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "archive-chat-a" }),
    );
    await waitFor(() =>
      expect(projectChatsApi.updateChat).toHaveBeenCalledWith(
        "project-a",
        "chat-a",
        { archived: true },
      ),
    );
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenCalledTimes(2),
    );
    expect(removed).not.toHaveBeenCalled();
    vi.mocked(projectChatsApi.listChats).mockResolvedValue({
      data: [{ id: "chat-a", name: "Archived", archived: true }],
      total: 1,
    } as never);
    fireEvent.click(screen.getByText("archive-list"));
    await screen.findByText("Archived");
    fireEvent.click(screen.getByRole("button", { name: "archive-chat-a" }));
    await waitFor(() =>
      expect(projectChatsApi.updateChat).toHaveBeenLastCalledWith(
        "project-a",
        "chat-a",
        { archived: false },
      ),
    );
  });

  it("confirms before clearing the whole archive and prevents duplicate submissions", async () => {
    render(
      <ProjectChatsDrawer
        projectId="project-a"
        currentChatId="other"
        currentChatRunning={false}
        onClose={vi.fn()}
        onSelectChat={vi.fn()}
        onChatUpdated={vi.fn()}
        onChatDeleted={vi.fn()}
      />,
    );
    await screen.findByText("Conversation A");
    fireEvent.click(screen.getByText("archive-list"));
    vi.mocked(Modal.confirm).mockResolvedValueOnce(false);
    fireEvent.click(screen.getByText("clear-archive"));
    await waitFor(() => expect(Modal.confirm).toHaveBeenCalledTimes(1));
    expect(projectChatsApi.clearArchive).not.toHaveBeenCalled();
    vi.mocked(Modal.confirm).mockResolvedValueOnce(true);
    vi.mocked(projectChatsApi.clearArchive).mockResolvedValue({
      deleted: ["chat-a"],
      skipped: [],
    });
    fireEvent.click(screen.getByText("clear-archive"));
    fireEvent.click(screen.getByText("clear-archive"));
    await waitFor(() =>
      expect(projectChatsApi.clearArchive).toHaveBeenCalledTimes(1),
    );
  });

  it("shows project chats until a non-blank search is active", async () => {
    render(
      <ProjectChatsDrawer
        projectId="project-a"
        currentChatRunning={false}
        onClose={vi.fn()}
        onSelectChat={vi.fn()}
        onChatUpdated={vi.fn()}
        onChatDeleted={vi.fn()}
      />,
    );

    expect(
      await screen.findByRole("button", { name: "Conversation A" }),
    ).toBeInTheDocument();
    expect(screen.getByTestId("search-content")).toBeEmptyDOMElement();

    await act(async () => {
      fireEvent.change(screen.getByRole("textbox", { name: "search" }), {
        target: { value: "   " },
      });
    });

    expect(screen.getByTestId("search-content")).toBeEmptyDOMElement();

    await act(async () => {
      fireEvent.change(screen.getByRole("textbox", { name: "search" }), {
        target: { value: "Conversation" },
      });
    });

    expect(screen.getByText("Search: Conversation")).toBeInTheDocument();
  });

  it("selects the current chat so the parent can close the drawer", async () => {
    const onSelectChat = vi.fn();
    render(
      <ProjectChatsDrawer
        projectId="project-a"
        currentChatId="chat-a"
        currentChatRunning={false}
        onClose={vi.fn()}
        onSelectChat={onSelectChat}
        onChatUpdated={vi.fn()}
        onChatDeleted={vi.fn()}
      />,
    );

    fireEvent.click(
      await screen.findByRole("button", { name: "Conversation A" }),
    );

    await waitFor(() =>
      expect(onSelectChat).toHaveBeenCalledWith(
        expect.objectContaining({ id: "chat-a" }),
      ),
    );
  });

  it("loads the default order and reloads from the first page when sorting changes", async () => {
    render(
      <ProjectChatsDrawer
        projectId="project-a"
        currentChatRunning={false}
        onClose={vi.fn()}
        onSelectChat={vi.fn()}
        onChatUpdated={vi.fn()}
        onChatDeleted={vi.fn()}
      />,
    );

    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenCalledWith("project-a", {
        offset: 0,
        limit: 20,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
        view: "user",
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "sort-title-asc" }));

    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenLastCalledWith("project-a", {
        offset: 0,
        limit: 20,
        archived: false,
        sort_by: "title",
        sort_order: "asc",
        view: "user",
      }),
    );
  });

  it("reloads from the first page when the tag filter changes", async () => {
    renderDrawer();
    await screen.findByRole("button", { name: "Conversation A" });

    fireEvent.click(screen.getByRole("button", { name: "filter-red" }));

    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenLastCalledWith("project-a", {
        offset: 0,
        limit: 20,
        archived: false,
        sort_by: "updated_at",
        sort_order: "desc",
        tag: "red",
        view: "user",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "archive-list" }));
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenLastCalledWith("project-a", {
        offset: 0,
        limit: 20,
        archived: true,
        view: "all",
        sort_by: "updated_at",
        sort_order: "desc",
      }),
    );
    vi.mocked(Modal.confirm).mockResolvedValue(true);
    vi.mocked(projectChatsApi.clearArchive).mockResolvedValue({
      deleted: [],
      skipped: [],
    });
    fireEvent.click(screen.getByRole("button", { name: "clear-archive" }));
    await waitFor(() =>
      expect(projectChatsApi.clearArchive).toHaveBeenCalledWith("project-a", {
        view: "all",
      }),
    );
  });

  it("updates pin state immediately while the remote update is pending", async () => {
    const update = deferred<{
      id: string;
      name: string;
      pinned: boolean;
      pin_order: number;
    }>();
    vi.mocked(projectChatsApi.updateChat).mockReturnValue(update.promise);

    renderDrawer();
    await screen.findByRole("button", { name: "Conversation A" });
    fireEvent.click(screen.getByRole("button", { name: "toggle-chat-a" }));

    expect(screen.getByText("pinned")).toBeInTheDocument();
    expect(projectChatsApi.updateChat).toHaveBeenCalledWith(
      "project-a",
      "chat-a",
      expect.objectContaining({ pinned: true }),
    );
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);

    update.resolve({
      id: "chat-a",
      name: "Conversation A",
      pinned: true,
      pin_order: 42,
    });
    await waitFor(() => expect(projectChatsApi.updateChat).toHaveResolved());
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);
  });

  it("rolls pin state back when the remote update fails", async () => {
    const update = deferred<never>();
    vi.mocked(projectChatsApi.updateChat).mockReturnValue(update.promise);

    renderDrawer();
    await screen.findByRole("button", { name: "Conversation A" });
    fireEvent.click(screen.getByRole("button", { name: "toggle-chat-a" }));
    expect(screen.getByText("pinned")).toBeInTheDocument();

    update.reject(new Error("offline"));
    await screen.findByText("unpinned");
  });

  it("cycles the tag to red without reloading the list", async () => {
    vi.mocked(projectChatsApi.updateChat).mockResolvedValue({
      id: "chat-a",
      name: "Conversation A",
      tag: "red",
    } as never);

    renderDrawer();
    await screen.findByRole("button", { name: "Conversation A" });
    fireEvent.click(screen.getByRole("button", { name: "tag-chat-a" }));

    expect(screen.getByText("red")).toBeInTheDocument();
    expect(projectChatsApi.updateChat).toHaveBeenCalledWith(
      "project-a",
      "chat-a",
      {
        tag: "red",
      },
    );
    await waitFor(() => expect(projectChatsApi.updateChat).toHaveResolved());
    expect(projectChatsApi.listChats).toHaveBeenCalledTimes(1);
  });

  it("updates a title immediately and rolls it back when saving fails", async () => {
    const update = deferred<never>();
    vi.mocked(projectChatsApi.updateChat).mockReturnValue(update.promise);
    vi.mocked(Modal.confirm).mockImplementation(async (config) => {
      const content = config.content as {
        props: { onChange: (event: { target: { value: string } }) => void };
      };
      content.props.onChange({ target: { value: "Renamed remotely" } });
      return true;
    });

    renderDrawer();
    await screen.findByRole("button", { name: "Conversation A" });
    fireEvent.click(screen.getByRole("button", { name: "rename-chat-a" }));

    expect(
      await screen.findByRole("button", { name: "Renamed remotely" }),
    ).toBeInTheDocument();
    update.reject(new Error("offline"));
    expect(
      await screen.findByRole("button", { name: "Conversation A" }),
    ).toBeInTheDocument();
  });

  it("switches to the collaboration view and forwards relationship filters", async () => {
    renderDrawer();
    await screen.findByRole("button", { name: "Conversation A" });
    fireEvent.click(screen.getByRole("button", { name: "collaboration-view" }));
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenLastCalledWith(
        "project-a",
        expect.objectContaining({ view: "collaboration" }),
      ),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "collaboration-filters" }),
    );
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenLastCalledWith(
        "project-a",
        expect.objectContaining({
          view: "collaboration",
          source_chat_id: "source-a",
          executor: "mock_agent",
          status: "running",
        }),
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: "archive-list" }));
    await waitFor(() =>
      expect(projectChatsApi.listChats).toHaveBeenLastCalledWith("project-a", {
        offset: 0,
        limit: 20,
        archived: true,
        view: "all",
        sort_by: "updated_at",
        sort_order: "desc",
      }),
    );
    vi.mocked(Modal.confirm).mockResolvedValue(true);
    vi.mocked(projectChatsApi.clearArchive).mockResolvedValue({
      deleted: [],
      skipped: [],
    });
    fireEvent.click(screen.getByRole("button", { name: "clear-archive" }));
    await waitFor(() =>
      expect(projectChatsApi.clearArchive).toHaveBeenCalledWith("project-a", {
        view: "all",
      }),
    );
  });
});

function renderDrawer() {
  return render(
    <ProjectChatsDrawer
      projectId="project-a"
      currentChatRunning={false}
      onClose={vi.fn()}
      onSelectChat={vi.fn()}
      onChatUpdated={vi.fn()}
      onChatDeleted={vi.fn()}
    />,
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}
