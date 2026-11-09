import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { sharedMcpApi } from "./sharedMcp";

describe("sharedMcpApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses shared MCP CRUD routes and encodes client keys", async () => {
    const client = {
      transport: { type: "stdio", command: "docs" },
    } as Parameters<typeof sharedMcpApi.create>[1];

    await sharedMcpApi.list();
    await sharedMcpApi.create("shared/docs", client);
    await sharedMcpApi.get("shared/docs");
    await sharedMcpApi.update("shared/docs", {
      enabled: true,
    } as Parameters<typeof sharedMcpApi.update>[1]);
    await sharedMcpApi.delete("shared/docs");

    expect(request).toHaveBeenNthCalledWith(1, "/shared-mcp");
    expect(request).toHaveBeenNthCalledWith(2, "/shared-mcp", {
      method: "POST",
      body: JSON.stringify({ client_key: "shared/docs", client }),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/shared-mcp/shared%2Fdocs");
    expect(request).toHaveBeenNthCalledWith(4, "/shared-mcp/shared%2Fdocs", {
      method: "PUT",
      body: JSON.stringify({ enabled: true }),
    });
    expect(request).toHaveBeenNthCalledWith(5, "/shared-mcp/shared%2Fdocs", {
      method: "DELETE",
    });
  });

  it("uses Rust shared/agent sync routes with explicit conflict mode", async () => {
    await sharedMcpApi.broadcast(["shared/docs"], ["agent-1"], "overwrite");
    await sharedMcpApi.sync("agent-1", ["shared/docs"], "skip");
    await sharedMcpApi.listAgentMcp("agent-1");

    expect(request).toHaveBeenNthCalledWith(1, "/shared-mcp/broadcast", {
      method: "POST",
      body: JSON.stringify({
        client_keys: ["shared/docs"],
        agent_ids: ["agent-1"],
        conflict_mode: "overwrite",
      }),
    });
    expect(request).toHaveBeenNthCalledWith(2, "/shared-mcp/sync", {
      method: "POST",
      body: JSON.stringify({
        agent_id: "agent-1",
        client_keys: ["shared/docs"],
        conflict_mode: "skip",
      }),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/mcp", {
      headers: { "X-Agent-Id": "agent-1" },
    });
  });
});
