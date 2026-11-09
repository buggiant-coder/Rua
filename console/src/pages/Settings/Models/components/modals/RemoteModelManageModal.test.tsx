import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderInfo } from "../../../../../api/types";
import { toast } from "@/ui";
import { RemoteModelManageModal } from "./RemoteModelManageModal";

const {
  addModelMock,
  configureModelMock,
  discoverModelsMock,
  probeMultimodalMock,
  testModelConnectionMock,
} = vi.hoisted(() => ({
  addModelMock: vi.fn(),
  configureModelMock: vi.fn(),
  discoverModelsMock: vi.fn(),
  probeMultimodalMock: vi.fn(),
  testModelConnectionMock: vi.fn(),
}));

vi.mock("../../../../../api", () => ({
  default: {
    addModel: addModelMock,
    configureModel: configureModelMock,
    testModelConnection: testModelConnectionMock,
    probeMultimodal: probeMultimodalMock,
    removeModel: vi.fn(),
    discoverModels: discoverModelsMock,
    getOpenRouterSeries: vi.fn().mockResolvedValue({ series: [] }),
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) => {
      if (key === "models.modelsCount") return `${options?.count} models`;
      return key;
    },
  }),
}));

vi.mock("../../../../../contexts/ThemeContext", () => ({
  useTheme: () => ({ isDark: false }),
}));

function provider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "dashscope",
    name: "DashScope",
    api_key_prefix: "sk-",
    chat_model: "DashScopeChatModel",
    models: [
      {
        id: "qwen-plus",
        name: "Qwen Plus",
        supports_multimodal: null,
        supports_image: null,
        supports_video: null,
        context_window: 131072,
        generate_kwargs: {},
        thinking_enabled: false,
        thinking_param_style: "effort",
        reasoning_effort: "high",
        reasoning_effort_options: ["low", "medium", "high"],
      },
    ],
    extra_models: [],
    is_custom: false,
    is_local: false,
    support_model_discovery: true,
    support_connection_check: true,
    freeze_url: true,
    require_api_key: true,
    api_key: "sk-***",
    base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    generate_kwargs: {},
    thinking_param_style: "effort",
    reasoning_effort_options: ["low", "medium", "high"],
    ...overrides,
  };
}

describe("RemoteModelManageModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    addModelMock.mockResolvedValue(provider());
    configureModelMock.mockResolvedValue(provider());
    discoverModelsMock.mockResolvedValue({
      success: true,
      message: "ok",
      models: [{ ...provider().models[0], id: "listed-model" }],
      added_count: 0,
    });
    testModelConnectionMock.mockResolvedValue({
      success: true,
      message: "ok",
    });
    probeMultimodalMock.mockResolvedValue({
      supports_image: null,
      supports_video: null,
      supports_multimodal: null,
      message: null,
    });
  });

  it("saves model config without model-level thinking defaults", async () => {
    const onSaved = vi.fn().mockResolvedValue(undefined);

    render(
      <RemoteModelManageModal
        provider={provider()}
        open
        onClose={vi.fn()}
        onSaved={onSaved}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "models.modelSettings" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "200K" }));
    fireEvent.click(screen.getByRole("button", { name: "models.save" }));

    await waitFor(() => {
      expect(configureModelMock).toHaveBeenCalledWith(
        "dashscope",
        "qwen-plus",
        {
          generate_kwargs: {},
          context_window: 200000,
        },
      );
    });
    expect(onSaved).toHaveBeenCalledTimes(1);
  });

  it("allows adding a manually entered model ID outside discovery results", async () => {
    render(
      <RemoteModelManageModal
        provider={provider()}
        open
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "models.addModel" }));

    const modelIdInput = await screen.findByPlaceholderText(
      "models.modelIdPlaceholder",
    );
    fireEvent.change(modelIdInput, {
      target: { value: "unlisted-model-id" },
    });
    fireEvent.click(screen.getByRole("button", { name: "models.addModel" }));

    await waitFor(() => {
      expect(testModelConnectionMock).toHaveBeenCalledWith("dashscope", {
        model_id: "unlisted-model-id",
      });
      expect(addModelMock).toHaveBeenCalledWith("dashscope", {
        id: "unlisted-model-id",
        name: "unlisted-model-id",
      });
    });
  });

  it("keeps the latest model connection result visible in the model row", async () => {
    testModelConnectionMock.mockResolvedValue({
      success: false,
      message: "HTTP 401",
    });

    render(
      <RemoteModelManageModal
        provider={provider()}
        open
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "models.testConnection" }),
    );

    expect(
      await screen.findByText("models.testConnectionFailedWithMessage"),
    ).toBeInTheDocument();
  });

  it("shows the probe diagnostic message alongside the unsupported toast", async () => {
    const infoSpy = vi.spyOn(toast, "info").mockImplementation(() => undefined);
    const onSaved = vi.fn().mockResolvedValue(undefined);
    probeMultimodalMock.mockResolvedValue({
      supports_image: false,
      supports_video: false,
      supports_multimodal: false,
      message: "Model replied NO_MEDIA",
    });

    render(
      <RemoteModelManageModal
        provider={provider()}
        open
        onClose={vi.fn()}
        onSaved={onSaved}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "models.probeMultimodal" }),
    );

    await waitFor(() => {
      expect(infoSpy).toHaveBeenCalledWith(
        "models.probeNotSupported Model replied NO_MEDIA",
      );
    });
  });

  it("shows the HTTP probe rejection and skips onSaved", async () => {
    const errorSpy = vi
      .spyOn(toast, "error")
      .mockImplementation(() => undefined);
    const onSaved = vi.fn().mockResolvedValue(undefined);
    probeMultimodalMock.mockRejectedValue(
      new Error("image probe inconclusive: upstream returned HTTP 503"),
    );

    render(
      <RemoteModelManageModal
        provider={provider()}
        open
        onClose={vi.fn()}
        onSaved={onSaved}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "models.probeMultimodal" }),
    );

    await waitFor(() => {
      expect(errorSpy).toHaveBeenCalledWith(
        "image probe inconclusive: upstream returned HTTP 503",
      );
    });
    expect(onSaved).not.toHaveBeenCalled();
  });
});
