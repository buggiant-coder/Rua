import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ActiveModelsInfo, ProviderInfo } from "../../../api/types";
import ModelsPage from "./index";

const { providersRef, templatesRef, activeModelsRef, fetchAllMock } =
  vi.hoisted(() => ({
    providersRef: { current: [] as ProviderInfo[] },
    templatesRef: { current: [] as ProviderInfo[] },
    activeModelsRef: { current: null as ActiveModelsInfo | null },
    fetchAllMock: vi.fn(),
  }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) =>
      ({
        "common.more": "More",
        "common.refresh": "Refresh",
        "common.refreshPage": "Refresh page",
        "models.addProvider": "Add provider",
        "models.configure": "Configure",
        "models.free": "Free",
        "models.llmTitle": "LLM",
        "models.localProviders": "Local providers",
        "models.moreProviders": "More providers",
        "models.moreProvidersDescription": "More provider description",
        "models.models": "Models",
        "models.providersTitle": "Providers",
        "models.searchPlaceholder": "Search providers",
        "models.settings": "Settings",
        "models.sortByName": "Sort by name",
        "models.zeroConfigFreeProviders": "Ready-to-use free models",
      })[key] ??
      options?.defaultValue ??
      key,
  }),
}));

vi.mock("./useProviders", () => ({
  useProviders: () => ({
    providers: providersRef.current,
    activeModels: activeModelsRef.current,
    loading: false,
    error: null,
    fetchAll: fetchAllMock,
  }),
}));

vi.mock("../../../api/modules/provider", () => ({
  providerApi: {
    listProviderTemplates: () => Promise.resolve(templatesRef.current),
  },
}));

vi.mock("./components", () => ({
  CustomProviderModal: ({ open }: { open: boolean }) =>
    open ? <div role="dialog">custom-provider-modal</div> : null,
  LoadingState: ({ message }: { message: string }) => <div>{message}</div>,
  ModelsSection: ({ providers }: { providers: ProviderInfo[] }) => (
    <section aria-label="model-slot">
      {providers.map((provider) => (
        <span key={provider.id}>{provider.id}</span>
      ))}
    </section>
  ),
  ProviderCard: ({ provider }: { provider: ProviderInfo }) => (
    <article data-testid="provider-card" data-provider-id={provider.id}>
      {provider.name}
    </article>
  ),
  ProviderConfigModal: ({
    provider,
    open,
  }: {
    provider: ProviderInfo;
    open: boolean;
  }) =>
    open ? <div role="dialog">provider-settings:{provider.id}</div> : null,
  ModelManageModal: ({
    provider,
    open,
  }: {
    provider: ProviderInfo;
    open: boolean;
  }) => (open ? <div role="dialog">provider-models:{provider.id}</div> : null),
}));

function provider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "dashscope",
    name: "DashScope",
    api_key_prefix: "",
    chat_model: "",
    models: [],
    extra_models: [],
    is_custom: false,
    is_local: false,
    support_model_discovery: false,
    support_connection_check: false,
    freeze_url: false,
    require_api_key: true,
    api_key: "",
    base_url: "",
    generate_kwargs: {},
    ...overrides,
  };
}

function renderedProviderIds(): string[] {
  return screen
    .getAllByTestId("provider-card")
    .map((card) => card.getAttribute("data-provider-id") ?? "");
}

describe("ModelsPage provider list", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    templatesRef.current = [];
    providersRef.current = [
      provider({
        id: "zeta-remote",
        name: "Zeta Remote",
        api_key: "***",
        models: [{ id: "zeta", name: "Zeta" }],
      }),
      provider({
        id: "alpha-remote",
        name: "Alpha Remote",
        api_key: "***",
        models: [{ id: "alpha", name: "Alpha" }],
      }),
      provider({
        id: "beta-unconfigured",
        name: "Beta Unconfigured",
        api_key: "",
        models: [{ id: "beta", name: "Beta" }],
      }),
      provider({
        id: "rua-local",
        name: "Rua Local",
        is_local: true,
        require_api_key: false,
        models: [],
        extra_models: [],
      }),
    ];
  });

  it("keeps Rust local providers in their own group and sorts remote providers by readiness before name", () => {
    render(<ModelsPage />);

    expect(screen.getByText("Local providers")).toBeInTheDocument();
    expect(renderedProviderIds()).toEqual([
      "alpha-remote",
      "zeta-remote",
      "beta-unconfigured",
      "rua-local",
    ]);
  });

  it("keeps rua-local id payloads in the local group even when is_local is missing", () => {
    providersRef.current = [
      provider({
        id: "remote-ready",
        name: "Remote Ready",
        api_key: "***",
        models: [{ id: "ready", name: "Ready" }],
      }),
      provider({
        id: "rua-local",
        name: "Rua Local",
        is_local: false,
        require_api_key: true,
        api_key: "",
        models: [{ id: "qwen-local", name: "Qwen Local" }],
      }),
    ];

    render(<ModelsPage />);

    const localHeading = screen.getByText("Local providers");
    const localCard = screen
      .getAllByTestId("provider-card")
      .find((card) => card.getAttribute("data-provider-id") === "rua-local");
    expect(localCard).toBeTruthy();
    expect(
      localHeading.compareDocumentPosition(localCard as HTMLElement) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(renderedProviderIds()).toEqual(["remote-ready", "rua-local"]);
  });

  it("filters providers by Rust provider display name without hiding the model slot canary", () => {
    render(<ModelsPage />);

    fireEvent.change(screen.getByPlaceholderText("Search providers"), {
      target: { value: "local" },
    });

    expect(renderedProviderIds()).toEqual(["rua-local"]);
    expect(screen.getByLabelText("model-slot")).toHaveTextContent(
      "zeta-remote",
    );
  });

  it("toggles remote name sort inside the readiness buckets and refreshes through useProviders", () => {
    render(<ModelsPage />);

    fireEvent.click(screen.getAllByRole("button")[1]);
    expect(renderedProviderIds()).toEqual([
      "zeta-remote",
      "alpha-remote",
      "beta-unconfigured",
      "rua-local",
    ]);

    fireEvent.click(screen.getByRole("button", { name: "More" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Refresh page" }));
    expect(fetchAllMock).toHaveBeenCalledTimes(1);
  });

  it("exposes model and provider settings for zero-config free providers", () => {
    providersRef.current = [
      provider({
        id: "kilo",
        name: "Kilo Code",
        is_free_tier: true,
        require_api_key: false,
        base_url: "https://api.kilo.ai/api/gateway",
        models: [{ id: "kilo-auto/free", name: "Kilo Auto" }],
      }),
      provider({
        id: "opencode",
        name: "OpenCode",
        is_free_tier: true,
        require_api_key: false,
        base_url: "https://opencode.ai/zen/v1",
        models: [{ id: "deepseek-v4-flash-free", name: "DeepSeek V4 Flash" }],
      }),
    ];

    render(<ModelsPage />);

    expect(screen.getByText("Ready-to-use free models")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Models" })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: "Settings" })).toHaveLength(2);

    fireEvent.click(screen.getAllByRole("button", { name: "Models" })[0]);
    expect(screen.getByText("provider-models:kilo")).toBeInTheDocument();

    fireEvent.click(screen.getAllByRole("button", { name: "Settings" })[1]);
    expect(screen.getByText("provider-settings:opencode")).toBeInTheDocument();
  });

  it("keeps the free badge beside the provider name and configure action trailing", async () => {
    providersRef.current = [];
    templatesRef.current = [
      provider({
        id: "google",
        name: "Google Gemini",
        is_free_tier: true,
      }),
      provider({
        id: "groq",
        name: "Groq",
      }),
    ];

    render(<ModelsPage />);

    const freeBadge = await screen.findByText("Free");
    const providerName = screen.getByText("Google Gemini");
    const freeConfigure = freeBadge.closest("button")?.lastElementChild;

    expect(freeConfigure).toHaveTextContent("Configure");
    expect(freeBadge.parentElement).toBe(providerName.parentElement);
    expect(freeBadge.parentElement).not.toBe(freeConfigure?.parentElement);
  });
});
