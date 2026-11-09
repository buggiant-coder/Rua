import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { mcpApi } from "./mcp";

describe("mcpApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust root MCP CRUD and status routes", async () => {
    const createBody = {
      client_key: "local/docs",
      client: { transport: { type: "stdio", command: "docs" } },
    } as Parameters<typeof mcpApi.createMCPClient>[0];
    const updateBody = { enabled: false } as Parameters<
      typeof mcpApi.updateMCPClient
    >[1];

    await mcpApi.listMCPClients();
    await mcpApi.createMCPClient(createBody);
    await mcpApi.getMCPClient("local/docs");
    await mcpApi.updateMCPClient("local/docs", updateBody);
    await mcpApi.toggleMCPClient("local/docs");
    await mcpApi.deleteMCPClient("local/docs");
    await mcpApi.listStatus();

    expect(request).toHaveBeenNthCalledWith(1, "/mcp");
    expect(request).toHaveBeenNthCalledWith(2, "/mcp", {
      method: "POST",
      body: JSON.stringify(createBody),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/mcp/local%2Fdocs");
    expect(request).toHaveBeenNthCalledWith(4, "/mcp/local%2Fdocs", {
      method: "PUT",
      body: JSON.stringify(updateBody),
    });
    expect(request).toHaveBeenNthCalledWith(5, "/mcp/local%2Fdocs/toggle", {
      method: "PATCH",
    });
    expect(request).toHaveBeenNthCalledWith(6, "/mcp/local%2Fdocs", {
      method: "DELETE",
    });
    expect(request).toHaveBeenNthCalledWith(7, "/mcp/status");
  });

  it("encodes MCP tool and OAuth routes", async () => {
    await mcpApi.listMCPClientTools("local/docs");
    await mcpApi.toggleMCPClientTool("local/docs", "search/tool", true);
    await mcpApi.startOAuth("local/docs", {
      callback_url: "http://127.0.0.1/callback",
    } as Parameters<typeof mcpApi.startOAuth>[1]);
    await mcpApi.getOAuthStatus("local/docs");
    await mcpApi.revokeOAuth("local/docs");

    expect(request).toHaveBeenNthCalledWith(1, "/mcp/local%2Fdocs/tools");
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/mcp/local%2Fdocs/tool-toggle/search%2Ftool",
      {
        method: "PATCH",
        body: JSON.stringify({ enabled: true }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/mcp/oauth/start/local%2Fdocs",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/mcp/oauth/status/local%2Fdocs",
    );
    expect(request).toHaveBeenNthCalledWith(5, "/mcp/oauth/local%2Fdocs", {
      method: "DELETE",
    });
  });
});
