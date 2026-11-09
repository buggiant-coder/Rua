import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CustomProviderModal } from "./CustomProviderModal";
import { ProviderConfigModal } from "./ProviderConfigModal";
import { PROVIDER_PROTOCOLS } from "./providerProtocols";
import type { ModelInfo, ProviderInfo } from "../../../../../api/types";
import { Modal } from "@/ui";

const {
  listProviderTemplatesMock,
  createCustomProviderMock,
  configureProviderMock,
  testProviderConnectionMock,
  toastSuccessMock,
  toastErrorMock,
  toastWarningMock,
  tMock,
} = vi.hoisted(() => ({
  listProviderTemplatesMock: vi.fn(),
  createCustomProviderMock: vi.fn(),
  configureProviderMock: vi.fn(),
  testProviderConnectionMock: vi.fn(),
  toastSuccessMock: vi.fn(),
  toastErrorMock: vi.fn(),
  toastWarningMock: vi.fn(),
  tMock: vi.fn(
    (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  ),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../../../api", () => ({
  default: {
    listProviderTemplates: listProviderTemplatesMock,
    createCustomProvider: createCustomProviderMock,
    configureProvider: configureProviderMock,
    testProviderConnection: testProviderConnectionMock,
    startProviderOAuth: vi.fn(),
    getProviderOAuthStatus: vi.fn(),
  },
}));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    toast: {
      success: toastSuccessMock,
      error: toastErrorMock,
      warning: toastWarningMock,
    },
  };
});

function model(id: string, name = id): ModelInfo {
  return {
    id,
    name,
    supports_multimodal: null,
    supports_image: null,
    supports_video: null,
    context_window: null,
    generate_kwargs: {},
  };
}

function provider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "custom-openai",
    name: "Custom OpenAI",
    api_key_prefix: "sk-",
    chat_model: "OpenAIChatModel",
    models: [model("qwen-plus")],
    extra_models: [],
    is_custom: true,
    is_local: false,
    support_model_discovery: true,
    support_connection_check: true,
    freeze_url: false,
    require_api_key: false,
    api_key: "",
    base_url: "https://api.example.com/v1",
    generate_kwargs: {},
    ...overrides,
  };
}

describe("provider modals", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tMock.mockImplementation(
      (key: string, options?: { defaultValue?: string }) =>
        options?.defaultValue ?? key,
    );
    listProviderTemplatesMock.mockResolvedValue([]);
    createCustomProviderMock.mockResolvedValue(provider());
    configureProviderMock.mockResolvedValue(provider());
    testProviderConnectionMock.mockResolvedValue({
      success: true,
      message: "ok",
    });
  });

  it("offers only standard provider wire protocols", () => {
    expect(PROVIDER_PROTOCOLS.map((protocol) => protocol.value)).toEqual([
      "OpenAIChatModel",
      "OpenAIResponseModel",
      "AnthropicChatModel",
      "GeminiChatModel",
    ]);
  });

  it("creates Rust custom providers from provider templates without copying QwenPaw class details", async () => {
    const onClose = vi.fn();
    const onSaved = vi.fn();
    const template = provider({
      id: "dashscope",
      name: "DashScope",
      api_key_prefix: "sk-",
      base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
      models: [model("qwen-plus")],
      extra_models: [model("qwen-max")],
      is_custom: false,
    });
    listProviderTemplatesMock.mockResolvedValue([template]);

    render(<CustomProviderModal open onClose={onClose} onSaved={onSaved} />);

    await waitFor(() =>
      expect(listProviderTemplatesMock).toHaveBeenCalledTimes(1),
    );

    fireEvent.focus(
      screen.getByPlaceholderText("models.providerTemplatePlaceholder"),
    );
    fireEvent.click(await screen.findByText("DashScope"));

    expect(screen.getByDisplayValue("dashscope")).toBeInTheDocument();
    expect(screen.getByDisplayValue("DashScope")).toBeInTheDocument();
    expect(
      screen.getByDisplayValue(
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
      ),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "common.create" }));

    await waitFor(() => {
      expect(createCustomProviderMock).toHaveBeenCalledWith({
        id: "dashscope",
        template_id: "dashscope",
        name: "DashScope",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        api_key_prefix: "sk-",
        chat_model: "OpenAIChatModel",
        models: [model("qwen-plus"), model("qwen-max")],
      });
    });
    expect(onSaved).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(toastSuccessMock).toHaveBeenCalledWith("models.providerCreated");
  });

  it("carries QwenPaw parity metadata from provider templates into created providers", async () => {
    const onClose = vi.fn();
    const onSaved = vi.fn();
    const template = provider({
      id: "openai-response",
      name: "OpenAI (Responses API)",
      api_key_prefix: "sk-",
      api_key_prefixes: ["sk-", "sk-proj-"],
      base_url: "https://api.openai.com/v1",
      chat_model: "OpenAIResponseModel",
      models: [model("gpt-5")],
      is_custom: false,
      provider_group: "openai",
      provider_group_name: "OpenAI",
      provider_variant: "responses",
      base_url_options: [
        { label: "OpenAI", value: "https://api.openai.com/v1" },
        { label: "Azure", value: "https://example.openai.azure.com/openai" },
      ],
      thinking_param_style: "effort",
      reasoning_effort_options: ["low", "medium", "high"],
      thinking_budget_range: [1, 81920],
    });
    listProviderTemplatesMock.mockResolvedValue([template]);

    render(<CustomProviderModal open onClose={onClose} onSaved={onSaved} />);

    await waitFor(() =>
      expect(listProviderTemplatesMock).toHaveBeenCalledTimes(1),
    );
    fireEvent.focus(
      screen.getByPlaceholderText("models.providerTemplatePlaceholder"),
    );
    fireEvent.click(await screen.findByText("OpenAI (Responses API)"));
    expect(screen.getByText("models.providerEndpoint")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "common.create" }));

    await waitFor(() => {
      expect(createCustomProviderMock).toHaveBeenCalledWith({
        id: "openai-response",
        template_id: "openai-response",
        name: "OpenAI (Responses API)",
        base_url: "https://api.openai.com/v1",
        api_key_prefix: "sk-",
        api_key_prefixes: ["sk-", "sk-proj-"],
        chat_model: "OpenAIResponseModel",
        models: [model("gpt-5")],
        provider_group: "openai",
        provider_group_name: "OpenAI",
        provider_variant: "responses",
        base_url_options: [
          { label: "OpenAI", value: "https://api.openai.com/v1" },
          { label: "Azure", value: "https://example.openai.azure.com/openai" },
        ],
        thinking_param_style: "effort",
        reasoning_effort_options: ["low", "medium", "high"],
        thinking_budget_range: [1, 81920],
      });
    });
  });

  it("copies a provider adapter separately from the selected protocol", async () => {
    const template = provider({
      id: "idealab-claude",
      name: "Idealab Claude",
      base_url: "https://gateway.example.test/api/anthropic",
      chat_model: "AnthropicChatModel",
      provider_adapter: "idealab-anthropic",
      models: [model("claude-opus-4-6")],
      is_custom: false,
    });
    listProviderTemplatesMock.mockResolvedValue([template]);

    render(<CustomProviderModal open onClose={vi.fn()} onSaved={vi.fn()} />);

    fireEvent.focus(
      await screen.findByPlaceholderText("models.providerTemplatePlaceholder"),
    );
    fireEvent.click(await screen.findByText("Idealab Claude"));
    fireEvent.click(screen.getByRole("button", { name: "common.create" }));

    await waitFor(() => {
      expect(createCustomProviderMock).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "idealab-claude",
          chat_model: "AnthropicChatModel",
          provider_adapter: "idealab-anthropic",
        }),
      );
    });
  });

  it("configures providers after a successful connection preflight", async () => {
    const onClose = vi.fn();
    const onSaved = vi.fn().mockResolvedValue(undefined);

    render(
      <ProviderConfigModal
        open
        provider={provider({
          id: "openai-compatible",
          name: "OpenAI Compatible",
          generate_kwargs: { temperature: 0.2 },
        })}
        activeModels={{ default_llm: { provider_id: "other", model: "qwen" } }}
        onClose={onClose}
        onSaved={onSaved}
      />,
    );

    const save = screen.getByRole("button", { name: "models.save" });
    expect(save).toBeDisabled();

    fireEvent.change(screen.getByPlaceholderText("models.enterApiKey"), {
      target: { value: "sk-new" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "models.advancedConfig" }),
    );

    const generateConfig = await screen.findByPlaceholderText(
      "models.generateConfigPlaceholder",
    );
    fireEvent.change(generateConfig, {
      target: { value: '{ "temperature": 0.7, "top_p": 0.8 }' },
    });

    expect(save).not.toBeDisabled();
    fireEvent.click(save);

    await waitFor(() => {
      expect(testProviderConnectionMock).toHaveBeenCalledWith(
        "openai-compatible",
        {
          api_key: "sk-new",
          base_url: "https://api.example.com/v1",
          chat_model: "OpenAIChatModel",
        },
      );
      expect(configureProviderMock).toHaveBeenCalledWith("openai-compatible", {
        api_key: "sk-new",
        base_url: "https://api.example.com/v1",
        chat_model: "OpenAIChatModel",
        generate_kwargs: { temperature: 0.7, top_p: 0.8 },
      });
    });
    expect(onSaved).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(toastSuccessMock).toHaveBeenCalledWith("models.configurationSaved");
  });

  it("configures DashScope with Responses sessions and a dedicated endpoint", async () => {
    const dedicatedBaseUrl =
      "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1";

    render(
      <ProviderConfigModal
        open
        provider={provider({
          id: "dashscope",
          name: "DashScope",
          chat_model: "DashScopeChatModel",
          base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
          api_key: "sk-...masked",
          freeze_url: true,
          is_custom: true,
        })}
        activeModels={{}}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("combobox"));
    fireEvent.click(await screen.findByText("models.protocolOpenAIResponses"));
    expect(screen.getByRole("button", { name: "models.save" })).toBeEnabled();
    fireEvent.change(
      screen.getByDisplayValue(
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
      ),
      { target: { value: dedicatedBaseUrl } },
    );
    fireEvent.click(screen.getByRole("button", { name: "models.save" }));

    await waitFor(() => {
      expect(testProviderConnectionMock).toHaveBeenCalledWith("dashscope", {
        base_url: dedicatedBaseUrl,
        chat_model: "DashScopeResponseModel",
      });
      expect(configureProviderMock).toHaveBeenCalledWith("dashscope", {
        base_url: dedicatedBaseUrl,
        chat_model: "DashScopeResponseModel",
        generate_kwargs: {},
      });
    });
  });

  it("shows the redacted key and omits it from connection tests until edited", async () => {
    render(
      <ProviderConfigModal
        open
        provider={provider({
          api_key: "f101...25e8",
        })}
        activeModels={{}}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );

    const apiKeyInput = screen.getByDisplayValue("f101...25e8");
    expect(apiKeyInput).toHaveAttribute("type", "text");

    fireEvent.click(
      screen.getByRole("button", { name: "models.testConnection" }),
    );

    await waitFor(() => {
      expect(testProviderConnectionMock).toHaveBeenCalledWith("custom-openai", {
        base_url: "https://api.example.com/v1",
        chat_model: "OpenAIChatModel",
      });
    });
    expect(
      screen.getByText("models.testConnectionSuccess"),
    ).toBeInTheDocument();
  });

  it("configures providers with custom headers and multiple accepted key prefixes", async () => {
    const onClose = vi.fn();
    const onSaved = vi.fn().mockResolvedValue(undefined);

    render(
      <ProviderConfigModal
        open
        provider={provider({
          id: "github-models",
          name: "GitHub Models",
          api_key_prefix: "ghp_",
          api_key_prefixes: ["ghp_", "github_pat_"],
          custom_headers: { Authorization: "Bearer old-token" },
        })}
        activeModels={{}}
        onClose={onClose}
        onSaved={onSaved}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("models.enterApiKey"), {
      target: { value: "github_pat_new" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "models.advancedConfig" }),
    );
    const customHeaders = await screen.findByPlaceholderText(
      "models.customHeadersPlaceholder",
    );
    fireEvent.change(customHeaders, {
      target: { value: '{ "Authorization": "Bearer new-token" }' },
    });

    fireEvent.click(screen.getByRole("button", { name: "models.save" }));

    await waitFor(() => {
      expect(testProviderConnectionMock).toHaveBeenCalledWith("github-models", {
        api_key: "github_pat_new",
        base_url: "https://api.example.com/v1",
        chat_model: "OpenAIChatModel",
        custom_headers: { Authorization: "Bearer new-token" },
      });
      expect(configureProviderMock).toHaveBeenCalledWith("github-models", {
        api_key: "github_pat_new",
        base_url: "https://api.example.com/v1",
        chat_model: "OpenAIChatModel",
        generate_kwargs: {},
        custom_headers: { Authorization: "Bearer new-token" },
      });
    });
    expect(onSaved).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("asks for confirmation before saving after a failed connection preflight", async () => {
    const onClose = vi.fn();
    const onSaved = vi.fn().mockResolvedValue(undefined);
    testProviderConnectionMock.mockResolvedValue({
      success: false,
      message: "bad key",
    });

    render(
      <Modal.Provider>
        <ProviderConfigModal
          open
          provider={provider()}
          activeModels={{}}
          onClose={onClose}
          onSaved={onSaved}
        />
      </Modal.Provider>,
    );

    fireEvent.change(screen.getByPlaceholderText("models.enterApiKey"), {
      target: { value: "sk-bad" },
    });
    fireEvent.click(screen.getByRole("button", { name: "models.save" }));

    await waitFor(() => {
      expect(testProviderConnectionMock).toHaveBeenCalledTimes(1);
    });
    expect(configureProviderMock).not.toHaveBeenCalled();
    expect(
      await screen.findByText("models.providerTestFailedConfirm"),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "models.saveAnyway" }));

    await waitFor(() => {
      expect(configureProviderMock).toHaveBeenCalledWith("custom-openai", {
        api_key: "sk-bad",
        base_url: "https://api.example.com/v1",
        chat_model: "OpenAIChatModel",
        generate_kwargs: {},
      });
    });
    expect(onSaved).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("returns to editing without saving after a failed connection preflight", async () => {
    testProviderConnectionMock.mockRejectedValue(new Error("network timeout"));

    render(
      <Modal.Provider>
        <ProviderConfigModal
          open
          provider={provider()}
          activeModels={{}}
          onClose={vi.fn()}
          onSaved={vi.fn()}
        />
      </Modal.Provider>,
    );

    fireEvent.change(screen.getByPlaceholderText("models.enterApiKey"), {
      target: { value: "sk-timeout" },
    });
    fireEvent.click(screen.getByRole("button", { name: "models.save" }));

    expect(
      await screen.findByText("models.providerTestFailedConfirm"),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "models.returnToEdit" }),
    );

    await waitFor(() => {
      expect(
        screen.queryByText("models.providerTestFailedConfirm"),
      ).not.toBeInTheDocument();
    });
    expect(configureProviderMock).not.toHaveBeenCalled();
  });
});
