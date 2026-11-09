import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";

import ChatHistoryDrawer from "./ChatHistoryDrawer";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock("@/hooks/usePullPastBottom", () => ({
  usePullPastBottom: () => ({
    onScroll: vi.fn(),
    onWheel: vi.fn(),
    onTouchStart: vi.fn(),
    onTouchMove: vi.fn(),
    onTouchEnd: vi.fn(),
  }),
}));

vi.mock("@/ui/Dropdown", () => ({
  default: ({
    children,
    dropdownRender,
  }: {
    children: ReactNode;
    dropdownRender?: (menu: ReactNode) => ReactNode;
  }) => (
    <div>
      {children}
      {dropdownRender?.(null)}
    </div>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    icon,
    children,
    className,
    ...props
  }: {
    icon?: ReactNode;
    children?: ReactNode;
    className?: string;
    [key: string]: unknown;
  }) => (
    <button className={`rua-btn ${className ?? ""}`} {...props}>
      {icon}
      {children}
    </button>
  ),
  Drawer: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  Spinner: () => <span>loading</span>,
  TextInput: (props: Record<string, unknown>) => <input {...props} />,
  Select: ({
    value,
    options,
    onChange,
    ...props
  }: {
    value?: string;
    options?: Array<{ value: string; label: ReactNode }>;
    onChange?: (value: string) => void;
    [key: string]: unknown;
  }) => (
    <div role="combobox" data-value={value} {...props}>
      {options?.map((option) => (
        <button key={option.value} onClick={() => onChange?.(option.value)}>
          {option.label}
        </button>
      ))}
    </div>
  ),
  Tabs: ({
    items,
    activeKey,
    onChange,
  }: {
    items: Array<{ key: string; label: ReactNode }>;
    activeKey?: string;
    onChange?: (key: string) => void;
  }) => (
    <div role="tablist">
      {items.map((item) => (
        <button
          key={item.key}
          role="tab"
          aria-selected={item.key === activeKey}
          onClick={() => onChange?.(item.key)}
        >
          {item.label}
        </button>
      ))}
    </div>
  ),
  Tooltip: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));

describe("ChatHistoryDrawer filter and sort panel", () => {
  it("renders collaboration conversations without ordinary chat creation or metadata", () => {
    render(
      <ChatHistoryDrawer
        open
        title="Chats"
        conversationView="collaboration"
        onConversationViewChange={vi.fn()}
        collaborationFilters={{ sourceChatId: "", executor: "", status: "" }}
        onCollaborationFiltersChange={vi.fn()}
        chats={[
          {
            id: "collaboration-1",
            user_id: "default",
            channel: "console",
            name: "Review changes",
            created_at: "2026-09-23T10:00:00Z",
            updated_at: "2026-09-23T11:00:00Z",
            status: "running",
            tag: "red",
            pinned: true,
            collaboration: {
              source: { chat_id: "source-1" },
              executor: { kind: "acp", runner: "codex" },
              executor_agent_id: "",
              configuration: {
                desired_source_epoch: "epoch-1",
                applied_source_epoch: "epoch-1",
                status: "ready",
              },
              source_available: true,
            },
          },
        ]}
        loading={false}
        searchValue=""
        searchLabel="Search"
        searchPlaceholder="Search"
        sort={{ sort_by: "updated_at", sort_order: "desc" }}
        tagFilter={null}
        emptyLabel="Empty"
        createLabel="Create"
        loadMoreLabel="More"
        loadingMoreLabel="Loading"
        onClose={vi.fn()}
        onCreate={vi.fn()}
        onSearchChange={vi.fn()}
        onSortChange={vi.fn()}
        onTagFilterChange={vi.fn()}
        onSelect={vi.fn()}
        onRename={vi.fn()}
        onTogglePin={vi.fn()}
        onCycleTag={vi.fn()}
        onDelete={vi.fn()}
        deleteLabel={() => "Delete collaboration"}
        formatDate={() => "11:00"}
        titleForChat={(chat) => chat.name ?? chat.id}
      />,
    );

    expect(screen.queryByRole("button", { name: "Create" })).toBeNull();
    expect(screen.getByText("Review changes")).toHaveClass(
      "rua-owned-chat-list-title",
    );
    expect(screen.getByText("chat.collaborationExecutorAcp")).toBeVisible();
    expect(screen.getAllByText("chat.statusRunning").length).toBeGreaterThan(0);
    expect(screen.getByText("chat.collaborationSourceAvailable")).toBeVisible();
    expect(screen.queryByText("console")).toBeNull();
    expect(screen.queryByLabelText("chat.pin")).toBeNull();
    expect(screen.queryByLabelText("common.rename")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Delete collaboration" }),
    ).toBeVisible();
  });

  it("changes tag filtering, sort field, and direction independently", () => {
    const onTagFilterChange = vi.fn();
    const onSortChange = vi.fn();

    render(
      <ChatHistoryDrawer
        open
        title="Chats"
        conversationView="user"
        onConversationViewChange={vi.fn()}
        collaborationFilters={{ sourceChatId: "", executor: "", status: "" }}
        onCollaborationFiltersChange={vi.fn()}
        chats={[]}
        loading={false}
        searchValue=""
        searchLabel="Search"
        searchPlaceholder="Search"
        sort={{ sort_by: "updated_at", sort_order: "desc" }}
        tagFilter={null}
        emptyLabel="Empty"
        createLabel="Create"
        loadMoreLabel="More"
        loadingMoreLabel="Loading"
        onClose={vi.fn()}
        onCreate={vi.fn()}
        onSearchChange={vi.fn()}
        onSortChange={onSortChange}
        onTagFilterChange={onTagFilterChange}
        onSelect={vi.fn()}
        onRename={vi.fn()}
        onTogglePin={vi.fn()}
        onCycleTag={vi.fn()}
        onDelete={vi.fn()}
        formatDate={() => ""}
        titleForChat={() => ""}
      />,
    );

    expect(
      screen.getByRole("tab", { name: "chat.myConversations" }),
    ).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("button", { name: "Create" })).toHaveClass(
      "rua-btn",
    );
    expect(
      screen.getByRole("button", { name: "chat.filterAll" }),
    ).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "chat.tag.none" }));
    fireEvent.click(screen.getByRole("button", { name: "chat.tag.red" }));
    expect(onTagFilterChange).toHaveBeenNthCalledWith(1, "none");
    expect(onTagFilterChange).toHaveBeenNthCalledWith(2, "red");

    fireEvent.click(
      screen.getByRole("button", { name: "chat.sortFieldTitle" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "chat.sortAscending" }));
    expect(onSortChange).toHaveBeenNthCalledWith(1, {
      sort_by: "title",
      sort_order: "desc",
    });
    expect(onSortChange).toHaveBeenNthCalledWith(2, {
      sort_by: "updated_at",
      sort_order: "asc",
    });
  });

  it("opens a chat from the row while keeping row actions isolated", () => {
    const chat = {
      id: "chat-1",
      name: "First chat",
      channel: "console",
      created_at: "2026-05-19T10:00:00Z",
      updated_at: "2026-05-19T11:00:00Z",
    };
    const onSelect = vi.fn();
    const onRename = vi.fn();
    const onTogglePin = vi.fn();
    const onCycleTag = vi.fn();
    const onDelete = vi.fn();

    render(
      <ChatHistoryDrawer
        open
        title="Chats"
        chats={[chat]}
        loading={false}
        searchValue=""
        searchLabel="Search"
        searchPlaceholder="Search"
        sort={{ sort_by: "updated_at", sort_order: "desc" }}
        tagFilter={null}
        emptyLabel="Empty"
        createLabel="Create"
        loadMoreLabel="More"
        loadingMoreLabel="Loading"
        onClose={vi.fn()}
        onSearchChange={vi.fn()}
        onSortChange={vi.fn()}
        onTagFilterChange={vi.fn()}
        onSelect={onSelect}
        onRename={onRename}
        onTogglePin={onTogglePin}
        onCycleTag={onCycleTag}
        onDelete={onDelete}
        formatDate={() => "May 19"}
        titleForChat={() => "First chat"}
      />,
    );

    fireEvent.click(screen.getByText("May 19"));
    fireEvent.click(screen.getByText("channels.channelNames.console"));
    fireEvent.keyDown(screen.getByRole("link"), { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(3);
    expect(onSelect).toHaveBeenLastCalledWith(chat);

    const pinButton = screen.getByRole("button", { name: "chat.pinTooltip" });
    expect(pinButton.closest(".rua-owned-chat-list-actions")).not.toBeNull();
    expect(screen.getByRole("link")).toHaveClass(
      "rua-owned-chat-list-item--standard",
    );
    expect(
      screen.queryByRole("button", { name: "Create" }),
    ).not.toBeInTheDocument();
    fireEvent.keyDown(pinButton, { key: "Enter" });
    fireEvent.click(pinButton);
    fireEvent.click(screen.getByRole("button", { name: "chat.tagCycle" }));
    fireEvent.click(screen.getByRole("button", { name: "common.rename" }));
    fireEvent.click(screen.getByRole("button", { name: "common.delete" }));

    expect(onTogglePin).toHaveBeenCalledWith(chat);
    expect(onCycleTag).toHaveBeenCalledWith(chat);
    expect(onRename).toHaveBeenCalledWith(chat);
    expect(onDelete).toHaveBeenCalledWith(chat);
    expect(onSelect).toHaveBeenCalledTimes(3);
  });
});
