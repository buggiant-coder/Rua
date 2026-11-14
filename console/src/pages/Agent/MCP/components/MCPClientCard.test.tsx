import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MCPClientCard } from "./MCPClientCard";
import type { MCPClientInfo, MCPToolInfo } from "../../../../api/types";
import type { McpStatusEntry } from "../../../../api/modules/mcp";

const {
  listAgentMCPClientToolsMock,
  toggleAgentMCPClientToolMock,
  startAgentMCPOAuthMock,
  getAgentMCPOAuthStatusMock,
  revokeAgentMCPOAuthMock,
  toastSuccessMock,
  toastErrorMock,
  openOAuthAuthorizationMock,
  tMock,
} = vi.hoisted(() => ({
  listAgentMCPClientToolsMock: vi.fn(),
  toggleAgentMCPClientToolMock: vi.fn(),
  startAgentMCPOAuthMock: vi.fn(),
  getAgentMCPOAuthStatusMock: vi.fn(),
  revokeAgentMCPOAuthMock: vi.fn(),
  toastSuccessMock: vi.fn(),
  toastErrorMock: vi.fn(),
  openOAuthAuthorizationMock: vi.fn(),
  tMock: vi.fn((key: string) => key),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../../api/modules/agents", () => ({
  agentsApi: {
    listAgentMCPClientTools: listAgentMCPClientToolsMock,
    toggleAgentMCPClientTool: toggleAgentMCPClientToolMock,
    startAgentMCPOAuth: startAgentMCPOAuthMock,
    getAgentMCPOAuthStatus: getAgentMCPOAuthStatusMock,
    revokeAgentMCPOAuth: revokeAgentMCPOAuthMock,
  },
}));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    toast: {
      success: toastSuccessMock,
      error: toastErrorMock,
    },
  };
});

vi.mock("@/desktop/oauth", () => ({
  openOAuthAuthorization: openOAuthAuthorizationMock,
}));

function client(overrides: Partial<MCPClientInfo> = {}): MCPClientInfo {
  return {
    key: "docs",
    name: "Docs MCP",
    description: "Documentation tools",
    enabled: true,
    transport: "stdio",
    url: "",
    headers: {},
    command: "mcp-docs",
    args: ["--stdio"],
    env: {},
    cwd: "",
    disabled_tools: [],
    ...overrides,
  };
}

function tool(overrides: Partial<MCPToolInfo> = {}): MCPToolInfo {
  return {
    name: "search_docs",
    description: "Search docs",
    enabled: true,
    schema: { type: "object" },
    ...overrides,
  };
}

function renderCard(
  overrides: Partial<MCPClientInfo> = {},
  options: {
    status?: McpStatusEntry;
    onRefresh?: () => Promise<void>;
  } = {},
) {
  return render(
    <MCPClientCard
      client={client(overrides)}
      agentId="agent-a"
      status={options.status}
      onToggle={vi.fn()}
      onDelete={vi.fn()}
      onUpdate={vi.fn().mockResolvedValue(true)}
      onRefresh={options.onRefresh ?? vi.fn().mockResolvedValue(undefined)}
      isHovered={false}
      onMouseEnter={vi.fn()}
      onMouseLeave={vi.fn()}
    />,
  );
}

describe("MCPClientCard", () => {
  afterEach(() => vi.useRealTimers());

  beforeEach(() => {
    vi.clearAllMocks();
    listAgentMCPClientToolsMock.mockResolvedValue([
      tool({ name: "search_docs", enabled: true }),
      tool({ name: "read_doc", description: "Read doc", enabled: false }),
    ]);
    toggleAgentMCPClientToolMock.mockResolvedValue({});
    getAgentMCPOAuthStatusMock.mockResolvedValue({
      authorized: false,
      expires_at: 0,
      scope: "",
      client_id: "",
    });
    openOAuthAuthorizationMock.mockResolvedValue({
      mode: "external",
      popup: null,
    });
  });

  it("does not load tools for disabled clients and shows Rust disabled-client empty state", async () => {
    renderCard({ enabled: false });

    expect(listAgentMCPClientToolsMock).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("button", {
        name: "tools.selectionPolicy.discoverTools (0/0)",
      }),
    );

    expect(await screen.findByText("mcp.clientDisabled")).toBeInTheDocument();
    expect(listAgentMCPClientToolsMock).not.toHaveBeenCalled();
  });

  it("does not discover live tools when an enabled saved source mounts", async () => {
    renderCard();
    await Promise.resolve();
    expect(listAgentMCPClientToolsMock).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", {
        name: "tools.selectionPolicy.discoverTools (0/0)",
      }),
    ).toBeInTheDocument();
  });

  it("loads and toggles tools through selected-agent Rust MCP routes", async () => {
    renderCard();

    expect(listAgentMCPClientToolsMock).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("button", {
        name: "tools.selectionPolicy.discoverTools (0/0)",
      }),
    );
    await waitFor(() => {
      expect(listAgentMCPClientToolsMock).toHaveBeenCalledWith(
        "agent-a",
        "docs",
      );
    });
    expect(await screen.findByText("search_docs")).toBeInTheDocument();
    expect(await screen.findByText("read_doc")).toBeInTheDocument();

    const exposureToggles = screen.getAllByLabelText("mcp.toolColumnExposure");
    fireEvent.click(exposureToggles[1]);

    await waitFor(() => {
      expect(toggleAgentMCPClientToolMock).toHaveBeenCalledWith(
        "agent-a",
        "docs",
        "read_doc",
        true,
      );
    });
    expect(toastSuccessMock).toHaveBeenCalledWith("mcp.toolEnableSuccess");
  });

  it("shows whole-capability exclusion separately and prevents individual toggles", async () => {
    listAgentMCPClientToolsMock.mockResolvedValue([
      tool({
        enabled: false,
        capability: "browser_use",
        capability_enabled: false,
      }),
    ]);
    renderCard();
    fireEvent.click(
      screen.getByRole("button", {
        name: "tools.selectionPolicy.discoverTools (0/0)",
      }),
    );
    expect(
      await screen.findByText("tools.selectionPolicy.capabilityDisabled"),
    ).toBeInTheDocument();
    const toggle = screen.getByLabelText("mcp.toolColumnExposure");
    expect(toggle).toBeDisabled();
    fireEvent.click(toggle);
    expect(toggleAgentMCPClientToolMock).not.toHaveBeenCalled();
  });

  it("opens OAuth externally and completes through backend status polling", async () => {
    vi.useFakeTimers();
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    startAgentMCPOAuthMock.mockResolvedValue({
      auth_url: "https://auth.example/authorize",
      session_id: "oauth-session",
    });
    getAgentMCPOAuthStatusMock.mockResolvedValue({
      authorized: true,
      expires_at: 0,
      scope: "tools",
      client_id: "client-a",
    });
    renderCard(
      {
        transport: "streamable_http",
        url: "https://mcp.example",
      },
      {
        status: {
          status: "not_connected",
          last_error: null,
          auth: {
            kind: "oauth2",
            required: true,
            authorized: false,
            expired: false,
            expires_at: 0,
            scope: "tools",
            client_id: "client-a",
          },
        },
        onRefresh,
      },
    );

    fireEvent.click(screen.getByRole("button", { name: "mcp.oauthAuthorize" }));
    await vi.advanceTimersByTimeAsync(800);

    expect(openOAuthAuthorizationMock).toHaveBeenCalledWith(
      "https://auth.example/authorize",
      "mcp-oauth",
      "width=720,height=820",
    );
    expect(getAgentMCPOAuthStatusMock).toHaveBeenCalledWith("agent-a", "docs");
    expect(onRefresh).toHaveBeenCalled();
  });
});
