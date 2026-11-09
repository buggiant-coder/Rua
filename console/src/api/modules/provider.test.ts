import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { providerApi } from "./provider";

describe("providerApi", () => {
  afterEach(() => vi.clearAllMocks());

  it("lists providers and active models through Rust model routes", async () => {
    await providerApi.listProviders();
    await providerApi.getActiveModels();

    expect(request).toHaveBeenNthCalledWith(1, "/models");
    expect(request).toHaveBeenNthCalledWith(2, "/models/active");
  });

  it("builds active-model query strings with Rust scope and agent_id fields", async () => {
    await providerApi.getActiveModels({ scope: "effective" });
    await providerApi.getActiveModels({ agent_id: "agent/one" });
    await providerApi.getActiveModels({
      scope: "agent",
      agent_id: "agent one",
    });

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/models/active?scope=effective",
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/models/active?agent_id=agent%2Fone",
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/models/active?scope=agent&agent_id=agent+one",
    );
  });

  it("sets active model slots with the request body unchanged", async () => {
    const body = {
      provider_id: "dashscope",
      model: "qwen-plus",
      scope: "agent" as const,
      agent_id: "agent-1",
      slot: "chat" as const,
    };

    await providerApi.setActiveLlm(body);

    expect(request).toHaveBeenCalledWith("/models/active", {
      method: "PUT",
      body: JSON.stringify(body),
    });
  });

  it("keeps slash model ids opaque in CRUD and probe request bodies", async () => {
    const providerId = "open/router";
    const modelId = "gpt 5/pro";

    await providerApi.configureProvider(providerId, { api_key: "sk" });
    await providerApi.addModel(providerId, { id: modelId, name: "GPT 5" });
    await providerApi.configureModel(providerId, modelId, { enabled: true });
    await providerApi.removeModel(providerId, modelId);
    await providerApi.probeMultimodal(providerId, modelId);

    const encodedProvider = encodeURIComponent(providerId);
    expect(request).toHaveBeenNthCalledWith(
      1,
      `/models/${encodedProvider}/config`,
      expect.objectContaining({ method: "PUT" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      `/models/${encodedProvider}/models`,
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      3,
      `/models/${encodedProvider}/models/config`,
      {
        method: "PUT",
        body: JSON.stringify({ model_id: modelId, enabled: true }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      4,
      `/models/${encodedProvider}/models/delete`,
      {
        method: "POST",
        body: JSON.stringify({ model_id: modelId }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      `/models/${encodedProvider}/models/probe-multimodal`,
      {
        method: "POST",
        body: JSON.stringify({ model_id: modelId }),
      },
    );
  });

  it("covers custom-provider and discovery routes without provider OAuth assumptions", async () => {
    await providerApi.createCustomProvider({
      id: "custom",
      name: "Custom",
      base_url: "https://example.test",
      api_key: "sk",
    } as Parameters<typeof providerApi.createCustomProvider>[0]);
    await providerApi.deleteProvider("custom/provider");
    await providerApi.listProviderTemplates();
    await providerApi.testProviderConnection("custom/provider", {
      api_key: "sk",
    });
    await providerApi.testModelConnection("custom/provider", {
      model: "model-1",
    } as Parameters<typeof providerApi.testModelConnection>[1]);
    await providerApi.discoverModels("custom/provider", { api_key: "sk" });
    await providerApi.getOpenRouterSeries();
    await providerApi.discoverOpenRouterExtended({ api_key: "sk" });
    await providerApi.filterOpenRouterModels({
      query: "qwen",
    } as Parameters<typeof providerApi.filterOpenRouterModels>[0]);

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/models/custom-providers",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/models/providers/custom%2Fprovider",
      { method: "DELETE" },
    );
    expect(request).toHaveBeenNthCalledWith(3, "/models/templates");
    expect(request).toHaveBeenNthCalledWith(
      4,
      "/models/custom%2Fprovider/test",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      5,
      "/models/custom%2Fprovider/models/test",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      6,
      "/models/custom%2Fprovider/discover",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(7, "/models/openrouter/series");
    expect(request).toHaveBeenNthCalledWith(
      8,
      "/models/openrouter/discover-extended",
      expect.objectContaining({ method: "POST" }),
    );
    expect(request).toHaveBeenNthCalledWith(
      9,
      "/models/openrouter/models/filter",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("uses provider OAuth routes with encoded provider id and state", async () => {
    await providerApi.startProviderOAuth("open/router");
    await providerApi.getProviderOAuthStatus("open/router", "state with/slash");

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/providers/open%2Frouter/oauth/start",
      { method: "POST" },
    );
    expect(request).toHaveBeenNthCalledWith(
      2,
      "/providers/open%2Frouter/oauth/status?state=state%20with%2Fslash",
    );
  });
});
