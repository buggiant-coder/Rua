import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MCPClientInfo } from "../../../api/types";
import { useMCP } from "./useMCP";

const {
  selectedAgentRef,
  listAgentMCPClientsMock,
  createAgentMCPClientMock,
  updateAgentMCPClientMock,
  toggleAgentMCPClientMock,
  deleteAgentMCPClientMock,
  applyAgentMCPMock,
  toastMock,
  tMock,
} = vi.hoisted(() => ({
  selectedAgentRef: { current: "default" as string | null },
  listAgentMCPClientsMock: vi.fn(),
  createAgentMCPClientMock: vi.fn(),
  updateAgentMCPClientMock: vi.fn(),
  toggleAgentMCPClientMock: vi.fn(),
  deleteAgentMCPClientMock: vi.fn(),
  applyAgentMCPMock: vi.fn(),
  toastMock: {
    success: vi.fn(),
    error: vi.fn(),
  },
  tMock: (key: string) => key,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: selectedAgentRef.current }),
}));

vi.mock("../../../api/modules/agents", () => ({
  agentsApi: {
    listAgentMCPClients: listAgentMCPClientsMock,
    createAgentMCPClient: createAgentMCPClientMock,
    updateAgentMCPClient: updateAgentMCPClientMock,
    toggleAgentMCPClient: toggleAgentMCPClientMock,
    deleteAgentMCPClient: deleteAgentMCPClientMock,
    applyAgentMCP: applyAgentMCPMock,
  },
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
}));

function client(overrides: Partial<MCPClientInfo>): MCPClientInfo {
  return {
    key: "docs",
    name: "Docs",
    description: "Docs MCP",
    enabled: true,
    transport: "stdio",
    url: "",
    headers: {},
    command: "mcp-docs",
    args: [],
    env: {},
    cwd: "",
    ...overrides,
  };
}

describe("useMCP", () => {
  it("does not publish an old Agent application result after switching Agents", async () => {
    let resolve!: (value: {
      status: "failed";
      saved: boolean;
      failed_servers: string[];
    }) => void;
    applyAgentMCPMock.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const { result, rerender } = renderHook(() => useMCP());
    await waitFor(() => expect(result.current.loading).toBe(false));
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.retryApplication();
    });
    selectedAgentRef.current = "agent-b";
    rerender();
    await act(async () => {
      resolve({ status: "failed", saved: true, failed_servers: ["old"] });
      await pending;
    });
    expect(result.current.application).toBeNull();
    expect(toastMock.error).not.toHaveBeenCalled();
  });
  it("coalesces repeated apply clicks while one request is pending", async () => {
    let resolve!: (value: {
      status: "applied";
      saved: boolean;
      failed_servers: string[];
    }) => void;
    applyAgentMCPMock.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const { result } = renderHook(() => useMCP());
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => {
      const first = result.current.retryApplication();
      const second = result.current.retryApplication();
      expect(applyAgentMCPMock).toHaveBeenCalledTimes(1);
      resolve({ status: "applied", saved: true, failed_servers: [] });
      await Promise.all([first, second]);
    });
    expect(result.current.applying).toBe(false);
  });
  it("reports saved application failure and retries without saving again", async () => {
    createAgentMCPClientMock.mockResolvedValueOnce(
      client({
        application: {
          status: "failed",
          saved: true,
          failed_servers: ["docs"],
        },
      }),
    );
    applyAgentMCPMock.mockResolvedValueOnce({
      status: "applied",
      saved: true,
      failed_servers: [],
    });
    const { result } = renderHook(() => useMCP());
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => {
      await result.current.createClient("docs", {
        name: "Docs",
        command: "fixture",
      });
    });
    expect(result.current.application?.status).toBe("failed");
    expect(toastMock.success).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.retryApplication();
    });
    expect(applyAgentMCPMock).toHaveBeenCalledWith("agent-a");
    expect(createAgentMCPClientMock).toHaveBeenCalledTimes(1);
    expect(result.current.application?.status).toBe("applied");
  });
  beforeEach(() => {
    vi.clearAllMocks();
    selectedAgentRef.current = "agent-a";
    listAgentMCPClientsMock.mockResolvedValue([client({ key: "docs" })]);
    createAgentMCPClientMock.mockResolvedValue(client());
    updateAgentMCPClientMock.mockResolvedValue(client());
    toggleAgentMCPClientMock.mockResolvedValue(client());
    deleteAgentMCPClientMock.mockResolvedValue({ message: "deleted" });
  });

  it("loads MCP clients from the selected agent-scoped Rust route", async () => {
    const { result } = renderHook(() => useMCP());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listAgentMCPClientsMock).toHaveBeenCalledWith("agent-a");
    expect(result.current.clients.map((item) => item.key)).toEqual(["docs"]);
    expect(result.current.loadError).toBeNull();
  });

  it("does not call the MCP API when no agent is selected", async () => {
    selectedAgentRef.current = null;

    const { result } = renderHook(() => useMCP());

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(listAgentMCPClientsMock).not.toHaveBeenCalled();

    let created = true;
    await act(async () => {
      created = await result.current.createClient("docs", {
        name: "Docs",
        command: "mcp-docs",
      });
    });

    expect(created).toBe(false);
    expect(createAgentMCPClientMock).not.toHaveBeenCalled();
  });

  it("creates and updates clients through Rust agent-scoped payload shapes", async () => {
    createAgentMCPClientMock.mockResolvedValueOnce(client({ key: "new-docs" }));
    updateAgentMCPClientMock.mockResolvedValueOnce(client({ key: "new-docs" }));
    const { result } = renderHook(() => useMCP());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let created = false;
    let updated = false;
    await act(async () => {
      created = await result.current.createClient("new-docs", {
        name: "New Docs",
        command: "npx",
        args: ["-y", "mcp-docs"],
        enabled: true,
      });
      updated = await result.current.updateClient("new-docs", {
        transport: "streamable_http",
        url: "https://mcp.example.test",
      });
    });

    expect(created).toBe(true);
    expect(updated).toBe(true);
    expect(createAgentMCPClientMock).toHaveBeenCalledWith("agent-a", {
      client_key: "new-docs",
      client: {
        name: "New Docs",
        command: "npx",
        args: ["-y", "mcp-docs"],
        enabled: true,
      },
    });
    expect(updateAgentMCPClientMock).toHaveBeenCalledWith(
      "agent-a",
      "new-docs",
      {
        transport: "streamable_http",
        url: "https://mcp.example.test",
      },
    );
    expect(listAgentMCPClientsMock).toHaveBeenCalledTimes(3);
  });

  it("toggles and deletes clients then refreshes selected-agent state", async () => {
    toggleAgentMCPClientMock.mockResolvedValueOnce(client({ key: "docs" }));
    deleteAgentMCPClientMock.mockResolvedValueOnce({ message: "deleted" });
    const { result } = renderHook(() => useMCP());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.toggleEnabled(
        client({ key: "docs", enabled: false }),
      );
      await result.current.deleteClient(client({ key: "docs" }));
    });

    expect(toggleAgentMCPClientMock).toHaveBeenCalledWith("agent-a", "docs");
    expect(deleteAgentMCPClientMock).toHaveBeenCalledWith("agent-a", "docs");
    expect(toastMock.success).toHaveBeenCalledWith("mcp.enableSuccess");
    expect(toastMock.success).toHaveBeenCalledWith("mcp.deleteSuccess");
    expect(listAgentMCPClientsMock).toHaveBeenCalledTimes(3);
  });
});
