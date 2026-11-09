import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FormProvider, useForm, type UseFormReturn } from "react-hook-form";
import { MemorySummaryCard } from "./MemorySummaryCard";
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
    llm_max_qpm: 0,
    llm_rate_limit_pause: 60,
    llm_rate_limit_jitter: 0,
    llm_acquire_timeout: 30,
    context_compact: {
      context_compact_enabled: true,
      token_count_estimate_divisor: 4,
      token_count_model: "",
      token_count_use_mirror: false,
      blocking_compact_ratio: 0.4,
      early_background_compaction_enabled: true,
      early_compaction_target_ratio: 0.5,
      context_reserve_ratio: 0.2,
      compact_with_thinking_block: false,
      thinking_turns_to_keep: 0,
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
  onReady,
}: {
  onReady: (methods: UseFormReturn<AgentConfigFormValues>) => void;
}) {
  const methods = useForm<AgentConfigFormValues>({
    defaultValues: defaultValues(),
  });
  onReady(methods);
  return (
    <FormProvider {...methods}>
      <MemorySummaryCard />
    </FormProvider>
  );
}

describe("MemorySummaryCard", () => {
  it("keeps empty dream_cron empty until the example is explicitly applied", () => {
    let methods: UseFormReturn<AgentConfigFormValues> | undefined;
    const { container } = render(
      <Harness
        onReady={(nextMethods) => {
          methods = nextMethods;
        }}
      />,
    );

    const dreamCron = screen.getByPlaceholderText(
      "agentConfig.dreamCronPlaceholder",
    ) as HTMLInputElement;

    expect(dreamCron.value).toBe("");
    expect(methods?.getValues("memory_summary.dream_cron")).toBe("");
    fireEvent.click(screen.getByRole("button", { name: "common.disable" }));
    expect(methods?.getValues("memory_summary.memory_summary_enabled")).toBe(
      false,
    );

    fireEvent.blur(dreamCron);
    expect(methods?.getValues("memory_summary.dream_cron")).toBe("");

    const applyExample = container
      .querySelector(".lucide-clipboard-paste")
      ?.closest('[role="button"]');
    expect(applyExample).toBeTruthy();
    fireEvent.click(applyExample!);

    expect(methods?.getValues("memory_summary.dream_cron")).toBe(
      "agentConfig.dreamCronPlaceholder",
    );
  });
});
