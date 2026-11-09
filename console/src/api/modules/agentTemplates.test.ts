import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { agentTemplatesApi } from "./agentTemplates";

describe("agentTemplatesApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust agent-template CRUD routes", async () => {
    await agentTemplatesApi.list();
    await agentTemplatesApi.create({
      name: "Template",
    } as Parameters<typeof agentTemplatesApi.create>[0]);
    await agentTemplatesApi.get("tpl-1");
    await agentTemplatesApi.update("tpl-1", {
      name: "Next",
    } as Parameters<typeof agentTemplatesApi.update>[1]);
    await agentTemplatesApi.delete("tpl-1");

    expect(request).toHaveBeenNthCalledWith(1, "/agent-templates/");
    expect(request).toHaveBeenNthCalledWith(2, "/agent-templates/", {
      method: "POST",
      body: JSON.stringify({ name: "Template" }),
    });
    expect(request).toHaveBeenNthCalledWith(3, "/agent-templates/tpl-1");
    expect(request).toHaveBeenNthCalledWith(4, "/agent-templates/tpl-1", {
      method: "PUT",
      body: JSON.stringify({ name: "Next" }),
    });
    expect(request).toHaveBeenNthCalledWith(5, "/agent-templates/tpl-1", {
      method: "DELETE",
    });
  });
});
