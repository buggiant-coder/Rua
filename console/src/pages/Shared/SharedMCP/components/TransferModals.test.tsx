import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MCPClientInfo } from "../../../../api/types";
import { BroadcastModal } from "./BroadcastModal";
import { LoadFromSharedModal } from "./LoadFromSharedModal";
import { SyncFromAgentModal } from "./SyncFromAgentModal";
import { SyncToSharedModal } from "./SyncToSharedModal";

const { agentsApiMock, sharedMcpApiMock, toastMock } = vi.hoisted(() => ({
  agentsApiMock: {
    listAgents: vi.fn(),
  },
  sharedMcpApiMock: {
    broadcast: vi.fn(),
    list: vi.fn(),
    listAgentMcp: vi.fn(),
    sync: vi.fn(),
  },
  toastMock: {
    error: vi.fn(),
    success: vi.fn(),
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, values?: Record<string, unknown>) => {
      if (key === "sharedMcp.conflictMessage") {
        return `Conflict: ${values?.name ?? ""}`;
      }
      return (
        {
          "agent.selectAll": "Select all",
          "common.cancel": "Cancel",
          "common.confirm": "Confirm",
          "common.loading": "Loading",
          "mcp.emptyState": "No MCP clients",
          "sharedMcp.broadcastFailed": "Broadcast failed",
          "sharedMcp.broadcastSuccess": "Broadcast success",
          "sharedMcp.broadcastToAgents": "Broadcast",
          "sharedMcp.conflictTitle": "Conflict",
          "sharedMcp.emptyState": "No shared MCP clients",
          "sharedMcp.loadFromShared": "Load from shared",
          "sharedMcp.noAgentMcp": "No agent MCP clients",
          "sharedMcp.overwrite": "Overwrite",
          "sharedMcp.selectAgent": "Select agent",
          "sharedMcp.selectAgents": "Select agents",
          "sharedMcp.selectClients": "Select clients",
          "sharedMcp.selectClientsToSync": "Select clients to sync",
          "sharedMcp.skip": "Skip",
          "sharedMcp.syncFailed": "Sync failed",
          "sharedMcp.syncFromAgent": "Sync from agent",
          "sharedMcp.syncSuccess": "Sync success",
          "sharedMcp.syncToShared": "Sync to shared",
          "skills.clearSelection": "Clear selection",
          "skills.selectAll": "Select all",
        }[key] ?? key
      );
    },
  }),
}));

vi.mock("../../../../api", () => ({
  agentsApi: agentsApiMock,
  sharedMcpApi: sharedMcpApiMock,
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    disabled,
    loading,
    onClick,
    variant: _variant,
    size: _size,
  }: {
    children?: ReactNode;
    disabled?: boolean;
    loading?: boolean;
    onClick?: () => void;
    variant?: string;
    size?: string;
  }) => (
    <button disabled={disabled || loading} type="button" onClick={onClick}>
      {children}
    </button>
  ),
  Modal: ({
    children,
    footer,
    okButtonProps,
    okText,
    onClose,
    onOk,
    open,
    title,
  }: {
    children?: ReactNode;
    footer?: ReactNode;
    okButtonProps?: { disabled?: boolean; loading?: boolean };
    okText?: ReactNode;
    onClose?: () => void;
    onOk?: () => void;
    open?: boolean;
    title?: ReactNode;
  }) =>
    open ? (
      <div role="dialog" aria-label={String(title)}>
        <h2>{title}</h2>
        {children}
        {footer ?? (
          <div>
            <button type="button" onClick={onClose}>
              Cancel
            </button>
            {onOk ? (
              <button
                disabled={okButtonProps?.disabled || okButtonProps?.loading}
                type="button"
                onClick={onOk}
              >
                {okText ?? "Confirm"}
              </button>
            ) : null}
          </div>
        )}
      </div>
    ) : null,
  Select: ({
    options,
    onChange,
    placeholder,
    value,
  }: {
    options?: Array<{ label: ReactNode; value: string }>;
    onChange?: (value: string) => void;
    placeholder?: string;
    value?: string;
  }) => (
    <select
      aria-label={placeholder}
      value={value ?? ""}
      onChange={(event) => onChange?.(event.target.value)}
    >
      <option value="">{placeholder}</option>
      {(options ?? []).map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  ),
  toast: toastMock,
}));

function client(overrides: Partial<MCPClientInfo>): MCPClientInfo {
  return {
    key: "docs",
    name: "Docs MCP",
    description: "Docs search",
    enabled: true,
    transport: "streamable_http",
    url: "https://mcp.example.com/mcp",
    headers: {},
    command: "",
    args: [],
    env: {},
    cwd: "",
    ...overrides,
  };
}

describe("Shared MCP transfer modals", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    agentsApiMock.listAgents.mockResolvedValue({
      agents: [
        { id: "coder", name: "Coder" },
        { id: "default", name: "" },
      ],
    });
    sharedMcpApiMock.broadcast.mockResolvedValue({
      results: [{ status: "success", client_key: "docs", agent_id: "coder" }],
    });
    sharedMcpApiMock.list.mockResolvedValue([
      client({ key: "docs", name: "Docs MCP" }),
      client({ key: "shell", name: "Shell MCP", transport: "stdio" }),
    ]);
    sharedMcpApiMock.listAgentMcp.mockResolvedValue([
      client({ key: "agent-docs", name: "Agent Docs" }),
      client({ key: "agent-shell", name: "Agent Shell", transport: "stdio" }),
    ]);
    sharedMcpApiMock.sync.mockResolvedValue({
      results: [{ status: "success", client_key: "agent-docs" }],
    });
  });

  it("broadcasts selected shared MCP clients to selected Rust agents", async () => {
    const onClose = vi.fn();

    render(
      <BroadcastModal
        clients={[
          client({ key: "docs", name: "Docs MCP" }),
          client({ key: "shell", name: "Shell MCP" }),
        ]}
        defaultSelectedKeys={["docs"]}
        onClose={onClose}
        open
      />,
    );

    expect(await screen.findByText("Coder")).toBeVisible();
    fireEvent.click(screen.getByText("Coder"));
    fireEvent.click(screen.getByRole("button", { name: /Broadcast/ }));

    await waitFor(() => {
      expect(sharedMcpApiMock.broadcast).toHaveBeenCalledWith(
        ["docs"],
        ["coder"],
        "ask",
      );
      expect(toastMock.success).toHaveBeenCalledWith("Broadcast success");
      expect(onClose).toHaveBeenCalled();
    });
  });

  it("replays broadcast conflicts with the selected conflict mode", async () => {
    sharedMcpApiMock.broadcast
      .mockResolvedValueOnce({
        results: [
          { status: "conflict", client_key: "docs", agent_id: "coder" },
          { status: "conflict", client_key: "docs", agent_id: "coder" },
        ],
      })
      .mockResolvedValueOnce({
        results: [{ status: "success", client_key: "docs", agent_id: "coder" }],
      });
    const onClose = vi.fn();

    render(
      <BroadcastModal
        clients={[client({ key: "docs", name: "Docs MCP" })]}
        defaultSelectedKeys={["docs"]}
        onClose={onClose}
        open
      />,
    );

    fireEvent.click(await screen.findByText("Coder"));
    fireEvent.click(screen.getByRole("button", { name: /Broadcast/ }));
    expect(await screen.findByText("Conflict: docs")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Overwrite" }));

    await waitFor(() => {
      expect(sharedMcpApiMock.broadcast).toHaveBeenLastCalledWith(
        ["docs"],
        ["coder"],
        "overwrite",
      );
      expect(onClose).toHaveBeenCalled();
    });
  });

  it("loads selected shared MCP clients into one agent through broadcast", async () => {
    const onClose = vi.fn();
    const onLoaded = vi.fn();

    render(
      <LoadFromSharedModal
        agentId="coder"
        onClose={onClose}
        onLoaded={onLoaded}
        open
      />,
    );

    expect(await screen.findByText("Docs MCP")).toBeVisible();
    fireEvent.click(screen.getByText("Shell MCP"));
    fireEvent.click(screen.getByRole("button", { name: /Load from shared/ }));

    await waitFor(() => {
      expect(sharedMcpApiMock.broadcast).toHaveBeenCalledWith(
        ["shell"],
        ["coder"],
        "ask",
      );
      expect(onLoaded).toHaveBeenCalled();
      expect(onClose).toHaveBeenCalled();
    });
  });

  it("syncs selected agent MCP clients into the shared pool", async () => {
    const onClose = vi.fn();
    const onSynced = vi.fn();

    render(<SyncFromAgentModal onClose={onClose} onSynced={onSynced} open />);

    fireEvent.change(await screen.findByLabelText("Select agent"), {
      target: { value: "coder" },
    });
    expect(await screen.findByText("Agent Shell")).toBeVisible();
    fireEvent.click(screen.getByText("Agent Shell"));
    fireEvent.click(screen.getByRole("button", { name: /Sync from agent/ }));

    await waitFor(() => {
      expect(sharedMcpApiMock.listAgentMcp).toHaveBeenCalledWith("coder");
      expect(sharedMcpApiMock.sync).toHaveBeenCalledWith(
        "coder",
        ["agent-shell"],
        "ask",
      );
      expect(onSynced).toHaveBeenCalled();
      expect(onClose).toHaveBeenCalled();
    });
  });

  it("syncs selected current-agent MCP clients to the shared pool", async () => {
    const onClose = vi.fn();

    render(
      <SyncToSharedModal
        agentId="coder"
        clients={[
          client({ key: "docs", name: "Docs MCP" }),
          client({ key: "shell", name: "Shell MCP", transport: "stdio" }),
        ]}
        onClose={onClose}
        open
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Select all" }));
    fireEvent.click(screen.getByText("Docs MCP"));
    fireEvent.click(screen.getByRole("button", { name: /Sync to shared/ }));

    await waitFor(() => {
      expect(sharedMcpApiMock.sync).toHaveBeenCalledWith(
        "coder",
        ["shell"],
        "ask",
      );
      expect(onClose).toHaveBeenCalled();
    });
  });
});
