import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MCPClientInfo } from "../../../api/types";
import { useSharedMcp } from "./useSharedMcp";

const { listMock, createMock, updateMock, deleteMock, toastMock, tMock } =
  vi.hoisted(() => ({
    listMock: vi.fn(),
    createMock: vi.fn(),
    updateMock: vi.fn(),
    deleteMock: vi.fn(),
    toastMock: {
      success: vi.fn(),
      error: vi.fn(),
    },
    tMock: (key: string) => key,
  }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../api", () => ({
  sharedMcpApi: {
    list: listMock,
    create: createMock,
    update: updateMock,
    delete: deleteMock,
  },
}));

vi.mock("@/ui", () => ({
  toast: toastMock,
}));

function client(overrides: Partial<MCPClientInfo>): MCPClientInfo {
  return {
    key: "shared-docs",
    name: "Shared Docs",
    description: "Shared docs MCP",
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

describe("useSharedMcp", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listMock.mockResolvedValue([client({ key: "shared-docs" })]);
  });

  it("loads shared MCP clients from the Rust shared route", async () => {
    const { result } = renderHook(() => useSharedMcp());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listMock).toHaveBeenCalledTimes(1);
    expect(result.current.clients.map((item) => item.key)).toEqual([
      "shared-docs",
    ]);
  });

  it("creates and updates shared clients then refreshes shared state", async () => {
    createMock.mockResolvedValueOnce(client({ key: "new-shared" }));
    updateMock.mockResolvedValueOnce(client({ key: "new-shared" }));
    const { result } = renderHook(() => useSharedMcp());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let created = false;
    let updated = false;
    await act(async () => {
      created = await result.current.createClient("new-shared", {
        name: "New shared",
        command: "npx",
        args: ["-y", "mcp-docs"],
      });
      updated = await result.current.updateClient("new-shared", {
        enabled: false,
      });
    });

    expect(created).toBe(true);
    expect(updated).toBe(true);
    expect(createMock).toHaveBeenCalledWith("new-shared", {
      name: "New shared",
      command: "npx",
      args: ["-y", "mcp-docs"],
    });
    expect(updateMock).toHaveBeenCalledWith("new-shared", {
      enabled: false,
    });
    expect(listMock).toHaveBeenCalledTimes(3);
    expect(toastMock.success).toHaveBeenCalledWith("sharedMcp.createSuccess");
    expect(toastMock.success).toHaveBeenCalledWith("sharedMcp.updateSuccess");
  });

  it("surfaces create errors without refreshing shared state", async () => {
    createMock.mockRejectedValueOnce(new Error("duplicate shared client"));
    const { result } = renderHook(() => useSharedMcp());
    await waitFor(() => expect(result.current.loading).toBe(false));

    let created = true;
    await act(async () => {
      created = await result.current.createClient("shared-docs", {
        name: "Shared Docs",
      });
    });

    expect(created).toBe(false);
    expect(listMock).toHaveBeenCalledTimes(1);
    expect(toastMock.error).toHaveBeenCalledWith("duplicate shared client");
  });

  it("deletes shared clients and refreshes the shared list", async () => {
    deleteMock.mockResolvedValueOnce({ message: "deleted" });
    const { result } = renderHook(() => useSharedMcp());
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.deleteClient(client({ key: "shared-docs" }));
    });

    expect(deleteMock).toHaveBeenCalledWith("shared-docs");
    expect(listMock).toHaveBeenCalledTimes(2);
    expect(toastMock.success).toHaveBeenCalledWith("sharedMcp.deleteSuccess");
  });
});
