import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ActiveModelsInfo, ProviderInfo } from "../../../api/types";
import { useProviders } from "./useProviders";

const { selectedAgentRef, listProvidersMock, getActiveModelsMock } = vi.hoisted(
  () => ({
    selectedAgentRef: { current: "default" as string | null },
    listProvidersMock: vi.fn(),
    getActiveModelsMock: vi.fn(),
  }),
);

vi.mock("../../../api", () => ({
  default: {
    listProviders: listProvidersMock,
    getActiveModels: getActiveModelsMock,
  },
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: selectedAgentRef.current }),
}));

function provider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "dashscope",
    name: "DashScope",
    api_key_prefix: "sk-",
    chat_model: "qwen-plus",
    models: [],
    extra_models: [],
    is_custom: false,
    is_local: false,
    support_model_discovery: true,
    support_connection_check: true,
    freeze_url: false,
    require_api_key: true,
    api_key: "***",
    base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    generate_kwargs: {},
    ...overrides,
  };
}

const activeModels: ActiveModelsInfo = {
  default_llm: {
    provider_id: "dashscope",
    model: "qwen-plus",
  },
};

describe("useProviders", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    selectedAgentRef.current = "default";
    listProvidersMock.mockResolvedValue([provider()]);
    getActiveModelsMock.mockResolvedValue(activeModels);
  });

  it("loads provider list and Rust global active-model state", async () => {
    const { result } = renderHook(() => useProviders());

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(listProvidersMock).toHaveBeenCalledTimes(1);
    expect(getActiveModelsMock).toHaveBeenCalledWith({ scope: "global" });
    expect(result.current.providers.map((item) => item.id)).toEqual([
      "dashscope",
    ]);
    expect(result.current.activeModels).toEqual(activeModels);
    expect(result.current.error).toBeNull();
  });

  it("surfaces invalid provider responses without replacing current data", async () => {
    const { result } = renderHook(() => useProviders());
    await waitFor(() => expect(result.current.loading).toBe(false));

    listProvidersMock.mockResolvedValueOnce({ providers: [] });

    await act(async () => {
      await result.current.fetchAll();
    });

    expect(result.current.providers.map((item) => item.id)).toEqual([
      "dashscope",
    ]);
    expect(result.current.error).toBe(
      "Unexpected API response. Is VITE_API_BASE_URL configured correctly?",
    );
    expect(result.current.loading).toBe(false);
  });

  it("supports silent refresh without toggling the loading state", async () => {
    const { result } = renderHook(() => useProviders());
    await waitFor(() => expect(result.current.loading).toBe(false));

    listProvidersMock.mockResolvedValueOnce([
      provider({ id: "openrouter", name: "OpenRouter" }),
    ]);

    await act(async () => {
      await result.current.fetchAll(false);
    });

    expect(result.current.loading).toBe(false);
    expect(result.current.providers.map((item) => item.id)).toEqual([
      "openrouter",
    ]);
    expect(getActiveModelsMock).toHaveBeenLastCalledWith({ scope: "global" });
  });

  it("keeps silently refreshed providers when initial data is present", async () => {
    const initialProviders = [provider()];
    const refreshedProvider = provider({
      extra_models: [
        {
          id: "qwen-max",
          name: "Qwen Max",
          supports_multimodal: null,
          supports_image: null,
          supports_video: null,
          context_window: null,
          generate_kwargs: {},
        },
      ],
    });
    listProvidersMock.mockResolvedValueOnce([refreshedProvider]);

    const { result } = renderHook(() =>
      useProviders({
        providers: initialProviders,
        activeModels,
      }),
    );

    await act(async () => {
      await result.current.fetchAll(false);
    });

    await waitFor(() => {
      expect(result.current.providers[0].extra_models).toHaveLength(1);
    });
  });

  it("refreshes settings data when the selected agent changes", async () => {
    const { rerender, result } = renderHook(() => useProviders());
    await waitFor(() => expect(result.current.loading).toBe(false));

    listProvidersMock.mockResolvedValueOnce([
      provider({ id: "idealab", name: "Idealab" }),
    ]);
    selectedAgentRef.current = "agent-b";

    rerender();

    await waitFor(() =>
      expect(result.current.providers.map((item) => item.id)).toEqual([
        "idealab",
      ]),
    );
    expect(listProvidersMock).toHaveBeenCalledTimes(2);
    expect(getActiveModelsMock).toHaveBeenCalledTimes(2);
  });
});
