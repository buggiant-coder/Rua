import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FormProvider, useForm, type UseFormReturn } from "react-hook-form";
import { LlmRetryCard } from "./LlmRetryCard";
import { LlmRateLimiterCard } from "./LlmRateLimiterCard";
import { EmbeddingConfigCard } from "./EmbeddingConfigCard";
import type { AgentConfigFormValues } from "../useAgentConfig";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

function defaultValues(): AgentConfigFormValues {
  return {
    max_iters: 10,
    auto_continue_on_text_only: false,
    memory_manager_backend: "remelight",
    max_input_length: 128_000,
    history_max_length: 50_000,
    llm_retry_enabled: true,
    llm_max_retries: 3,
    llm_backoff_base: 1,
    llm_backoff_cap: 30,
    llm_max_concurrent: 4,
    llm_max_qpm: 120,
    llm_rate_limit_pause: 60,
    llm_rate_limit_jitter: 5,
    llm_acquire_timeout: 90,
    context_compact: {
      context_compact_enabled: true,
      token_count_estimate_divisor: 4,
      token_count_model: "",
      token_count_use_mirror: false,
      blocking_compact_ratio: 0.5,
      early_background_compaction_enabled: true,
      early_compaction_target_ratio: 0.5,
      context_reserve_ratio: 0.2,
      compact_with_thinking_block: false,
      thinking_turns_to_keep: 3,
      min_keep_messages: 4,
    },
    tool_result_compact: {
      enabled: true,
      projection_truncation_enabled: false,
      recent_n: 2,
      execution_output_max_tokens: 16_384,
      old_max_bytes: 4_096,
      retention_days: 14,
    },
    memory_summary: {
      memory_summary_enabled: true,
      dream_cron: "",
      search_mode: "precise",
    },
    embedding_config: {
      backend: "",
      base_url: "",
      model_name: "",
      api_key: "",
      dimensions: 1024,
      enable_cache: true,
      use_dimensions: true,
      max_cache_size: 1000,
      max_input_length: 8192,
      max_batch_size: 16,
    },
    session_ttl_days: 30,
  };
}

function Harness({
  children,
  values,
  onReady,
}: {
  children: React.ReactNode;
  values?: Partial<AgentConfigFormValues>;
  onReady?: (methods: UseFormReturn<AgentConfigFormValues>) => void;
}) {
  const methods = useForm<AgentConfigFormValues>({
    defaultValues: {
      ...defaultValues(),
      ...values,
    },
  });
  onReady?.(methods);
  return <FormProvider {...methods}>{children}</FormProvider>;
}

function inputByValue(value: string): HTMLInputElement {
  return screen.getByDisplayValue(value) as HTMLInputElement;
}

describe("runtime config cards", () => {
  it("binds Rust LLM retry fields and disables retry numeric inputs when retry is unavailable", () => {
    let methods: UseFormReturn<AgentConfigFormValues> | undefined;

    render(
      <Harness
        onReady={(nextMethods) => {
          methods = nextMethods;
        }}
      >
        <LlmRetryCard llmRetryEnabled={false} />
      </Harness>,
    );

    expect(screen.getByText("agentConfig.llmRetryTitle")).toBeInTheDocument();

    const retryDisable = screen.getByRole("button", {
      name: "common.disable",
    });
    const retryEnable = screen.getByRole("button", { name: "common.enable" });
    expect(retryEnable).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(retryDisable);
    expect(methods?.getValues("llm_retry_enabled")).toBe(false);

    expect(inputByValue("3")).toBeDisabled();
    expect(inputByValue("1")).toBeDisabled();
    expect(inputByValue("30")).toBeDisabled();
  });

  it("validates Rust LLM backoff and rate-limit cross-field boundaries", async () => {
    let methods: UseFormReturn<AgentConfigFormValues> | undefined;

    render(
      <Harness
        values={{
          llm_backoff_base: 10,
          llm_backoff_cap: 5,
          llm_rate_limit_pause: 60,
          llm_rate_limit_jitter: 5,
          llm_acquire_timeout: 65,
        }}
        onReady={(nextMethods) => {
          methods = nextMethods;
        }}
      >
        <LlmRetryCard />
        <LlmRateLimiterCard />
      </Harness>,
    );

    await methods?.trigger();

    await waitFor(() => {
      expect(
        screen.getByText("agentConfig.llmBackoffCapGteBase"),
      ).toBeInTheDocument();
      expect(
        screen.getByText("agentConfig.llmAcquireTimeoutGtPauseJitter"),
      ).toBeInTheDocument();
    });
  });

  it("enables embedding runtime fields only after Rust base URL and model are configured", () => {
    let methods: UseFormReturn<AgentConfigFormValues> | undefined;

    render(
      <Harness
        onReady={(nextMethods) => {
          methods = nextMethods;
        }}
      >
        <EmbeddingConfigCard />
      </Harness>,
    );

    const baseUrl = screen.getByPlaceholderText(
      "agentConfig.embeddingBaseUrlPlaceholder",
    );
    const modelName = screen.getByPlaceholderText(
      "agentConfig.embeddingModelNamePlaceholder",
    );
    const apiKey = screen.getByPlaceholderText(
      "agentConfig.embeddingApiKeyPlaceholder",
    ) as HTMLInputElement;
    const dimensions = inputByValue("1024");
    const cacheSize = inputByValue("1000");
    const maxInputLength = inputByValue("8192");
    const maxBatchSize = inputByValue("16");
    const cacheDisable = screen.getByRole("button", {
      name: "common.disable",
    });
    const cacheEnable = screen.getByRole("button", { name: "common.enable" });

    expect(apiKey.type).toBe("password");
    expect(dimensions).toBeDisabled();
    expect(cacheSize).toBeDisabled();
    expect(maxInputLength).toBeDisabled();
    expect(maxBatchSize).toBeDisabled();
    expect(cacheDisable).toBeDisabled();
    expect(cacheEnable).toBeDisabled();

    fireEvent.change(baseUrl, { target: { value: "http://localhost:11434" } });
    expect(dimensions).toBeDisabled();

    fireEvent.change(modelName, { target: { value: "bge-m3" } });

    expect(dimensions).not.toBeDisabled();
    expect(cacheSize).not.toBeDisabled();
    expect(maxInputLength).not.toBeDisabled();
    expect(maxBatchSize).not.toBeDisabled();
    expect(cacheDisable).not.toBeDisabled();
    expect(cacheEnable).not.toBeDisabled();

    fireEvent.click(cacheDisable);
    expect(methods?.getValues("embedding_config.enable_cache")).toBe(false);
  });
});
