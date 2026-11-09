import { test, expect } from "./fixtures";

type ProviderInfo = {
  id: string;
  name: string;
  chat_model: string;
  models: unknown[];
  extra_models: unknown[];
  is_custom: boolean;
  is_local: boolean;
  require_api_key: boolean;
  api_key: string;
  base_url: string;
};

type ActiveModelsInfo = {
  active_llm?: {
    provider_id: string;
    model: string;
  };
};

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

test.describe("models and providers", () => {
  test("provider registry and models page mount on the real Rust server", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = apiBase(baseURL);

    const providersResponse = await request.get(`${base}/api/models`);
    expect(providersResponse.ok()).toBeTruthy();
    const providers = (await providersResponse.json()) as ProviderInfo[];
    expect(providers.length).toBeGreaterThan(0);
    for (const provider of providers) {
      expect(provider.id).toEqual(expect.any(String));
      expect(provider.id.length).toBeGreaterThan(0);
      expect(provider.name).toEqual(expect.any(String));
      expect(provider.name.length).toBeGreaterThan(0);
      expect(provider.chat_model).toEqual(expect.any(String));
      expect(Array.isArray(provider.models)).toBeTruthy();
      expect(Array.isArray(provider.extra_models)).toBeTruthy();
      expect(typeof provider.is_custom).toBe("boolean");
      expect(typeof provider.is_local).toBe("boolean");
      expect(typeof provider.require_api_key).toBe("boolean");
      expect(typeof provider.api_key).toBe("string");
      expect(typeof provider.base_url).toBe("string");
    }

    const activeResponse = await request.get(
      `${base}/api/models/active?scope=global`,
    );
    expect(activeResponse.ok()).toBeTruthy();
    const activeModels = (await activeResponse.json()) as ActiveModelsInfo;
    expect(activeModels).toEqual(expect.any(Object));
    if (activeModels.active_llm) {
      expect(activeModels.active_llm.provider_id).toEqual(expect.any(String));
      expect(activeModels.active_llm.model).toEqual(expect.any(String));
    }

    const unknownProvider = `e2e_unknown_provider_${Date.now()}`;
    const unknownResponse = await request.put(`${base}/api/models/active`, {
      data: {
        provider_id: unknownProvider,
        model: "e2e-model",
        scope: "global",
      },
    });
    expect(unknownResponse.status()).toBe(404);
    await expect(await unknownResponse.json()).toEqual(
      expect.objectContaining({
        detail: expect.stringMatching(new RegExp(unknownProvider, "i")),
      }),
    );

    await page.goto("/models", { waitUntil: "domcontentloaded" });
    await expect(page.getByText(/Default LLM|默认 LLM/).first()).toBeVisible();
    await expect(page.getByText(/Providers|提供商/).first()).toBeVisible();

    const search = page.getByPlaceholder(/Search providers|搜索提供商/);
    await expect(search).toBeVisible();

    const searchableProvider =
      providers.find((provider) => provider.id === "rua-local") ??
      providers.find((provider) => provider.name.trim().length > 0) ??
      providers[0];
    await search.fill(searchableProvider.name);
    await expect(page.getByText(searchableProvider.name).first()).toBeVisible();
  });
});
