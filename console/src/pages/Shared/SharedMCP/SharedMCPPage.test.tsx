import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SharedMCPPage from "./index";

const {
  useSharedMcpMock,
  createClientMock,
  loadClientsMock,
  toastErrorMock,
  tMock,
} = vi.hoisted(() => ({
  useSharedMcpMock: vi.fn(),
  createClientMock: vi.fn(),
  loadClientsMock: vi.fn(),
  toastErrorMock: vi.fn(),
  tMock: vi.fn((key: string) => key),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("./useSharedMcp", () => ({
  useSharedMcp: useSharedMcpMock,
}));

vi.mock("./components", () => ({
  SharedMCPCard: ({ client }: { client: { key: string; name: string } }) => (
    <div data-testid="shared-mcp-card">{client.name || client.key}</div>
  ),
  BroadcastModal: () => null,
}));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    toast: {
      error: toastErrorMock,
      success: vi.fn(),
      warning: vi.fn(),
    },
  };
});

describe("SharedMCPPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    loadClientsMock.mockResolvedValue(undefined);
    createClientMock.mockResolvedValue(true);
    useSharedMcpMock.mockReturnValue({
      clients: [],
      loading: false,
      loadClients: loadClientsMock,
      createClient: createClientMock,
      updateClient: vi.fn(),
      deleteClient: vi.fn(),
    });
  });

  it("imports standard mcpServers JSON into Rust shared-MCP create payloads", async () => {
    render(<SharedMCPPage />);

    fireEvent.click(screen.getByRole("button", { name: "sharedMcp.create" }));

    const jsonInput = await screen.findByRole("textbox");
    fireEvent.change(jsonInput, {
      target: {
        value: JSON.stringify({
          mcpServers: {
            docs: {
              name: "Docs MCP",
              description: "Docs search",
              type: "http",
              baseUrl: "https://mcp.example.com/mcp",
              headers: { Authorization: "Bearer token" },
              enabled: false,
            },
            local: {
              command: "npx",
              args: ["-y", "@example/mcp-server"],
              env: { API_KEY: "secret" },
              cwd: "/tmp/project",
            },
          },
        }),
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "common.create" }));

    await waitFor(() => {
      expect(createClientMock).toHaveBeenCalledWith("docs", {
        name: "Docs MCP",
        description: "Docs search",
        enabled: false,
        transport: "streamable_http",
        url: "https://mcp.example.com/mcp",
        headers: { Authorization: "Bearer token" },
        command: "",
        args: [],
        env: {},
        cwd: "",
      });
      expect(createClientMock).toHaveBeenCalledWith("local", {
        name: "local",
        description: "",
        enabled: true,
        transport: "stdio",
        url: "",
        headers: {},
        command: "npx",
        args: ["-y", "@example/mcp-server"],
        env: { API_KEY: "secret" },
        cwd: "/tmp/project",
      });
    });
    expect(
      JSON.parse(localStorage.getItem("rua.sharedMcp.updatedAt")!),
    ).toEqual({
      docs: expect.any(Number),
      local: expect.any(Number),
    });
  });

  it("keeps the import modal open when any shared MCP create fails", async () => {
    createClientMock.mockResolvedValueOnce(true).mockResolvedValueOnce(false);

    render(<SharedMCPPage />);
    fireEvent.click(screen.getByRole("button", { name: "sharedMcp.create" }));

    const jsonInput = await screen.findByRole("textbox");
    fireEvent.change(jsonInput, {
      target: {
        value: JSON.stringify({
          first: { command: "one" },
          second: { command: "two" },
        }),
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.create" }));

    await waitFor(() => {
      expect(createClientMock).toHaveBeenCalledTimes(2);
    });
    expect(screen.getByText("sharedMcp.create")).toBeInTheDocument();
  });

  it("creates streamable HTTP shared MCP clients from the Rust form tab", async () => {
    render(<SharedMCPPage />);
    fireEvent.click(screen.getByRole("button", { name: "sharedMcp.create" }));
    fireEvent.click(await screen.findByRole("tab", { name: "mcp.tab.form" }));

    fireEvent.change(screen.getByPlaceholderText("mcp.form.keyPlaceholder"), {
      target: { value: " docs " },
    });
    fireEvent.change(screen.getByPlaceholderText("mcp.form.namePlaceholder"), {
      target: { value: " Docs MCP " },
    });
    fireEvent.change(
      screen.getByPlaceholderText("https://mcp.example.com/mcp"),
      {
        target: { value: " https://mcp.example.com/mcp " },
      },
    );
    fireEvent.change(
      screen.getByPlaceholderText("mcp.form.headersPlaceholder"),
      {
        target: {
          value: "Authorization=Bearer token\nIgnored line\nX-Scope= docs ",
        },
      },
    );
    fireEvent.change(
      screen.getByPlaceholderText("mcp.form.descriptionPlaceholder"),
      {
        target: { value: " Shared docs " },
      },
    );

    fireEvent.click(screen.getByRole("button", { name: "common.create" }));

    await waitFor(() => {
      expect(createClientMock).toHaveBeenCalledWith("docs", {
        name: "Docs MCP",
        description: "Shared docs",
        enabled: true,
        transport: "streamable_http",
        url: "https://mcp.example.com/mcp",
        headers: {
          Authorization: "Bearer token",
          "X-Scope": "docs",
        },
        command: "",
        args: [],
        env: {},
        cwd: "",
      });
    });
  });

  it("rejects incomplete streamable HTTP form clients before calling create", async () => {
    render(<SharedMCPPage />);
    fireEvent.click(screen.getByRole("button", { name: "sharedMcp.create" }));
    fireEvent.click(await screen.findByRole("tab", { name: "mcp.tab.form" }));

    fireEvent.change(screen.getByPlaceholderText("mcp.form.keyPlaceholder"), {
      target: { value: "docs" },
    });
    fireEvent.change(screen.getByPlaceholderText("mcp.form.namePlaceholder"), {
      target: { value: "Docs MCP" },
    });
    fireEvent.click(screen.getByRole("button", { name: "common.create" }));

    expect(createClientMock).not.toHaveBeenCalled();
    expect(toastErrorMock).toHaveBeenCalledWith("mcp.form.urlRequired");
  });
});
