import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  MarketProviderInfo,
  MarketResult,
  MarketSearchResponse,
} from "../../../api/modules/market";
import { useHubSearch } from "./useHubSearch";

const { listMarketProvidersMock, searchMarketMock } = vi.hoisted(() => ({
  listMarketProvidersMock: vi.fn(),
  searchMarketMock: vi.fn(),
}));

const i18nMock = vi.hoisted(() => ({ language: "en" }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ i18n: i18nMock }),
}));

vi.mock("../../../api/modules/market", () => ({
  marketApi: {
    listMarketProviders: listMarketProvidersMock,
    searchMarket: searchMarketMock,
  },
}));

const providers: MarketProviderInfo[] = [
  { key: "github", label: "GitHub", available: true, reason: null },
  { key: "modelscope", label: "ModelScope", available: true, reason: null },
  {
    key: "disabled",
    label: "Disabled",
    available: false,
    reason: "missing token",
  },
];

function result(overrides: Partial<MarketResult>): MarketResult {
  return {
    source: "github",
    slug: "skill",
    name: "Skill",
    description: null,
    source_url: "https://example.test/skill",
    version: null,
    author: null,
    icon_url: null,
    stats: null,
    ...overrides,
  };
}

function response(results: MarketResult[]): MarketSearchResponse {
  return {
    results,
    errors: [],
    by_provider: {
      github: { has_more: false, total: results.length },
      modelscope: { has_more: false, total: 0 },
    },
  };
}

async function waitLoaded(
  resultRef: ReturnType<typeof renderHook<typeof useHubSearch>>["result"],
) {
  await waitFor(() => expect(resultRef.current.loading).toBe(false));
}

describe("useHubSearch", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    i18nMock.language = "en";
    listMarketProvidersMock.mockResolvedValue(providers);
  });

  it("restores only available selected providers and persists the cleaned set", async () => {
    localStorage.setItem(
      "rua:market:selectedProviders",
      JSON.stringify(["github", "disabled", "missing"]),
    );

    const hook = renderHook(() => useHubSearch());
    await waitLoaded(hook.result);

    expect(
      hook.result.current.providers.map((provider) => provider.key),
    ).toEqual(["github", "modelscope", "disabled"]);
    expect(Array.from(hook.result.current.selectedProviderKeys)).toEqual([
      "github",
    ]);
    expect(localStorage.getItem("rua:market:selectedProviders")).toBe(
      JSON.stringify(["github"]),
    );
  });

  it("searches Rust market providers with trimmed query, selected pages, limit, and language", async () => {
    searchMarketMock.mockResolvedValueOnce(
      response([
        result({
          slug: "rust",
          name: "Rust Skill",
          stats: { downloads: 5, category: "dev" },
        }),
      ]),
    );
    const hook = renderHook(() => useHubSearch());
    await waitLoaded(hook.result);

    act(() => hook.result.current.toggleProvider("modelscope"));
    act(() => hook.result.current.setQuery("  rust  "));
    await act(async () => {
      hook.result.current.triggerSearch();
    });
    await waitLoaded(hook.result);

    expect(searchMarketMock).toHaveBeenCalledWith({
      query: "rust",
      provider_pages: { github: 1 },
      limit: 10,
      lang: "en",
    });
    expect(hook.result.current.results.map((item) => item.name)).toEqual([
      "Rust Skill",
    ]);
    expect(hook.result.current.totalResults).toBe(1);
  });

  it("sorts and filters display results from the cached Rust market response", async () => {
    searchMarketMock.mockResolvedValueOnce(
      response([
        result({
          slug: "low",
          name: "Low Downloads",
          stats: { downloads: 2, category: "docs" },
        }),
        result({
          slug: "high",
          name: "High Downloads",
          stats: { downloads: 20, category: "dev" },
        }),
        result({
          slug: "unknown",
          name: "Unknown Downloads",
          stats: { category: "dev" },
        }),
      ]),
    );
    const hook = renderHook(() => useHubSearch());
    await waitLoaded(hook.result);

    act(() => hook.result.current.setQuery("skill"));
    await act(async () => {
      hook.result.current.triggerSearch();
    });
    await waitLoaded(hook.result);

    act(() => hook.result.current.setSortBy("downloads"));
    expect(hook.result.current.displayResults.map((item) => item.name)).toEqual(
      ["High Downloads", "Low Downloads", "Unknown Downloads"],
    );
    expect(hook.result.current.categories).toEqual(["dev", "docs"]);

    act(() => hook.result.current.setFilterCategory("dev"));
    expect(hook.result.current.displayResults.map((item) => item.name)).toEqual(
      ["High Downloads", "Unknown Downloads"],
    );
  });
});
