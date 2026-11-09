import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FormProvider, useForm, type UseFormReturn } from "react-hook-form";
import { ToolResultCompactCard } from "./ToolResultCompactCard";
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
      retention_days: 10,
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
      <ToolResultCompactCard />
    </FormProvider>
  );
}

describe("ToolResultCompactCard", () => {
  it("renders execution-time tool result handling from form state", () => {
    render(<Harness onReady={() => {}} />);

    expect(
      screen.getByText("agentConfig.toolResultCompactTitle"),
    ).toBeVisible();
    expect(
      screen.getByText("agentConfig.toolResultProjectionTruncationEnabled"),
    ).toBeVisible();
    expect(
      screen.getByText("agentConfig.toolResultCompactRecentN"),
    ).toBeVisible();
    expect(
      screen.getByPlaceholderText(
        "agentConfig.toolResultCompactRecentThresholdPlaceholder",
      ),
    ).toHaveValue(16_384);
    expect(screen.getByDisplayValue("2")).toBeDisabled();
    expect(screen.getByDisplayValue("4096")).toBeDisabled();
    expect(
      screen.getByLabelText(
        "agentConfig.toolResultProjectionTruncationBatchSize",
      ),
    ).toBeDisabled();
    expect(
      screen.getByLabelText(
        "agentConfig.toolResultProjectionTruncationBatchSize",
      ),
    ).toHaveValue(1);

    const sliders = screen.getAllByRole("slider");
    expect(
      sliders.map((slider) => slider.getAttribute("aria-valuenow")),
    ).toEqual(["10"]);
  });

  it("writes enabled, inline threshold, and retention back to running-config fields", () => {
    let methods: UseFormReturn<AgentConfigFormValues> | undefined;
    render(
      <Harness
        onReady={(nextMethods) => {
          methods = nextMethods;
        }}
      />,
    );

    const disable = screen.getAllByRole("button", { name: "common.disable" });
    const enable = screen.getAllByRole("button", { name: "common.enable" });
    expect(enable[0]).toHaveAttribute("aria-pressed", "true");
    expect(disable[1]).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(disable[0]);
    expect(methods?.getValues("tool_result_compact.enabled")).toBe(false);
    fireEvent.click(enable[1]);
    expect(
      methods?.getValues("tool_result_compact.projection_truncation_enabled"),
    ).toBe(true);
    expect(screen.getByDisplayValue("2")).not.toBeDisabled();
    expect(screen.getByDisplayValue("4096")).not.toBeDisabled();
    const batchSize = screen.getByLabelText(
      "agentConfig.toolResultProjectionTruncationBatchSize",
    );
    expect(batchSize).not.toBeDisabled();
    fireEvent.change(batchSize, { target: { value: "10" } });
    expect(
      methods?.getValues(
        "tool_result_compact.projection_truncation_batch_size",
      ),
    ).toBe(10);

    fireEvent.change(
      screen.getByPlaceholderText(
        "agentConfig.toolResultCompactRecentThresholdPlaceholder",
      ),
      { target: { value: "20000" } },
    );

    expect(
      methods?.getValues("tool_result_compact.execution_output_max_tokens"),
    ).toBe(20_000);
    expect(methods?.getValues("tool_result_compact.old_max_bytes")).toBe(4_096);
  });
});
