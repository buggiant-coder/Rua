import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MCPClientInfo } from "../../../../api/types";
import { SharedMCPCard } from "./SharedMCPCard";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "common.cancel": "Cancel",
        "common.close": "Close",
        "common.confirm": "Confirm",
        "common.delete": "Delete",
        "common.edit": "Edit",
        "common.save": "Save",
        "sharedMcp.deleteConfirm": "Delete this shared MCP client?",
        "sharedMcp.maskedFieldHint":
          "Secret fields stay masked unless changed.",
      })[key] ?? key,
  }),
}));

vi.mock("../../../../contexts/ThemeContext", () => ({
  useTheme: () => ({ isDark: false }),
}));

vi.mock("@/components/RuaCard", () => ({
  RuaCard: ({
    actions,
    children,
    footer,
    onClick,
    onMouseEnter,
    onMouseLeave,
    selected,
    tags,
    title,
  }: {
    actions?: ReactNode[];
    children?: ReactNode;
    footer?: ReactNode;
    onClick?: () => void;
    onMouseEnter?: () => void;
    onMouseLeave?: () => void;
    selected?: boolean;
    tags?: ReactNode[];
    title?: ReactNode;
  }) => (
    <article
      data-selected={selected ? "true" : "false"}
      onClick={onClick}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
    >
      <header>
        {title}
        {tags}
        {actions}
      </header>
      {children}
      {footer}
    </article>
  ),
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    onClick,
    variant: _variant,
  }: {
    children?: ReactNode;
    onClick?: (event: { stopPropagation: () => void }) => void;
    variant?: string;
  }) => (
    <button
      type="button"
      onClick={(event) =>
        onClick?.({
          stopPropagation: () => event.stopPropagation(),
        })
      }
    >
      {children}
    </button>
  ),
  Check: ({
    checked,
    onChange,
    onClick,
  }: {
    checked?: boolean;
    onChange?: () => void;
    onClick?: (event: { stopPropagation: () => void }) => void;
  }) => (
    <input
      aria-label="selected"
      checked={checked}
      onChange={onChange}
      onClick={(event) =>
        onClick?.({
          stopPropagation: () => event.stopPropagation(),
        })
      }
      readOnly
      type="checkbox"
    />
  ),
  Modal: ({
    children,
    footer,
    open,
    title,
  }: {
    children?: ReactNode;
    footer?: ReactNode;
    open?: boolean;
    title?: ReactNode;
  }) =>
    open ? (
      <div role="dialog" aria-label={String(title)}>
        <h2>{title}</h2>
        {children}
        {footer}
      </div>
    ) : null,
  TextArea: ({
    onChange,
    value,
  }: {
    onChange?: (event: { target: { value: string } }) => void;
    value?: string;
  }) => (
    <textarea
      aria-label="json"
      value={value}
      onChange={(event) => onChange?.(event)}
    />
  ),
  Tooltip: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));

function client(overrides: Partial<MCPClientInfo> = {}): MCPClientInfo {
  return {
    key: "docs",
    name: "Docs MCP",
    description: "Docs search",
    enabled: true,
    transport: "streamable_http",
    url: "https://mcp.example.com/mcp",
    headers: { Authorization: "***" },
    command: "",
    args: [],
    env: {},
    cwd: "",
    ...overrides,
  };
}

describe("SharedMCPCard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders Rust transport type and opens a masked JSON view", () => {
    render(
      <SharedMCPCard
        client={client()}
        isHovered={false}
        onDelete={vi.fn()}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
        onUpdate={vi.fn()}
      />,
    );

    expect(screen.getByText("Docs MCP")).toBeVisible();
    expect(screen.getByText("Remote")).toBeVisible();

    fireEvent.click(screen.getByText("Docs MCP"));

    expect(screen.getByRole("dialog", { name: /Configuration/ })).toBeVisible();
    expect(
      screen.getByText("Secret fields stay masked unless changed."),
    ).toBeVisible();
    expect(screen.getByText(/"Authorization": "\*\*\*"/)).toBeVisible();
  });

  it("updates a shared MCP client from edited JSON without sending the key field", async () => {
    const onUpdate = vi.fn().mockResolvedValue(true);

    render(
      <SharedMCPCard
        client={client()}
        isHovered={false}
        onDelete={vi.fn()}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
        onUpdate={onUpdate}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.change(screen.getByLabelText("json"), {
      target: {
        value: JSON.stringify({
          key: "changed-key",
          name: "Docs MCP v2",
          enabled: false,
          transport: "sse",
          url: "https://mcp.example.com/sse",
        }),
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(onUpdate).toHaveBeenCalledWith("docs", {
        name: "Docs MCP v2",
        enabled: false,
        transport: "sse",
        url: "https://mcp.example.com/sse",
      });
    });
  });

  it("delegates delete only after confirmation", () => {
    const onDelete = vi.fn();

    render(
      <SharedMCPCard
        client={client({ key: "local", name: "Local MCP", transport: "stdio" })}
        isHovered={false}
        onDelete={onDelete}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
        onUpdate={vi.fn()}
      />,
    );

    expect(screen.getByText("Local")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onDelete).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    expect(onDelete).toHaveBeenCalledWith(
      expect.objectContaining({ key: "local" }),
    );
  });

  it("uses selectable mode as batch-selection instead of opening JSON config", () => {
    const onSelect = vi.fn();

    render(
      <SharedMCPCard
        client={client()}
        isHovered={false}
        onDelete={vi.fn()}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
        onSelect={onSelect}
        onUpdate={vi.fn()}
        selectable
        selected
      />,
    );

    expect(screen.getByLabelText("selected")).toBeChecked();
    fireEvent.click(screen.getByText("Docs MCP"));

    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ key: "docs" }),
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
