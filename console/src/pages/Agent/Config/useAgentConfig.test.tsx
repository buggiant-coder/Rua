import { act, renderHook, waitFor } from "@testing-library/react";
import type { UseFormReturn } from "react-hook-form";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentsRunningConfig } from "../../../api/types";
import { useAgentStore } from "../../../stores/agentStore";
import {
  agentConfigSchema,
  type AgentConfigFormValues,
  useAgentConfig,
} from "./useAgentConfig";

const {
  getAgentRunningConfigMock,
  resetAgentRunningConfigMock,
  updateAgentRunningConfigMock,
  getAgentLanguageMock,
  updateAgentLanguageMock,
  getUserTimezoneMock,
  updateUserTimezoneMock,
  modalConfirmMock,
  toastSuccessMock,
  toastErrorMock,
  tMock,
} = vi.hoisted(() => ({
  getAgentRunningConfigMock: vi.fn(),
  resetAgentRunningConfigMock: vi.fn(),
  updateAgentRunningConfigMock: vi.fn(),
  getAgentLanguageMock: vi.fn(),
  updateAgentLanguageMock: vi.fn(),
  getUserTimezoneMock: vi.fn(),
  updateUserTimezoneMock: vi.fn(),
  modalConfirmMock: vi.fn(),
  toastSuccessMock: vi.fn(),
  toastErrorMock: vi.fn(),
  tMock: vi.fn((key: string) => key),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../../../api/modules/agents", () => ({
  agentsApi: {
    getAgentRunningConfig: getAgentRunningConfigMock,
    resetAgentRunningConfig: resetAgentRunningConfigMock,
    updateAgentRunningConfig: updateAgentRunningConfigMock,
    getAgentLanguage: getAgentLanguageMock,
    updateAgentLanguage: updateAgentLanguageMock,
  },
}));

vi.mock("../../../api", () => ({
  default: {
    getUserTimezone: getUserTimezoneMock,
    updateUserTimezone: updateUserTimezoneMock,
  },
}));

vi.mock("@/ui", () => ({
  Modal: { confirm: modalConfirmMock },
  toast: {
    success: toastSuccessMock,
    error: toastErrorMock,
  },
}));

const runningConfig = (): AgentsRunningConfig => ({
  max_iters: 8,
  auto_continue_on_text_only: true,
  show_debug_menu: false,
  memory_manager_backend: "remelight",
  max_input_length: 128000,
  history_max_length: 100,
  llm_retry_enabled: true,
  llm_max_retries: 3,
  llm_backoff_base: 1,
  llm_backoff_cap: 8,
  llm_max_concurrent: 2,
  llm_max_qpm: 60,
  llm_rate_limit_pause: 1,
  llm_rate_limit_jitter: 0,
  llm_acquire_timeout: 10,
  context_compact: {
    context_compact_enabled: true,
    token_count_estimate_divisor: 4,
    token_count_model: "gpt-4o-mini",
    token_count_use_mirror: false,
    blocking_compact_ratio: 0.5,
    early_background_compaction_enabled: true,
    early_compaction_target_ratio: 0.5,
    context_reserve_ratio: 0.2,
    compact_with_thinking_block: false,
    min_keep_messages: 4,
  },
  tool_result_compact: {
    enabled: true,
    projection_truncation_enabled: false,
    recent_n: 3,
    execution_output_max_tokens: 4000,
    old_max_bytes: 1000,
    retention_days: 7,
  },
  memory_summary: {
    memory_summary_enabled: true,
    dream_cron: "",
    search_mode: "precise",
  },
  embedding_config: {
    backend: "none",
    base_url: "",
    model_name: "",
    api_key: "",
    dimensions: 1536,
    enable_cache: true,
    use_dimensions: false,
    max_cache_size: 100,
    max_input_length: 8192,
    max_batch_size: 16,
  },
});

function renderAgentConfigHook() {
  let formValues = runningConfig() as unknown as AgentConfigFormValues;
  const methods = {
    reset: vi.fn((values: AgentConfigFormValues) => {
      formValues = values;
    }),
    trigger: vi.fn().mockResolvedValue(true),
    getValues: vi.fn(() => formValues),
    setValue: vi.fn((key: keyof AgentConfigFormValues, value: unknown) => {
      formValues = { ...formValues, [key]: value };
    }),
  } as unknown as UseFormReturn<AgentConfigFormValues>;

  return {
    ...renderHook(() => useAgentConfig(methods)),
    methods,
  };
}

describe("useAgentConfig", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tMock.mockImplementation((key: string) => key);
    sessionStorage.clear();
    useAgentStore.setState({ selectedAgent: "agent-1", agents: [] });
    getAgentRunningConfigMock.mockResolvedValue(runningConfig());
    getAgentLanguageMock.mockResolvedValue({ language: "en" });
    getUserTimezoneMock.mockResolvedValue({ timezone: "Asia/Shanghai" });
    updateAgentRunningConfigMock.mockResolvedValue({});
    resetAgentRunningConfigMock.mockResolvedValue({
      ...runningConfig(),
      max_iters: 100,
    });
    updateAgentLanguageMock.mockResolvedValue({
      language: "zh-CN",
      copied_files: ["AGENT_MODE.md"],
    });
    updateUserTimezoneMock.mockResolvedValue({ timezone: "UTC" });
  });

  it("loads selected-agent running config, language, and shared timezone", async () => {
    const { result } = renderAgentConfigHook();

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(getAgentRunningConfigMock).toHaveBeenCalledWith("agent-1");
    expect(getAgentLanguageMock).toHaveBeenCalledWith("agent-1");
    expect(getUserTimezoneMock).toHaveBeenCalledTimes(1);
    expect(result.current.language).toBe("en");
    expect(result.current.timezone).toBe("Asia/Shanghai");
  });

  it("saves valid form values back to the selected Rust running-config route", async () => {
    const { result, methods } = renderAgentConfigHook();
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => {
      methods.setValue("max_iters", 0);
      methods.setValue("tool_result_compact", {
        ...runningConfig().tool_result_compact,
        projection_truncation_batch_size: 10,
      });
    });
    await act(async () => {
      await result.current.handleSave();
    });

    expect(updateAgentRunningConfigMock).toHaveBeenCalledWith(
      "agent-1",
      expect.objectContaining({
        max_iters: 0,
        tool_result_compact: expect.objectContaining({
          projection_truncation_batch_size: 10,
        }),
      }),
    );
    expect(toastSuccessMock).toHaveBeenCalledWith("agentConfig.saveSuccess");
  });

  it("defaults the second-pass batch interval and rejects nonpositive or fractional values", () => {
    const schema = agentConfigSchema.shape.tool_result_compact;
    const tools = runningConfig().tool_result_compact;
    expect(schema.parse(tools).projection_truncation_batch_size).toBe(1);
    for (const value of [0, -1, 1.5]) {
      expect(
        schema.safeParse({ ...tools, projection_truncation_batch_size: value })
          .success,
      ).toBe(false);
    }
    expect(
      schema.parse({ ...tools, projection_truncation_batch_size: 10 })
        .projection_truncation_batch_size,
    ).toBe(10);
  });

  it("confirms and resets running config to application defaults", async () => {
    const { result, methods } = renderAgentConfigHook();
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => result.current.handleReset());

    expect(modalConfirmMock).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "agentConfig.resetTitle",
        okText: "common.reset",
      }),
    );
    expect(resetAgentRunningConfigMock).not.toHaveBeenCalled();

    const confirmArgs = modalConfirmMock.mock.calls[0][0] as {
      onOk: () => Promise<void>;
    };
    await act(async () => {
      await confirmArgs.onOk();
    });

    expect(resetAgentRunningConfigMock).toHaveBeenCalledWith("agent-1");
    expect(methods.reset).toHaveBeenLastCalledWith(
      expect.objectContaining({ max_iters: 100 }),
    );
    expect(toastSuccessMock).toHaveBeenCalledWith("agentConfig.resetSuccess");
  });

  it("confirms language changes before updating the selected agent", async () => {
    const { result } = renderAgentConfigHook();
    await waitFor(() => expect(result.current.loading).toBe(false));

    act(() => result.current.handleLanguageChange("zh-CN"));

    expect(modalConfirmMock).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "agentConfig.languageConfirmTitle",
        okText: "agentConfig.languageConfirmOk",
      }),
    );
    expect(updateAgentLanguageMock).not.toHaveBeenCalled();

    const confirmArgs = modalConfirmMock.mock.calls[0][0] as {
      onOk: () => Promise<void>;
    };
    await act(async () => {
      await confirmArgs.onOk();
    });

    expect(updateAgentLanguageMock).toHaveBeenCalledWith("agent-1", "zh-CN");
    expect(result.current.language).toBe("zh-CN");
    expect(toastSuccessMock).toHaveBeenCalledWith(
      "agentConfig.languageSaveSuccessWithFiles",
    );
  });

  it("writes timezone through the shared user-timezone route state", async () => {
    const { result } = renderAgentConfigHook();
    await waitFor(() => expect(result.current.loading).toBe(false));

    await act(async () => {
      await result.current.handleTimezoneChange("UTC");
    });

    expect(updateUserTimezoneMock).toHaveBeenCalledWith("UTC");
    expect(result.current.timezone).toBe("UTC");
    expect(toastSuccessMock).toHaveBeenCalledWith(
      "agentConfig.timezoneSaveSuccess",
    );
  });
});
