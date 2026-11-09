import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { marketApi } from "./market";

describe("marketApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust market provider and search routes", async () => {
    const payload = {
      query: "agent",
      provider_pages: { github: 1 },
      limit: 20,
      lang: "zh",
    };

    await marketApi.listMarketProviders();
    await marketApi.searchMarket(payload);

    expect(request).toHaveBeenNthCalledWith(1, "/market/providers");
    expect(request).toHaveBeenNthCalledWith(2, "/market/search", {
      method: "POST",
      body: JSON.stringify(payload),
    });
  });

  it("uses explicit Agent paths for workspace hub install tasks", async () => {
    const payload = { bundle_url: "https://example.test/skill.zip" };

    await marketApi.startHubInstall(payload, "agent-1");
    await marketApi.getHubInstallStatus("task/1", "agent-1");
    await marketApi.cancelHubInstall("task/1", "agent-1");
    await marketApi.importPoolFromHub(payload);

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/agents/agent-1/skills/hub/install/start",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify(payload),
      }),
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/agents/agent-1/skills/hub/install/status/task%2F1",
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/agents/agent-1/skills/hub/install/cancel/task%2F1",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(4, "/skills/pool/import", {
      method: "POST",
      body: JSON.stringify(payload),
    });
  });
});
