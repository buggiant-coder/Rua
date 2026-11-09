import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import type { MarketResult } from "../../../../api/modules/market";
import type { InstallQueueItem, InstallTarget } from "../useHubInstall";
import { DetailDrawer } from "./DetailDrawer";
import { QueueItem } from "./QueueItem";
import { ResultCard } from "./ResultCard";
import { TargetToggle } from "./TargetToggle";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, values?: Record<string, unknown>) => {
      if (key === "market.queueMsg.failedPrefix") {
        return `Failed: ${values?.msg ?? ""}`;
      }
      return (
        {
          "common.cancel": "Cancel",
          "market.detail.author": "Author",
          "market.detail.missing": "Missing",
          "market.detail.slug": "Slug",
          "market.detail.sourceUrl": "Source URL",
          "market.detail.title": "Skill detail",
          "market.detail.version": "Version",
          "market.install": "Install",
          "market.installed": "Installed",
          "market.noDescription": "No description",
          "market.queueMsg.timedOut": "Timed out",
          "market.retry": "Retry",
          "market.stats.downloads": "Downloads",
          "market.stats.updatedAt": "Updated at",
          "market.status.cancelled": "Cancelled",
          "market.status.failed": "Failed",
          "market.status.installing": "Installing",
          "market.status.queued": "Queued",
          "market.targetPool": "Skill pool",
          "market.targetWorkspace": "Workspace",
        }[key] ?? key
      );
    },
  }),
}));

vi.mock("@/components/RuaCard", () => ({
  RuaCard: ({
    children,
    onClick,
    onMouseEnter,
    onMouseLeave,
  }: {
    children?: ReactNode;
    onClick?: () => void;
    onMouseEnter?: () => void;
    onMouseLeave?: () => void;
  }) => (
    <article
      onClick={onClick}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
    >
      {children}
    </article>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    onClick,
    variant,
  }: {
    children?: ReactNode;
    onClick?: () => void;
    variant?: string;
  }) => (
    <button data-variant={variant} type="button" onClick={onClick}>
      {children}
    </button>
  ),
  Drawer: ({
    children,
    footer,
    onClose,
    open,
    title,
  }: {
    children?: ReactNode;
    footer?: ReactNode;
    onClose?: () => void;
    open?: boolean;
    title?: ReactNode;
  }) =>
    open ? (
      <aside role="dialog" aria-label={String(title)}>
        <button type="button" onClick={onClose}>
          Close
        </button>
        {children}
        {footer}
      </aside>
    ) : null,
  Tag: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
  Tooltip: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));

vi.mock("./SkillIcon", () => ({
  SkillIcon: ({ alt, source }: { alt?: string; source?: string }) => (
    <span data-testid="skill-icon">{alt ?? source}</span>
  ),
  sourceLabel: (source: string) => `source:${source}`,
}));

function marketResult(overrides: Partial<MarketResult> = {}): MarketResult {
  return {
    source: "github",
    slug: "writer",
    name: "Writer",
    description: "Drafts text",
    source_url: "https://example.test/writer/SKILL.md",
    version: "1.2.3",
    author: "Market Author",
    icon_url: "https://example.test/icon.png",
    stats: {
      downloads: 1234,
      updated_at: "2026-01-02T00:00:00Z",
    },
    ...overrides,
  };
}

function queueItem(
  overrides: Partial<InstallQueueItem> = {},
): InstallQueueItem {
  return {
    id: "queue-1",
    result: marketResult(),
    target: "workspace",
    status: "queued",
    message: "",
    ...overrides,
  };
}

describe("TargetToggle", () => {
  it("switches between Rust skill-pool and workspace install targets", () => {
    const onChange = vi.fn();

    render(<TargetToggle target="pool" onChange={onChange} />);

    expect(screen.getByRole("button", { name: "Skill pool" })).toHaveAttribute(
      "data-variant",
      "filled",
    );
    fireEvent.click(screen.getByRole("button", { name: "Workspace" }));

    expect(onChange).toHaveBeenCalledWith("workspace");
  });
});

describe("ResultCard", () => {
  it("opens detail on card click and installs from the hover footer without reopening detail", () => {
    const onOpenDetail = vi.fn();
    const onInstall = vi.fn();
    const onTargetChange = vi.fn();

    render(
      <ResultCard
        item={marketResult()}
        target="workspace"
        onInstall={onInstall}
        onOpenDetail={onOpenDetail}
        onTargetChange={onTargetChange}
      />,
    );

    const title = screen.getByRole("heading", { name: "Writer" });
    const card = title.closest("article")!;

    fireEvent.click(title);
    expect(onOpenDetail).toHaveBeenCalledTimes(1);

    fireEvent.mouseEnter(card);
    fireEvent.click(screen.getByRole("button", { name: "Skill pool" }));
    expect(onTargetChange).toHaveBeenCalledWith("pool");

    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalledTimes(1);
    expect(onOpenDetail).toHaveBeenCalledTimes(1);
  });

  it("renders installed market results with installed state", () => {
    render(
      <ResultCard
        item={marketResult({ description: null })}
        target="pool"
        isInstalled
        onInstall={vi.fn()}
        onOpenDetail={vi.fn()}
        onTargetChange={vi.fn()}
      />,
    );

    expect(screen.getByText("No description")).toBeVisible();
    expect(screen.getByText("Installed")).toBeVisible();
    fireEvent.mouseEnter(
      screen.getByRole("heading", { name: "Writer" }).closest("article")!,
    );
    expect(screen.getByRole("button", { name: "Installed" })).toHaveAttribute(
      "data-variant",
      "outline",
    );
  });
});

describe("QueueItem", () => {
  it("allows cancelling queued workspace installs and retrying failed installs", () => {
    const onCancel = vi.fn();
    const onRetry = vi.fn();

    const { rerender } = render(
      <QueueItem item={queueItem()} onCancel={onCancel} onRetry={onRetry} />,
    );

    expect(screen.getByText("source:github → Workspace")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledWith("queue-1");

    rerender(
      <QueueItem
        item={queueItem({
          status: "failed",
          message: "network down",
          target: "pool",
        })}
        onCancel={onCancel}
        onRetry={onRetry}
      />,
    );

    expect(screen.getByText("source:github → Skill pool")).toBeVisible();
    expect(screen.getByText("Failed: network down")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledWith("queue-1");
  });

  it("does not allow cancelling a synchronous pool install while installing", () => {
    render(
      <QueueItem
        item={queueItem({
          status: "installing",
          target: "pool",
          message: "__TIMED_OUT__",
        })}
        onCancel={vi.fn()}
        onRetry={vi.fn()}
      />,
    );

    expect(screen.getByText("Timed out")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Cancel" }),
    ).not.toBeInTheDocument();
  });
});

describe("DetailDrawer", () => {
  it("renders market metadata, formatted stats, and install target controls", () => {
    const onInstall = vi.fn();
    const onTargetChange = vi.fn();
    const onClose = vi.fn();

    render(
      <DetailDrawer
        item={marketResult()}
        target={"workspace" as InstallTarget}
        onClose={onClose}
        onInstall={onInstall}
        onTargetChange={onTargetChange}
      />,
    );

    expect(screen.getByRole("dialog", { name: "Skill detail" })).toBeVisible();
    expect(screen.getByText("Market Author")).toBeVisible();
    expect(screen.getByText("1.2.3")).toBeVisible();
    expect(
      screen.getByText("https://example.test/writer/SKILL.md"),
    ).toBeVisible();
    expect(screen.getByText("1,234")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Skill pool" }));
    expect(onTargetChange).toHaveBeenCalledWith("pool");
    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    expect(onInstall).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("stays closed without a selected market result", () => {
    render(
      <DetailDrawer
        item={null}
        target="pool"
        onClose={vi.fn()}
        onInstall={vi.fn()}
        onTargetChange={vi.fn()}
      />,
    );

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
