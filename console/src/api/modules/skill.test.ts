import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));
vi.mock("../config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));
vi.mock("../authHeaders", () => ({
  buildAuthHeaders: vi.fn(() => ({ "X-Test": "1" })),
}));

import { request } from "../request";
import { buildAuthHeaders } from "../authHeaders";
import { skillApi } from "./skill";

describe("skillApi explicit workspace identity", () => {
  afterEach(() => vi.clearAllMocks());
  it("reads and modifies only the Agent named in the URL", async () => {
    await skillApi.getSkill("my/skill", "agent/a");
    await skillApi.listSkills("agent/a");
    await skillApi.refreshSkills("agent/a");
    await skillApi.disableSkill("my/skill", "agent/a");
    await skillApi.deleteSkill("my/skill", "agent/a");
    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent%2Fa/skills/my%2Fskill",
      {},
    );
    expect(request).toHaveBeenNthCalledWith(2, "/agents/agent%2Fa/skills", {});
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent%2Fa/skills/refresh",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agents/agent%2Fa/skills/my%2Fskill/disable",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/agents/agent%2Fa/skills/my%2Fskill",
      { method: "DELETE" },
    );
  });
});

describe("skillApi pool and hub install routes", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust pool routes for pool CRUD, tags, config, upload, and download", async () => {
    const skillName = "pool/skill";

    await skillApi.listSkillPoolSkills();
    await skillApi.refreshSkillPool();
    await skillApi.createSkillPoolSkill({ name: skillName, content: "# p" });
    await skillApi.saveSkillPoolSkill({
      name: skillName,
      content: "# next",
      overwrite: true,
    });
    await skillApi.updatePoolSkillTags(skillName, ["prod"]);
    await skillApi.getPoolSkillConfig(skillName);
    await skillApi.updatePoolSkillConfig(skillName, { a: 1 });
    await skillApi.deletePoolSkillConfig(skillName);
    await skillApi.deleteSkillPoolSkill(skillName);
    await skillApi.batchDeletePoolSkills([skillName]);
    await skillApi.uploadWorkspaceSkillToPool({
      workspace_id: "default",
      skill_name: skillName,
      preview_only: true,
    });
    await skillApi.downloadSkillPoolSkill({
      skill_name: skillName,
      targets: [{ workspace_id: "default" }],
      overwrite: true,
    });

    expect(request).toHaveBeenNthCalledWith(1, "/skills/pool");
    expect(request).toHaveBeenNthCalledWith(2, "/skills/pool/refresh", {
      method: "POST",
    });
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/skills/pool/create",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/skills/pool/save",
      expect.objectContaining({ method: "PUT" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/skills/pool/pool%2Fskill/tags",
      {
        method: "PUT",
        body: JSON.stringify({ tags: ["prod"] }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/skills/pool/pool%2Fskill/config",
    );
    expect(request).toHaveBeenNthCalledWith(
      7,
      "/skills/pool/pool%2Fskill/config",
      {
        method: "PUT",
        body: JSON.stringify({ config: { a: 1 } }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      8,
      "/skills/pool/pool%2Fskill/config",
      { method: "DELETE" },
    );
    expect(request).toHaveBeenNthCalledWith(9, "/skills/pool/pool%2Fskill", {
      method: "DELETE",
    });
    expect(request).toHaveBeenNthCalledWith(10, "/skills/pool/batch-delete", {
      method: "POST",
      body: JSON.stringify([skillName]),
    });
    expect(request).toHaveBeenNthCalledWith(11, "/skills/pool/upload", {
      method: "POST",
      body: JSON.stringify({
        workspace_id: "default",
        skill_name: skillName,
        preview_only: true,
      }),
    });
    expect(request).toHaveBeenNthCalledWith(12, "/skills/pool/download", {
      method: "POST",
      body: JSON.stringify({
        skill_name: skillName,
        targets: [{ workspace_id: "default" }],
        overwrite: true,
      }),
    });
  });

  it("covers hub search, install status, cancel, and import routes without remote semantics", async () => {
    await skillApi.searchHubSkills("hello world", 10);
    await skillApi.startHubSkillInstall(
      {
        bundle_url: "https://example.test/skill.zip",
        version: "1.0.0",
        enable: true,
      },
      "agent/a",
    );
    await skillApi.importPoolSkillFromHub({
      bundle_url: "https://example.test/skill.zip",
      target_name: "local",
    });
    await skillApi.getHubSkillInstallStatus("task/1", "agent/a");
    await skillApi.cancelHubSkillInstall("task/1", "agent/a");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/skills/hub/search?q=hello%20world&limit=10",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent%2Fa/skills/hub/install/start",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/skills/pool/import",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/agents/agent%2Fa/skills/hub/install/status/task%2F1",
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/agents/agent%2Fa/skills/hub/install/cancel/task%2F1",
      { method: "POST" },
    );
  });

  it("uploads skill zip files through Rust endpoints with auth headers", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ imported: ["s"], count: 1 }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ imported: ["p"], count: 1 }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    vi.stubGlobal("fetch", fetchMock);
    const file = new File(["zip"], "skill.zip", { type: "application/zip" });

    await skillApi.uploadSkillPoolZip(file, {
      target_name: "pool",
      rename_map: { a: "b" },
    });

    expect(buildAuthHeaders).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      `/api/skills/pool/upload-zip?target_name=pool&rename_map=${encodeURIComponent(
        JSON.stringify({ a: "b" }),
      )}`,
      expect.objectContaining({
        method: "POST",
        headers: { "X-Test": "1" },
      }),
    );

    vi.unstubAllGlobals();
  });
});
