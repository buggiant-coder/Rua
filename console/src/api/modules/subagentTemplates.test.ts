import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { subagentTemplatesApi } from "./subagentTemplates";

describe("subagentTemplatesApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust subagent-template CRUD routes with optional agent visibility filter", async () => {
    await subagentTemplatesApi.list();
    await subagentTemplatesApi.list("agent-1");
    await subagentTemplatesApi.create({
      name: "Sub",
    } as Parameters<typeof subagentTemplatesApi.create>[0]);
    await subagentTemplatesApi.get("sub-1");
    await subagentTemplatesApi.update("sub-1", {
      name: "Next",
    } as Parameters<typeof subagentTemplatesApi.update>[1]);
    await subagentTemplatesApi.delete("sub-1");

    expect(request).toHaveBeenNthCalledWith(1, "/subagent-templates");
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/subagent-templates?agent_id=agent-1",
    );
    expect(request).toHaveBeenNthCalledWith(3, "/subagent-templates", {
      method: "POST",
      body: JSON.stringify({ name: "Sub" }),
    });
    expect(request).toHaveBeenNthCalledWith(4, "/subagent-templates/sub-1");
    expect(request).toHaveBeenNthCalledWith(5, "/subagent-templates/sub-1", {
      method: "PUT",
      body: JSON.stringify({ name: "Next" }),
    });
    expect(request).toHaveBeenNthCalledWith(6, "/subagent-templates/sub-1", {
      method: "DELETE",
    });
  });
});
