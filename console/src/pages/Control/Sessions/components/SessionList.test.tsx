import { fireEvent, render, screen } from "@testing-library/react";
import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";
import type { TFunction } from "i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionList } from "./SessionList";
import type { Session } from "./constants";

dayjs.extend(relativeTime);

vi.mock("@/ui", () => ({
  Btn: ({
    "aria-label": ariaLabel,
    onClick,
    variant,
  }: {
    "aria-label"?: string;
    onClick?: () => void;
    variant?: string;
  }) => (
    <button type="button" aria-label={ariaLabel} onClick={onClick}>
      {ariaLabel ?? variant}
    </button>
  ),
  Check: ({
    checked,
    onChange,
  }: {
    checked?: boolean;
    onChange?: () => void;
  }) => (
    <button aria-pressed={checked} type="button" onClick={onChange}>
      select
    </button>
  ),
  StackedList: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  Tag: ({ children }: { children: React.ReactNode }) => <span>{children}</span>,
  Tooltip: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

const t = ((key: string) =>
  ({
    "common.delete": "Delete",
    "common.edit": "Edit",
    "sessions.openChat": "Open chat",
    "sessions.columnCreatedAt": "Created",
    "sessions.columnUpdatedAt": "Updated",
    "sessions.fieldName": "Untitled",
  })[key] ?? key) as TFunction;

const sessions: Session[] = [
  {
    id: "short-id",
    user_id: "user-a",
    channel: "console",
    name: "Release notes",
    created_at: "2026-06-18T10:00:00",
    updated_at: "2026-06-19T10:00:00",
  } as Session,
  {
    id: "very-long-session-id-1234567890",
    user_id: "user-b",
    channel: "",
    name: "",
    created_at: "2026-06-17T10:00:00Z",
    updated_at: null,
  } as Session,
];

describe("SessionList", () => {
  beforeEach(() => {
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn() },
    });
  });

  it("renders Rust session rows with fallback name and default channel", () => {
    render(
      <SessionList
        sessions={sessions}
        batchMode={false}
        selectedKeys={[]}
        onSelectChange={vi.fn()}
        onOpenChat={vi.fn()}
        onEdit={vi.fn()}
        onDelete={vi.fn()}
        t={t}
      />,
    );

    expect(screen.getByText("Release notes")).toBeVisible();
    expect(screen.getByText("Untitled")).toBeVisible();
    expect(screen.getByText("console")).toBeVisible();
    expect(screen.getByText("default")).toBeVisible();
    expect(screen.getByText("very-l…7890")).toBeVisible();
  });

  it("toggles batch selection using stable chat ids", () => {
    const onSelectChange = vi.fn();

    render(
      <SessionList
        sessions={sessions}
        batchMode
        selectedKeys={["short-id"]}
        onSelectChange={onSelectChange}
        onOpenChat={vi.fn()}
        onEdit={vi.fn()}
        onDelete={vi.fn()}
        t={t}
      />,
    );

    fireEvent.click(screen.getAllByRole("button", { name: "select" })[0]);
    expect(onSelectChange).toHaveBeenCalledWith([]);

    fireEvent.click(screen.getAllByRole("button", { name: "select" })[1]);
    expect(onSelectChange).toHaveBeenCalledWith([
      "short-id",
      "very-long-session-id-1234567890",
    ]);
  });

  it("delegates open, edit, delete, and id copy actions", () => {
    const onOpenChat = vi.fn();
    const onEdit = vi.fn();
    const onDelete = vi.fn();

    render(
      <SessionList
        sessions={sessions}
        batchMode={false}
        selectedKeys={[]}
        onSelectChange={vi.fn()}
        onOpenChat={onOpenChat}
        onEdit={onEdit}
        onDelete={onDelete}
        t={t}
      />,
    );

    fireEvent.click(screen.getAllByRole("button", { name: "Open chat" })[0]);
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    fireEvent.click(screen.getAllByRole("button", { name: "Delete" })[0]);
    fireEvent.click(screen.getByText("short-id"));

    expect(onOpenChat).toHaveBeenCalledWith("short-id");
    expect(onEdit).toHaveBeenCalledWith(sessions[0]);
    expect(onDelete).toHaveBeenCalledWith("short-id");
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("short-id");
  });
});
