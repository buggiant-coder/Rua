import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { chatApi } from "@/api/modules/chat";
import { projectChatsApi } from "@/api/modules/projectChats";
import { Modal } from "@/ui";
import type { ChatSpec } from "@/api/types";
import type { ConversationAddress } from "@/pages/Chat/conversationAddress";
import ConversationRow from "./ConversationRow";
import {
  useConversationGroup,
  type NavigationScope,
} from "./useConversationGroups";
import {
  beginConversationMetadataUpdate,
  invalidateConversationNavigation,
  publishConversationMetadata,
} from "../conversationNavigationEvents";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/api/modules/chat", () => ({
  chatApi: { listChats: vi.fn(), updateChat: vi.fn(), deleteChat: vi.fn() },
}));
vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: {
    listChats: vi.fn(),
    updateChat: vi.fn(),
    deleteChat: vi.fn(),
  },
}));
vi.mock("@/ui", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/ui")>()),
  Modal: { confirm: vi.fn() },
}));
const chat: ChatSpec = {
  id: "chat-one",
  name: "Example",
  user_id: "fixture",
  channel: "console",
  created_at: null,
  updated_at: null,
  tag: "none",
};
const address: ConversationAddress = {
  scope: "project",
  projectId: "project-one",
  chatId: chat.id,
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function show(
  overrides: Partial<React.ComponentProps<typeof ConversationRow>> = {},
) {
  const props = {
    scope: { kind: "project" as const, id: "project-one" },
    chat,
    current: address,
    getCurrentAddress: () => address,
    onManage: vi.fn(),
    onNavigate: vi.fn(),
    onRemoved: vi.fn(),
    ...overrides,
  };
  render(<ConversationRow {...props} />);
  fireEvent.click(
    screen.getByRole("button", { name: "navigation.conversationActions" }),
  );
  return props;
}
async function clickDelete() {
  fireEvent.click(
    await screen.findByRole("menuitem", { name: "common.delete" }),
  );
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(Modal.confirm).mockResolvedValue(true);
  vi.mocked(projectChatsApi.listChats).mockResolvedValue({
    data: [{ ...chat, id: "remaining" }],
    total: 1,
  });
});

describe("ConversationRow deletion", () => {
  it("confirms newly busy project work before using force", async () => {
    vi.mocked(projectChatsApi.deleteChat)
      .mockRejectedValueOnce(
        new Error("Project Chat has active or pending work"),
      )
      .mockResolvedValueOnce({
        deleted: true,
        chat_id: chat.id,
        was_busy: true,
      });
    const props = show();
    await clickDelete();
    await waitFor(() =>
      expect(props.onNavigate).toHaveBeenCalledWith(
        "/projects/project-one/chats/remaining",
      ),
    );
    expect(projectChatsApi.deleteChat).toHaveBeenNthCalledWith(
      1,
      "project-one",
      chat.id,
      false,
    );
    expect(Modal.confirm).toHaveBeenCalledTimes(2);
    expect(projectChatsApi.deleteChat).toHaveBeenNthCalledWith(
      2,
      "project-one",
      chat.id,
      true,
    );
  });
  it("does not force deletion when the busy confirmation is cancelled", async () => {
    vi.mocked(Modal.confirm)
      .mockResolvedValueOnce(true)
      .mockResolvedValueOnce(false);
    vi.mocked(projectChatsApi.deleteChat).mockRejectedValueOnce(
      new Error("Project Chat has active or pending work"),
    );
    const props = show();
    await clickDelete();
    await waitFor(() => expect(Modal.confirm).toHaveBeenCalledTimes(2));
    expect(projectChatsApi.deleteChat).toHaveBeenCalledTimes(1);
    expect(props.onRemoved).not.toHaveBeenCalled();
  });
  it("does not navigate back if the user changes scope while deletion is pending", async () => {
    const result = deferred<{
      deleted: true;
      chat_id: string;
      was_busy: boolean;
    }>();
    vi.mocked(projectChatsApi.deleteChat).mockReturnValue(result.promise);
    let current: ConversationAddress = address;
    const props = show({ getCurrentAddress: () => current });
    await clickDelete();
    await waitFor(() => expect(projectChatsApi.deleteChat).toHaveBeenCalled());
    current = { scope: "agent", agentId: "other", chatId: "other-chat" };
    await act(async () =>
      result.resolve({ deleted: true, chat_id: chat.id, was_busy: false }),
    );
    expect(props.onRemoved).toHaveBeenCalledWith(chat.id);
    expect(props.onNavigate).not.toHaveBeenCalled();
    expect(projectChatsApi.listChats).not.toHaveBeenCalled();
  });
  it("keeps a pinned conversation undeletable", async () => {
    show({ chat: { ...chat, pinned: true } });
    const action = await screen.findByRole("menuitem", {
      name: "navigation.unpinBeforeDelete",
    });
    expect(action).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(action);
    expect(Modal.confirm).not.toHaveBeenCalled();
    expect(projectChatsApi.deleteChat).not.toHaveBeenCalled();
    expect(chatApi.deleteChat).not.toHaveBeenCalled();
  });
});

function GroupRows({ scope }: { scope: NavigationScope }) {
  const group = useConversationGroup(scope, true);
  return (
    <div data-testid="group-rows">
      {group.chats.map((item) => (
        <ConversationRow
          key={item.id}
          scope={scope}
          chat={item}
          current={null}
          getCurrentAddress={() => null}
          onManage={() => {}}
          onNavigate={() => {}}
          onRemoved={group.removeChat}
        />
      ))}
      {group.hasMore && <button onClick={group.loadMore}>Load more</button>}
    </div>
  );
}

describe.each(["agent", "project"] as const)("%s sidebar metadata", (kind) => {
  const scope: NavigationScope = { kind, id: "scope-one" };
  const list =
    kind === "project" ? projectChatsApi.listChats : chatApi.listChats;
  const update =
    kind === "project" ? projectChatsApi.updateChat : chatApi.updateChat;
  it("shows a tag immediately and retains the acknowledged value over a stale reload", async () => {
    const pending = deferred<ChatSpec>();
    vi.mocked(list).mockResolvedValue({ data: [chat], total: 1 });
    vi.mocked(update).mockReturnValue(pending.promise);
    const view = render(<GroupRows scope={scope} />);
    await screen.findByTitle("Example");
    fireEvent.click(
      screen.getByRole("button", { name: "navigation.conversationActions" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "chat.tag.red" }),
    );
    const marker = () => view.container.querySelector("[data-chat-marker]");
    expect(marker()).toHaveAttribute("data-tag", "red");
    await act(async () => pending.resolve({ ...chat, tag: "red" }));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    expect(marker()).toHaveAttribute("data-tag", "red");
  });
  const scopedAddress: ConversationAddress =
    kind === "project"
      ? { scope: "project", projectId: scope.id, chatId: chat.id }
      : { scope: "agent", agentId: scope.id, chatId: chat.id };

  it("rolls back only the failed tag while preserving a concurrent title change", async () => {
    const pending = deferred<ChatSpec>();
    vi.mocked(list).mockResolvedValue({ data: [chat], total: 1 });
    vi.mocked(update).mockReturnValue(pending.promise);
    const view = render(<GroupRows scope={scope} />);
    await screen.findByTitle("Example");
    fireEvent.click(
      screen.getByRole("button", { name: "navigation.conversationActions" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "chat.tag.red" }),
    );
    expect(view.container.querySelector("[data-chat-marker]")).toHaveAttribute(
      "data-tag",
      "red",
    );
    let rename!: ReturnType<typeof beginConversationMetadataUpdate>;
    act(() => {
      rename = beginConversationMetadataUpdate(
        scopedAddress,
        { ...chat, tag: "red" },
        { name: "Renamed elsewhere" },
      );
    });
    act(() =>
      rename.commit({ ...chat, tag: "red", name: "Renamed elsewhere" }),
    );
    await act(async () => pending.reject(new Error("offline")));
    expect(screen.getByTitle("Renamed elsewhere")).toBeInTheDocument();
    expect(view.container.querySelector("[data-chat-marker]")).toHaveAttribute(
      "data-tag",
      "none",
    );
  });

  it("sorts pin and unpin immediately, uses confirmed metadata, and rebuilds pagination past stale membership", async () => {
    const newer = {
      ...chat,
      id: "newer",
      name: "Newer",
      updated_at: "2026-10-23T08:00:00Z",
    };
    const older = { ...chat, updated_at: "2026-10-22T08:00:00Z" };
    const oldest = {
      ...chat,
      id: "oldest",
      name: "Oldest",
      updated_at: "2026-10-21T08:00:00Z",
    };
    vi.mocked(list).mockResolvedValue({ data: [newer, older], total: 3 });
    const pin = deferred<ChatSpec>();
    const unpin = deferred<ChatSpec>();
    vi.mocked(update)
      .mockReturnValueOnce(pin.promise)
      .mockReturnValueOnce(unpin.promise);
    const view = render(<GroupRows scope={scope} />);
    await screen.findByTitle("Example");
    const order = () =>
      [...view.container.querySelectorAll("[data-chat-id]")].map((row) =>
        row.getAttribute("data-chat-id"),
      );
    const openMenu = () =>
      fireEvent.click(
        within(
          view.container.querySelector(
            `[data-chat-id="${chat.id}"]`,
          ) as HTMLElement,
        ).getByRole("button", { name: "navigation.conversationActions" }),
      );
    openMenu();
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "projects.chat.pin" }),
    );
    expect(order()).toEqual([chat.id, "newer"]);
    const pinned = { ...older, pinned: true, pin_order: 10 };
    await act(async () => pin.resolve(pinned));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    expect(order()).toEqual([chat.id, "newer"]);
    // The next page must restart at zero: the server has moved this item out of ordinary rows.
    vi.mocked(list).mockResolvedValue({
      data: [pinned, newer, oldest],
      total: 3,
    });
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    await screen.findByTitle("Oldest");
    expect(list).toHaveBeenLastCalledWith(
      ...(kind === "project"
        ? [scope.id, expect.objectContaining({ offset: 0, limit: 22 })]
        : [expect.objectContaining({ offset: 0, limit: 22 }), scope.id]),
    );
    expect(order()).toEqual([chat.id, "newer", "oldest"]);
    expect(
      screen.queryByRole("button", { name: "Load more" }),
    ).not.toBeInTheDocument();
    openMenu();
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "projects.chat.unpin" }),
    );
    expect(order()).toEqual(["newer", chat.id, "oldest"]);
    await act(async () =>
      unpin.resolve({ ...older, pinned: false, pin_order: 0 }),
    );
    expect(order()).toEqual(["newer", chat.id, "oldest"]);
  });

  it("ignores a superseded completion and keeps pending changes over a late list response", async () => {
    const stale = deferred<{ data: ChatSpec[]; total: number }>();
    vi.mocked(list)
      .mockResolvedValue({ data: [chat], total: 1 })
      .mockResolvedValueOnce({ data: [chat], total: 1 })
      .mockReturnValueOnce(stale.promise);
    const pending = deferred<ChatSpec>();
    vi.mocked(update).mockReturnValue(pending.promise);
    const view = render(<GroupRows scope={scope} />);
    await screen.findByTitle("Example");
    act(() => invalidateConversationNavigation(scopedAddress));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    fireEvent.click(
      screen.getByRole("button", { name: "navigation.conversationActions" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "chat.tag.red" }),
    );
    await act(async () => stale.resolve({ data: [chat], total: 1 }));
    expect(view.container.querySelector("[data-chat-marker]")).toHaveAttribute(
      "data-tag",
      "red",
    );
    act(() =>
      publishConversationMetadata(scopedAddress, { ...chat, tag: "green" }),
    );
    await act(async () => pending.resolve({ ...chat, tag: "red" }));
    expect(view.container.querySelector("[data-chat-marker]")).toHaveAttribute(
      "data-tag",
      "green",
    );
  });
  it("restores an optimistically archived row when its request fails after unmount", async () => {
    const pending = deferred<ChatSpec>();
    vi.mocked(list).mockResolvedValue({ data: [chat], total: 1 });
    vi.mocked(update).mockReturnValue(pending.promise);
    render(<GroupRows scope={scope} />);
    await screen.findByTitle("Example");
    fireEvent.click(
      screen.getByRole("button", { name: "navigation.conversationActions" }),
    );
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "projects.chat.archive" }),
    );
    expect(screen.queryByTitle("Example")).not.toBeInTheDocument();
    await act(async () => pending.reject(new Error("offline")));
    expect(screen.getByTitle("Example")).toBeInTheDocument();
  });

  it.each([false, true])(
    "releases confirmed fields for a later server edit (read same value first: %s)",
    async (readSameValue) => {
      const original = { ...chat, updated_at: "2026-10-20T08:00:00Z" };
      const confirmed = {
        ...chat,
        tag: "red" as const,
        updated_at: "2026-10-21T08:00:00Z",
      };
      const newer = {
        ...chat,
        tag: "green" as const,
        updated_at: "2026-10-22T08:00:00Z",
      };
      vi.mocked(list).mockResolvedValue({ data: [original], total: 1 });
      vi.mocked(update).mockResolvedValue(confirmed);
      const view = render(<GroupRows scope={scope} />);
      await screen.findByTitle("Example");
      fireEvent.click(
        screen.getByRole("button", { name: "navigation.conversationActions" }),
      );
      fireEvent.click(
        await screen.findByRole("button", { name: "chat.tag.red" }),
      );
      await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
      expect(
        view.container.querySelector("[data-chat-marker]"),
      ).toHaveAttribute("data-tag", "red");
      if (readSameValue) {
        vi.mocked(list).mockResolvedValue({ data: [confirmed], total: 1 });
        await act(async () => invalidateConversationNavigation(scopedAddress));
      }
      vi.mocked(list).mockResolvedValue({ data: [newer], total: 1 });
      await act(async () => invalidateConversationNavigation(scopedAddress));
      expect(
        view.container.querySelector("[data-chat-marker]"),
      ).toHaveAttribute("data-tag", "green");
    },
  );
  it("does not recreate a confirmed row absent from a refreshed active list", async () => {
    vi.mocked(list).mockResolvedValue({ data: [chat], total: 1 });
    vi.mocked(update).mockResolvedValue({ ...chat, tag: "red" });
    render(<GroupRows scope={scope} />);
    await screen.findByTitle("Example");
    fireEvent.click(
      screen.getByRole("button", { name: "navigation.conversationActions" }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "chat.tag.red" }),
    );
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    vi.mocked(list).mockResolvedValue({ data: [], total: 0 });
    await act(async () => invalidateConversationNavigation(scopedAddress));
    expect(screen.queryByTitle("Example")).not.toBeInTheDocument();
  });

  it("refreshes and extends more than 200 loaded ordinary chats without server truncation", async () => {
    const all = Array.from({ length: 241 }, (_, i) => ({
      ...chat,
      id: `chat-${i}`,
    }));
    const page = (params?: { offset?: number; limit?: number }) => ({
      data: all.slice(
        params?.offset ?? 0,
        (params?.offset ?? 0) + Math.min(200, params?.limit ?? 20),
      ),
      total: all.length,
    });
    if (kind === "project")
      vi.mocked(projectChatsApi.listChats).mockImplementation(
        async (_, params) => page(params),
      );
    else
      vi.mocked(chatApi.listChats).mockImplementation(async (params) =>
        page(params),
      );
    const { result } = renderHook(() => useConversationGroup(scope, true));
    await waitFor(() => expect(result.current.chats).toHaveLength(5));
    for (let i = 0; i < 10; i++)
      await act(async () => result.current.loadMore());
    expect(result.current.chats).toHaveLength(205);
    const firstAddress = { ...scopedAddress, chatId: all[0].id };
    await act(async () =>
      publishConversationMetadata(firstAddress, {
        ...all[0],
        pinned: true,
        pin_order: 1,
      }),
    );
    expect(result.current.chats).toHaveLength(205);
    await act(async () => result.current.loadMore());
    expect(result.current.chats).toHaveLength(225);
    await act(async () => result.current.loadMore());
    expect(result.current.chats).toHaveLength(241);
    expect(result.current.hasMore).toBe(false);
    const calls = vi.mocked(list).mock.calls;
    expect(
      calls.every(
        (args) =>
          (kind === "project"
            ? (args[1] as { limit: number })
            : (args[0] as { limit: number })
          ).limit <= 200,
      ),
    ).toBe(true);
  });
});
