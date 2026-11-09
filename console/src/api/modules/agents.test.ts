import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));
vi.mock("../config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));
vi.mock("../authHeaders", () => ({
  buildAuthHeaders: vi.fn(() => ({ "X-Test": "1" })),
}));

import { buildAuthHeaders } from "../authHeaders";
import { request } from "../request";
import { agentsApi } from "./agents";

describe("agentsApi management routes", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust multi-agent management routes and bodies", async () => {
    await agentsApi.listAgents();
    await agentsApi.createAgent({
      id: "agent-1",
      name: "Agent",
    } as Parameters<typeof agentsApi.createAgent>[0]);
    await agentsApi.getAgent("agent-1");
    await agentsApi.updateAgent("agent-1", {
      name: "Next",
    } as Parameters<typeof agentsApi.updateAgent>[1]);
    await agentsApi.reorderAgents(["default", "agent-1"]);
    await agentsApi.renameAgent("agent-1", "agent-2");
    await agentsApi.toggleAgentEnabled("agent-2", false);
    await agentsApi.deleteAgent("agent-2");

    expect(request).toHaveBeenNthCalledWith(1, "/agents");
    expect(request).toHaveBeenNthCalledWith(2, "/agents", {
      method: "POST",
      body: JSON.stringify({ id: "agent-1", name: "Agent" }),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/agents/agent-1");
    expect(request).toHaveBeenNthCalledWith(4, "/agents/agent-1", {
      method: "PUT",
      body: JSON.stringify({ name: "Next" }),
    });
    expect(request).toHaveBeenNthCalledWith(5, "/agents/order", {
      method: "PUT",
      body: JSON.stringify({ agent_ids: ["default", "agent-1"] }),
    });
    expect(request).toHaveBeenNthCalledWith(6, "/agents/rename", {
      method: "POST",
      body: JSON.stringify({ old_id: "agent-1", new_id: "agent-2" }),
    });
    expect(request).toHaveBeenNthCalledWith(7, "/agents/agent-2/toggle", {
      method: "PATCH",
      body: JSON.stringify({ enabled: false }),
    });
    expect(request).toHaveBeenNthCalledWith(8, "/agents/agent-2", {
      method: "DELETE",
    });
  });
});

describe("agentsApi workspace and config routes", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust agent-scoped workspace file and prompt config routes", async () => {
    await agentsApi.listAgentFiles("agent-1");
    await agentsApi.readAgentFile("agent-1", "my file.md");
    await agentsApi.writeAgentFile("agent-1", "my file.md", "# body");
    await agentsApi.deleteAgentFile("agent-1", "folder/my file.md", true);
    await agentsApi.renameAgentFile("agent-1", "my file.md", "next.md");
    await agentsApi.getSystemPromptFiles("agent-1");
    await agentsApi.setSystemPromptFiles("agent-1", ["AGENT_MODE.md"]);
    await agentsApi.getAgentRunningConfig("agent-1");
    await agentsApi.updateAgentRunningConfig("agent-1", {
      model: "x",
    } as Parameters<typeof agentsApi.updateAgentRunningConfig>[1]);
    await agentsApi.getAgentLanguage("agent-1");
    await agentsApi.updateAgentLanguage("agent-1", "zh-CN");

    expect(request).toHaveBeenNthCalledWith(1, "/agents/agent-1/files");
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent-1/files/my%20file.md",
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent-1/files/my%20file.md",
      {
        method: "PUT",
        body: JSON.stringify({ content: "# body" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agents/agent-1/files/folder%2Fmy%20file.md?include_subdir=true",
      { method: "DELETE" },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/agents/agent-1/files/my%20file.md/rename",
      {
        method: "POST",
        body: JSON.stringify({ new_name: "next.md" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/agents/agent-1/system-prompt-files",
    );
    expect(request).toHaveBeenNthCalledWith(
      7,
      "/agents/agent-1/system-prompt-files",
      {
        method: "PUT",
        body: JSON.stringify(["AGENT_MODE.md"]),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      8,
      "/agents/agent-1/running-config",
    );
    expect(request).toHaveBeenNthCalledWith(
      9,
      "/agents/agent-1/running-config",
      {
        method: "PUT",
        body: JSON.stringify({ model: "x" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(10, "/agents/agent-1/language");
    expect(request).toHaveBeenNthCalledWith(11, "/agents/agent-1/language", {
      method: "PUT",
      body: JSON.stringify({ language: "zh-CN" }),
    });
  });

  it("normalizes agent subdir file list timestamps", async () => {
    vi.mocked(request).mockResolvedValueOnce([
      { filename: "a.md", modified_time: "2026-06-18T00:00:00Z" },
    ]);

    const files = await agentsApi.listSubdirFiles("agent-1", "folder/name");

    expect(request).toHaveBeenCalledWith(
      "/agents/agent-1/subdirs/folder%2Fname",
    );
    expect(files[0]).toMatchObject({
      filename: "a.md",
      subdir: "folder/name",
      updated_at: new Date("2026-06-18T00:00:00Z").getTime(),
    });
  });

  it("uses explicit Agent routes for running-config reset and Chat defaults", async () => {
    await agentsApi.getAgentChatRunningDefaults("agent-1");
    await agentsApi.resetAgentRunningConfig("agent-1");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent-1/running-config/chat-defaults",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent-1/running-config",
      { method: "DELETE" },
    );
  });
});

describe("agentsApi scoped tools, MCP, and skills", () => {
  afterEach(() => vi.clearAllMocks());

  it("encodes the explicit Agent and tool identifiers for every tool operation", async () => {
    await agentsApi.listAgentTools("agent/a");
    await agentsApi.toggleAgentTool("agent/a", "task_create");
    await agentsApi.updateAgentToolAsyncExecution("agent/a", "MCP/tool", false);
    await agentsApi.getAgentToolConfig("agent/a", "MCP/tool");
    await agentsApi.updateAgentToolConfig("agent/a", "MCP/tool", {
      enabled: true,
    });
    expect(request).toHaveBeenNthCalledWith(1, "/agents/agent%2Fa/tools");
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2Fa/tools/task_create/toggle",
      { method: "PATCH" },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent%2Fa/tools/MCP%2Ftool/async-execution",
      {
        method: "PATCH",
        body: JSON.stringify({ async_execution: false }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agents/agent%2Fa/tools/MCP%2Ftool/config",
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/agents/agent%2Fa/tools/MCP%2Ftool/config",
      {
        method: "PUT",
        body: JSON.stringify({ config: { enabled: true } }),
      },
    );
  });

  it("uses Rust agent-scoped tool and MCP routes", async () => {
    await agentsApi.listAgentTools("agent-1");
    await agentsApi.toggleAgentTool("agent-1", "Bash");
    await agentsApi.updateAgentToolAsyncExecution("agent-1", "Read", true);
    await agentsApi.listAgentMCPClients("agent-1");
    await agentsApi.createAgentMCPClient("agent-1", {
      client_key: "local/docs",
    } as Parameters<typeof agentsApi.createAgentMCPClient>[1]);
    await agentsApi.updateAgentMCPClient("agent-1", "local/docs", {
      enabled: false,
    } as Parameters<typeof agentsApi.updateAgentMCPClient>[2]);
    await agentsApi.toggleAgentMCPClient("agent-1", "local/docs");
    await agentsApi.listAgentMCPClientTools("agent-1", "local/docs");
    await agentsApi.toggleAgentMCPClientTool(
      "agent-1",
      "local/docs",
      "search/tool",
      true,
    );
    await agentsApi.startAgentMCPOAuth("agent-1", "local/docs", {
      callback_url: "http://127.0.0.1/callback",
    } as Parameters<typeof agentsApi.startAgentMCPOAuth>[2]);
    await agentsApi.getAgentMCPOAuthStatus("agent-1", "local/docs");
    await agentsApi.revokeAgentMCPOAuth("agent-1", "local/docs");
    await agentsApi.deleteAgentMCPClient("agent-1", "local/docs");

    expect(request).toHaveBeenNthCalledWith(1, "/agents/agent-1/tools");
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent-1/tools/Bash/toggle",
      { method: "PATCH" },
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent-1/tools/Read/async-execution",
      {
        method: "PATCH",
        body: JSON.stringify({ async_execution: true }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(4, "/agents/agent-1/mcp");
    expect(request).toHaveBeenNthCalledWith(5, "/agents/agent-1/mcp", {
      method: "POST",
      body: JSON.stringify({ client_key: "local/docs" }),
    });
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/agents/agent-1/mcp/local%2Fdocs",
      {
        method: "PUT",
        body: JSON.stringify({ enabled: false }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      7,
      "/agents/agent-1/mcp/local%2Fdocs/toggle",
      { method: "PATCH" },
    );
    expect(request).toHaveBeenNthCalledWith(
      8,
      "/agents/agent-1/mcp/local%2Fdocs/tools",
    );
    expect(request).toHaveBeenNthCalledWith(
      9,
      "/agents/agent-1/mcp/local%2Fdocs/tool-toggle/search%2Ftool",
      {
        method: "PATCH",
        body: JSON.stringify({ enabled: true }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      10,
      "/agents/agent-1/mcp/oauth/start/local%2Fdocs",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      11,
      "/agents/agent-1/mcp/oauth/status/local%2Fdocs",
    );
    expect(request).toHaveBeenNthCalledWith(
      12,
      "/agents/agent-1/mcp/oauth/local%2Fdocs",
      { method: "DELETE" },
    );
    expect(request).toHaveBeenNthCalledWith(
      13,
      "/agents/agent-1/mcp/local%2Fdocs",
      { method: "DELETE" },
    );
  });

  it("uses Rust agent-scoped skill routes and bodies", async () => {
    await agentsApi.listAgentSkills("agent-1");
    await agentsApi.refreshAgentSkills("agent-1");
    await agentsApi.createAgentSkill("agent-1", "s", "# s", { a: 1 }, true);
    await agentsApi.saveAgentSkill("agent-1", {
      name: "new",
      source_name: "old",
      content: "# next",
      overwrite: true,
    });
    await agentsApi.enableAgentSkill("agent-1", "folder/skill");
    await agentsApi.disableAgentSkill("agent-1", "folder/skill");
    await agentsApi.updateAgentSkillChannels("agent-1", "folder/skill", [
      "console",
    ]);
    await agentsApi.getAgentSkillConfig("agent-1", "folder/skill");
    await agentsApi.batchEnableAgentSkills("agent-1", ["folder/skill"]);
    await agentsApi.batchDeleteAgentSkills("agent-1", ["folder/skill"]);
    await agentsApi.deleteAgentSkill("agent-1", "folder/skill");

    expect(request).toHaveBeenNthCalledWith(1, "/agents/agent-1/skills");
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent-1/skills/refresh",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(3, "/agents/agent-1/skills", {
      method: "POST",
      body: JSON.stringify({
        name: "s",
        content: "# s",
        config: { a: 1 },
        enable: true,
      }),
    });
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agents/agent-1/skills/save",
      expect.objectContaining({ method: "PUT" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/agents/agent-1/skills/folder%2Fskill/enable",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/agents/agent-1/skills/folder%2Fskill/disable",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      7,
      "/agents/agent-1/skills/folder%2Fskill/channels",
      {
        method: "PUT",
        body: JSON.stringify(["console"]),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      8,
      "/agents/agent-1/skills/folder%2Fskill/config",
    );
    expect(request).toHaveBeenNthCalledWith(
      9,
      "/agents/agent-1/skills/batch-enable",
      {
        method: "POST",
        body: JSON.stringify(["folder/skill"]),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      10,
      "/agents/agent-1/skills/batch-delete",
      {
        method: "POST",
        body: JSON.stringify(["folder/skill"]),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      11,
      "/agents/agent-1/skills/folder%2Fskill",
      { method: "DELETE" },
    );
  });
});

describe("agentsApi scoped workspace and skill uploads", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("downloads an agent workspace zip through the scoped Rust route", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValueOnce(
        new Response("zip", {
          status: 200,
          headers: {
            "Content-Disposition": 'attachment; filename="agent.zip"',
          },
        }),
      ),
    );

    const result = await agentsApi.downloadAgentWorkspace("agent-1");

    expect(buildAuthHeaders).toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledWith(
      "/api/agents/agent-1/workspace/download",
      {
        method: "GET",
        headers: { "X-Test": "1" },
      },
    );
    expect(result.filename).toBe("agent.zip");
  });

  it("uploads agent workspace and skill zip files through scoped Rust routes", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ success: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ imported: ["s"], count: 1 }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    vi.stubGlobal("fetch", fetchMock);
    const zip = new File(["zip"], "agent.zip", { type: "application/zip" });

    await agentsApi.uploadAgentWorkspace("agent-1", zip);
    await agentsApi.uploadAgentSkill("agent-1", zip, {
      enable: true,
      target_name: "target",
      rename_map: { a: "b" },
    });

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "/api/agents/agent-1/workspace/upload",
      expect.objectContaining({
        method: "POST",
        headers: { "X-Test": "1" },
      }),
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      `/api/agents/agent-1/skills/upload?enable=true&target_name=target&rename_map=${encodeURIComponent(
        JSON.stringify({ a: "b" }),
      )}`,
      expect.objectContaining({
        method: "POST",
        headers: { "X-Test": "1" },
      }),
    );
  });
});
