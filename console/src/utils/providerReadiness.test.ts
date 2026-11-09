import { describe, expect, it } from "vitest";

import {
  isLocalOrLanScopedProvider,
  isProviderConfigured,
  isProviderReadyForModelSelection,
  isRustLocalProvider,
  providerHasModels,
  type ProviderReadinessInput,
} from "./providerReadiness";

function provider(
  overrides: Partial<ProviderReadinessInput> = {},
): ProviderReadinessInput {
  return {
    id: "dashscope",
    is_custom: false,
    is_local: false,
    require_api_key: true,
    api_key: "sk-test",
    base_url: "",
    models: [{ id: "qwen-plus" }],
    extra_models: [],
    ...overrides,
  };
}

describe("providerReadiness", () => {
  it("requires at least one model for model selection", () => {
    expect(providerHasModels(provider())).toBe(true);
    expect(providerHasModels(provider({ models: [], extra_models: [] }))).toBe(
      false,
    );
  });

  it("treats local providers as configured without api key or base url", () => {
    const local = provider({
      id: "rua-local",
      is_local: true,
      require_api_key: false,
      api_key: "",
      base_url: "",
      models: [{ id: "qwen-local" }],
    });

    expect(isProviderConfigured(local)).toBe(true);
    expect(isProviderReadyForModelSelection(local)).toBe(true);
  });

  it("treats the Rust local provider id as configured even when older payloads omit is_local", () => {
    const local = provider({
      id: "rua-local",
      is_local: false,
      require_api_key: true,
      api_key: "",
      base_url: "",
      models: [{ id: "qwen-local" }],
    });

    expect(isProviderConfigured(local)).toBe(true);
    expect(isProviderReadyForModelSelection(local)).toBe(true);
    expect(isRustLocalProvider(local)).toBe(true);
    expect(isLocalOrLanScopedProvider(local)).toBe(true);
  });

  it("identifies localhost and LAN-scoped provider URLs", () => {
    expect(
      isLocalOrLanScopedProvider(
        provider({ base_url: "http://localhost:8000" }),
      ),
    ).toBe(true);
    expect(
      isLocalOrLanScopedProvider(provider({ base_url: "127.0.0.1:11434" })),
    ).toBe(true);
    expect(
      isLocalOrLanScopedProvider(
        provider({ base_url: "http://192.168.1.11:3000" }),
      ),
    ).toBe(true);
    expect(
      isLocalOrLanScopedProvider(provider({ base_url: "http://10.0.0.7/v1" })),
    ).toBe(true);
    expect(
      isLocalOrLanScopedProvider(
        provider({ base_url: "http://172.20.0.2/v1" }),
      ),
    ).toBe(true);
    expect(
      isLocalOrLanScopedProvider(
        provider({ base_url: "http://model-box.local/v1" }),
      ),
    ).toBe(true);
  });

  it("does not mark public provider URLs as local or LAN-scoped", () => {
    expect(
      isLocalOrLanScopedProvider(
        provider({ base_url: "https://api.openai.com/v1" }),
      ),
    ).toBe(false);
    expect(
      isLocalOrLanScopedProvider(
        provider({ base_url: "https://172.32.0.2/v1" }),
      ),
    ).toBe(false);
  });

  it("keeps local providers without models out of model selection", () => {
    const local = provider({
      id: "rua-local",
      is_local: true,
      require_api_key: false,
      api_key: "",
      base_url: "",
      models: [],
      extra_models: [],
    });

    expect(isProviderConfigured(local)).toBe(true);
    expect(isProviderReadyForModelSelection(local)).toBe(false);
  });

  it("requires custom providers to have a base url and an api key when declared", () => {
    expect(
      isProviderReadyForModelSelection(
        provider({ is_custom: true, base_url: "" }),
      ),
    ).toBe(false);
    expect(
      isProviderReadyForModelSelection(
        provider({
          is_custom: true,
          base_url: "https://example.com/v1",
          require_api_key: true,
          api_key: "",
        }),
      ),
    ).toBe(false);
    expect(
      isProviderReadyForModelSelection(
        provider({
          is_custom: true,
          base_url: "https://example.com/v1",
          require_api_key: false,
          api_key: "",
        }),
      ),
    ).toBe(true);
  });

  it("requires api keys only when the provider declares that requirement", () => {
    expect(
      isProviderReadyForModelSelection(
        provider({ require_api_key: true, api_key: "" }),
      ),
    ).toBe(false);
    expect(
      isProviderReadyForModelSelection(
        provider({ require_api_key: false, api_key: "", base_url: "" }),
      ),
    ).toBe(true);
  });
});
