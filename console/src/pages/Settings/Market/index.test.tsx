import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MarketResult } from "../../../api/modules/market";
import { useAgentStore } from "../../../stores/agentStore";
import MarketPage from "./index";

const {
  clearCompletedMock,
  enqueueMock,
  refreshProvidersMock,
  retryMock,
  setFilterCategoryMock,
  setQueryMock,
  setSortByMock,
  toggleProviderMock,
  triggerSearchMock,
  useHubInstallMock,
  useHubSearchMock,
  cancelInstallMock,
  listSkillsMock,
} = vi.hoisted(() => ({
  clearCompletedMock: vi.fn(),
  enqueueMock: vi.fn(),
  refreshProvidersMock: vi.fn(),
  retryMock: vi.fn(),
  setFilterCategoryMock: vi.fn(),
  setQueryMock: vi.fn(),
  setSortByMock: vi.fn(),
  toggleProviderMock: vi.fn(),
  triggerSearchMock: vi.fn(),
  useHubInstallMock: vi.fn(),
  useHubSearchMock: vi.fn(),
  cancelInstallMock: vi.fn(),
  listSkillsMock: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, values?: Record<string, unknown>) => {
      const labels: Record<string, string> = {
        "common.loading": "Loading",
        "common.refresh": "Refresh",
        "common.search": "Search",
        "market.allCategories": "All categories",
        "market.clearCompleted": "Clear completed",
        "market.installQueue": "Install queue",
        "market.noResults": "No results",
        "market.searchPlaceholder": "Search market",
        "market.startTyping": "Start typing",
        "market.totalResults": `${values?.count ?? 0} results`,
        "nav.market": "Market",
      };
      return labels[key] ?? key;
    },
  }),
}));

vi.mock("lucide-react", () => ({
  ArrowUpNarrowWide: () => <span data-testid="sort-icon" />,
  MoreVertical: () => <span data-testid="more-icon" />,
  RefreshCw: () => <span data-testid="refresh-icon" />,
}));

vi.mock("@/components/PageHeader", () => ({
  PageHeader: ({
    current,
    extra,
    onRefresh,
  }: {
    current?: ReactNode;
    extra?: ReactNode;
    onRefresh?: () => void;
  }) => (
    <header>
      <h1>{current}</h1>
      {extra}
      <button type="button" onClick={onRefresh}>
        <span data-testid="refresh-icon" />
      </button>
    </header>
  ),
}));

vi.mock("@/components/EmptyState", () => ({
  EmptyState: ({
    action,
    description,
  }: {
    action?: ReactNode;
    description?: ReactNode;
  }) => (
    <div>
      <p>{description}</p>
      {action}
    </div>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    icon,
    onClick,
  }: {
    children?: ReactNode;
    icon?: ReactNode;
    onClick?: () => void;
  }) => (
    <button type="button" onClick={onClick}>
      {children ?? icon ?? "icon-button"}
    </button>
  ),
  Dropdown: ({ children }: { children?: ReactNode }) => <>{children}</>,
  Pagination: ({
    current,
    onChange,
    total,
  }: {
    current: number;
    onChange: (page: number) => void;
    total: number;
  }) => (
    <button type="button" onClick={() => onChange(current + 1)}>
      page {current} of {total}
    </button>
  ),
  Select: ({
    onChange,
    options,
    value,
  }: {
    onChange: (value: string) => void;
    options: { value: string; label: string }[];
    value: string;
  }) => (
    <select
      aria-label="category"
      value={value}
      onChange={(e) => onChange(e.target.value)}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  ),
  TextInput: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input {...props} />
  ),
  Tooltip: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));

vi.mock("./useHubSearch", () => ({
  useHubSearch: useHubSearchMock,
}));

vi.mock("./useHubInstall", () => ({
  useHubInstall: useHubInstallMock,
}));

vi.mock("../../../api/modules/skill", () => ({
  skillApi: {
    listSkills: listSkillsMock,
  },
}));

vi.mock("./components", () => ({
  AliyunConfigModal: ({
    onClose,
    onSuccess,
    open,
  }: {
    onClose: () => void;
    onSuccess: () => void;
    open: boolean;
  }) =>
    open ? (
      <section aria-label="Aliyun config">
        <button type="button" onClick={onSuccess}>
          Save Aliyun
        </button>
        <button type="button" onClick={onClose}>
          Close Aliyun
        </button>
      </section>
    ) : null,
  DetailDrawer: ({
    item,
    onInstall,
    onTargetChange,
    target,
  }: {
    item: MarketResult | null;
    onInstall: () => void;
    onTargetChange: (target: "pool" | "workspace") => void;
    target: "pool" | "workspace";
  }) =>
    item ? (
      <section aria-label="detail">
        <span>
          detail {item.name} {target}
        </span>
        <button type="button" onClick={() => onTargetChange("pool")}>
          Detail pool
        </button>
        <button type="button" onClick={onInstall}>
          Detail install
        </button>
      </section>
    ) : null,
  EmptyState: ({
    children,
    text,
  }: {
    children?: ReactNode;
    text?: ReactNode;
  }) => (
    <div>
      <p>{text}</p>
      {children}
    </div>
  ),
  QueueItem: ({
    item,
    onCancel,
    onRetry,
  }: {
    item: { id: string; result: MarketResult };
    onCancel: (id: string) => void;
    onRetry: (id: string) => void;
  }) => (
    <div>
      <span>queue {item.result.name}</span>
      <button type="button" onClick={() => onCancel(item.id)}>
        Cancel queued
      </button>
      <button type="button" onClick={() => onRetry(item.id)}>
        Retry queued
      </button>
    </div>
  ),
  ResultCard: ({
    isInstalled,
    item,
    onInstall,
    onOpenDetail,
    onTargetChange,
    target,
  }: {
    isInstalled?: boolean;
    item: MarketResult;
    onInstall: () => void;
    onOpenDetail: () => void;
    onTargetChange: (target: "pool" | "workspace") => void;
    target: "pool" | "workspace";
  }) => (
    <article>
      <h2>{item.name}</h2>
      <span>{target}</span>
      {isInstalled && <span>installed</span>}
      <button type="button" onClick={onOpenDetail}>
        Open detail
      </button>
      <button type="button" onClick={() => onTargetChange("pool")}>
        Use pool
      </button>
      <button type="button" onClick={onInstall}>
        Install {item.name}
      </button>
    </article>
  ),
}));

function marketResult(overrides: Partial<MarketResult> = {}): MarketResult {
  return {
    source: "github",
    slug: "writer",
    name: "Writer",
    description: "Drafts text",
    source_url: "https://example.test/writer/SKILL.md",
    version: "1.0.0",
    author: "Alice",
    icon_url: null,
    stats: { downloads: 10, category: "docs" },
    ...overrides,
  };
}

function installQueueItem(result: MarketResult) {
  return {
    id: "queue-1",
    result,
    target: "workspace" as const,
    status: "failed" as const,
    message: "network down",
  };
}

function mockMarketState(overrides: Record<string, unknown> = {}) {
  const item = marketResult();
  useHubSearchMock.mockReturnValue({
    categories: ["docs"],
    currentPage: 1,
    displayResults: [item],
    errors: [],
    filterCategory: "",
    globalError: null,
    goToPage: vi.fn(),
    loading: false,
    providers: [
      { key: "github", label: "GitHub", available: true, reason: null },
      {
        key: "aliyun",
        label: "Aliyun",
        available: false,
        reason: "missing credentials",
      },
    ],
    query: "writer",
    refreshProviders: refreshProvidersMock,
    results: [item],
    retry: retryMock,
    selectedProviderKeys: new Set(["github"]),
    setFilterCategory: setFilterCategoryMock,
    setQuery: setQueryMock,
    setSortBy: setSortByMock,
    sortBy: "relevance",
    toggleProvider: toggleProviderMock,
    totalPages: 2,
    totalResults: 1,
    triggerSearch: triggerSearchMock,
    ...overrides,
  });
  return item;
}

describe("MarketPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAgentStore.setState({ selectedAgent: "agent-1" });
    listSkillsMock.mockResolvedValue([]);
    useHubInstallMock.mockReturnValue({
      cancel: cancelInstallMock,
      clearCompleted: clearCompletedMock,
      enqueue: enqueueMock,
      queue: [],
      retry: retryMock,
    });
    mockMarketState();
  });

  it("wires search, refresh, provider chips, and disabled provider setup", async () => {
    render(<MarketPage />);
    await waitFor(() => {
      expect(listSkillsMock).toHaveBeenCalledWith("agent-1");
    });

    fireEvent.change(screen.getByLabelText("Search market"), {
      target: { value: "planner" },
    });
    expect(setQueryMock).toHaveBeenCalledWith("planner");

    fireEvent.keyDown(screen.getByLabelText("Search market"), { key: "Enter" });
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    fireEvent.click(screen.getByTestId("refresh-icon").closest("button")!);

    expect(triggerSearchMock).toHaveBeenCalledTimes(3);

    fireEvent.click(screen.getByRole("button", { name: "GitHub" }));
    expect(toggleProviderMock).toHaveBeenCalledWith("github");

    fireEvent.click(screen.getByRole("button", { name: "Aliyun" }));
    expect(screen.getByRole("region", { name: "Aliyun config" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Save Aliyun" }));
    expect(refreshProvidersMock).toHaveBeenCalledTimes(1);
  });

  it("loads installed Rust skills and installs a result with the card target", async () => {
    const item = mockMarketState();
    listSkillsMock.mockResolvedValueOnce([
      { name: "writer", install_meta: { source_url: item.source_url } },
    ]);

    render(<MarketPage />);

    await screen.findByText("installed");
    fireEvent.click(screen.getByRole("button", { name: "Use pool" }));
    await waitFor(() => {
      expect(screen.getByText("pool")).toBeVisible();
    });

    fireEvent.click(screen.getByRole("button", { name: "Install Writer" }));
    expect(enqueueMock).toHaveBeenCalledWith([item], "pool");
    expect(listSkillsMock).toHaveBeenCalledWith("agent-1");
  });

  it("wires detail drawer install target and install queue actions", async () => {
    const item = mockMarketState();
    useHubInstallMock.mockReturnValue({
      cancel: cancelInstallMock,
      clearCompleted: clearCompletedMock,
      enqueue: enqueueMock,
      queue: [installQueueItem(item)],
      retry: retryMock,
    });

    render(<MarketPage />);

    fireEvent.click(screen.getByRole("button", { name: "Open detail" }));
    expect(screen.getByText("detail Writer workspace")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Detail pool" }));
    await waitFor(() => {
      expect(screen.getByText("detail Writer pool")).toBeVisible();
    });
    fireEvent.click(screen.getByRole("button", { name: "Detail install" }));
    expect(enqueueMock).toHaveBeenCalledWith([item], "pool");
    expect(
      screen.queryByRole("region", { name: "detail" }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Cancel queued" }));
    fireEvent.click(screen.getByRole("button", { name: "Retry queued" }));
    fireEvent.click(screen.getByRole("button", { name: "Clear completed" }));
    expect(cancelInstallMock).toHaveBeenCalledWith("queue-1");
    expect(retryMock).toHaveBeenCalledWith("queue-1");
    expect(clearCompletedMock).toHaveBeenCalledTimes(1);
  });
});
