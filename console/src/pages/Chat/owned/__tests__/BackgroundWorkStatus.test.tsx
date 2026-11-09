import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import BackgroundWorkStatus from "../BackgroundWorkStatus";
import { createInstance } from "i18next";
import { I18nextProvider } from "react-i18next";
import zh from "@/locales/zh.json";
import en from "@/locales/en.json";
import ja from "@/locales/ja.json";
import ru from "@/locales/ru.json";
import type { ConversationBackgroundWorkView } from "@/api/types";
import type { TaskChatAddress } from "@/api/types/task";

function agentChat(chatId: string): TaskChatAddress {
  return { scope: { kind: "agent", agent_id: "default" }, chat_id: chatId };
}

function view(
  overrides: Partial<ConversationBackgroundWorkView> = {},
): ConversationBackgroundWorkView {
  return { suppressed: false, items: [], ...overrides };
}

describe("BackgroundWorkStatus", () => {
  it.each([
    ["zh", zh, "后台任务运行中（1）", "后台任务详情", "运行中"],
    [
      "en",
      en,
      "Background work running (1)",
      "Background work details",
      "Running",
    ],
    [
      "ja",
      ja,
      "バックグラウンドで実行中（1件）",
      "バックグラウンド処理の詳細",
      "実行中",
    ],
    [
      "ru",
      ru,
      "Фоновые задачи выполняются (1)",
      "Подробности фоновых задач",
      "Выполняется",
    ],
  ] as const)(
    "translates the compact row and details in %s",
    async (lng, resource, summary, details, running) => {
      const i18n = createInstance();
      await i18n.init({
        lng,
        resources: { [lng]: { translation: resource } },
        interpolation: { escapeValue: false },
      });
      render(
        <I18nextProvider i18n={i18n}>
          <BackgroundWorkStatus
            view={view({
              items: [
                {
                  kind: "task",
                  id: "task-1",
                  status: "running",
                  chat: agentChat("chat-1"),
                },
              ],
            })}
            turnActive={false}
          />
        </I18nextProvider>,
      );
      expect(screen.getByRole("status")).toHaveAccessibleName(summary);
      expect(screen.queryByRole("list")).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: details }));
      expect(screen.getByText(running, { exact: true })).toBeVisible();
    },
  );
  it("opens the existing Task drawer from a waiting task", () => {
    const open = vi.fn();
    render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "running",
              chat: agentChat("chat-1"),
            },
          ],
        })}
        turnActive={false}
        onOpenTasks={open}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "View background Tasks" }),
    );
    expect(open).toHaveBeenCalledOnce();
  });
  it("renders nothing without a control snapshot fact", () => {
    const { container } = render(
      <BackgroundWorkStatus view={null} turnActive={false} />,
    );
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("renders nothing when the snapshot reports no pending work", () => {
    const { container } = render(
      <BackgroundWorkStatus view={view()} turnActive={false} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows an accessible waiting status while idle with pending items", () => {
    render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "running",
              chat: agentChat("chat-1"),
            },
          ],
        })}
        turnActive={false}
      />,
    );

    const status = screen.getByRole("status", {
      name: "Waiting for background work (1)",
    });
    const strip = status.closest<HTMLElement>(".rua-owned-background-work")!;
    expect(strip).toHaveAttribute("data-rua-background-work", "waiting");
    expect(strip).toHaveClass("rua-owned-background-work--waiting");
    // role="status" is an implicit polite live region; assert it is not
    // overridden with an assertive one that would interrupt the user.
    expect(status).toHaveAttribute("role", "status");
    expect(status).not.toHaveAttribute("aria-live");
    expect(within(strip).queryByRole("list")).toBeNull();
    const details = within(strip).getByRole("button", {
      name: "Background work details",
    });
    expect(details).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(details);
    expect(within(strip).getByRole("list")).toBeInTheDocument();
  });

  it("does not animate the waiting indicator", () => {
    render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "waiting",
              chat: agentChat("chat-1"),
            },
          ],
        })}
        turnActive={false}
      />,
    );
    const status = screen.getByRole("status");
    const strip = status.closest<HTMLElement>(".rua-owned-background-work")!;
    expect(status.querySelector("svg")).not.toBeNull();
    expect(strip.className).not.toMatch(/spin/);
  });

  it("differentiates waiting_approval, result_ready and delivery_failed", () => {
    const { container } = render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "task",
              id: "task-a",
              status: "waiting_approval",
              chat: agentChat("chat-1"),
            },
            {
              kind: "task",
              id: "task-b",
              status: "result_ready",
              chat: agentChat("chat-1"),
            },
            {
              kind: "collaboration",
              id: "req-c",
              status: "delivery_failed",
              chat: agentChat("chat-2"),
            },
          ],
        })}
        turnActive={false}
      />,
    );

    const statusBadge = (token: string) => {
      const badge = container.querySelector<HTMLElement>(
        `.rua-owned-background-work-item-status[data-status="${token}"]`,
      );
      expect(badge).not.toBeNull();
      return badge!;
    };
    fireEvent.click(
      screen.getByRole("button", { name: "Background work details" }),
    );
    expect(
      screen.getByRole("list", { name: "Background work items" }),
    ).toBeInTheDocument();
    const approval = statusBadge("waiting_approval");
    expect(approval).toHaveTextContent("Needs approval");
    expect(approval).toHaveClass("is-approval");
    const ready = statusBadge("result_ready");
    expect(ready).toHaveTextContent("Result ready");
    expect(ready).toHaveClass("is-ready");
    const failed = statusBadge("delivery_failed");
    expect(failed).toHaveTextContent("Delivery failed");
    expect(failed).toHaveClass("is-failed");
  });

  it("shows an unknown status token verbatim instead of inventing meaning", () => {
    const { container } = render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "blocked",
              chat: agentChat("chat-1"),
            },
          ],
        })}
        turnActive={false}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Background work details" }),
    );
    const badge = container.querySelector<HTMLElement>(
      '.rua-owned-background-work-item-status[data-status="blocked"]',
    );
    expect(badge).not.toBeNull();
    expect(badge).toHaveTextContent("blocked");
    expect(badge!).toHaveAttribute("data-differentiated", "false");
  });

  it("shows the parallel count and hides the wait list while a turn is active", () => {
    render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "running",
              chat: agentChat("chat-1"),
            },
            {
              kind: "collaboration",
              id: "req-1",
              status: "running",
              chat: agentChat("chat-2"),
            },
          ],
        })}
        turnActive
      />,
    );

    const status = screen.getByRole("status", {
      name: "Background work running in parallel (2)",
    });
    const strip = status.closest<HTMLElement>(".rua-owned-background-work")!;
    expect(strip).toHaveAttribute("data-rua-background-work", "parallel");
    expect(strip).toHaveAttribute("data-rua-background-work-count", "2");
    expect(status).not.toHaveAttribute("aria-live");
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("reports suppressed work as stopped auto-resume", () => {
    render(
      <BackgroundWorkStatus
        view={view({
          suppressed: true,
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "running",
              chat: agentChat("chat-1"),
            },
          ],
        })}
        turnActive={false}
      />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Background work details" }),
    );
    const suppressed = screen.getByText("Auto-resume stopped");
    expect(suppressed.closest(".rua-owned-background-work-note")).toHaveClass(
      "is-suppressed",
    );
    // The suppressed fact is announced, not only painted.
    expect(screen.getByRole("status")).toHaveAccessibleName(
      "Waiting for background work (1), Auto-resume stopped",
    );
  });

  it("reports a snapshot error as unavailable", () => {
    render(
      <BackgroundWorkStatus
        view={view({ error: "task store locked", suppressed: true })}
        turnActive={false}
      />,
    );
    const status = screen.getByRole("status");
    const strip = status.closest<HTMLElement>(".rua-owned-background-work")!;
    expect(strip).toHaveClass("rua-owned-background-work--error");
    expect(status).toHaveAccessibleName(
      "Background work unavailable, Auto-resume stopped, task store locked",
    );
  });

  it("keeps collaboration items navigable and task items without a route", () => {
    const onOpenChat = vi.fn();
    const { container } = render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "collaboration",
              id: "req-1",
              status: "running",
              chat: agentChat("chat-other"),
            },
            {
              kind: "collaboration",
              id: "req-self",
              status: "running",
              chat: agentChat("chat-current"),
            },
            {
              kind: "task",
              id: "task-1",
              status: "running",
              chat: agentChat("chat-other"),
            },
          ],
        })}
        turnActive={false}
        currentChatId="chat-current"
        onOpenChat={onOpenChat}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Background work details" }),
    );
    const navigable = screen.getAllByRole("button", {
      name: /Open conversation/,
    });
    expect(navigable).toHaveLength(1);
    expect(navigable[0]).toHaveAccessibleName(
      "Collaboration req-1, running, Open conversation",
    );
    expect(
      container.querySelectorAll(
        ".rua-owned-background-work-item:not(.rua-owned-background-work-item--link)",
      ),
    ).toHaveLength(2);

    fireEvent.click(navigable[0]);
    expect(onOpenChat).toHaveBeenCalledWith(agentChat("chat-other"));
  });

  it("does not offer a stop control", () => {
    render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "running",
              chat: agentChat("chat-1"),
            },
          ],
        })}
        turnActive={false}
      />,
    );
    const strip = screen
      .getByRole("status")
      .closest<HTMLElement>(".rua-owned-background-work")!;
    expect(
      within(strip).queryByRole("button", {
        name: "Stop automatic continuation",
      }),
    ).toBeNull();
  });

  it("does not expose a Stop control for background work", () => {
    render(
      <BackgroundWorkStatus
        view={view({
          items: [
            {
              kind: "task",
              id: "task-1",
              status: "running",
              chat: agentChat("chat-1"),
            },
          ],
        })}
        turnActive={false}
      />,
    );
    expect(screen.queryByRole("button", { name: /stop/i })).toBeNull();
  });

  it("hides a stale suppression flag when no background work exists", () => {
    const { container } = render(
      <BackgroundWorkStatus
        view={view({ suppressed: true })}
        turnActive={false}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
