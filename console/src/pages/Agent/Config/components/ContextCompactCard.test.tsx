import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { FormProvider, useForm, type UseFormReturn } from "react-hook-form";
import { ContextCompactCard } from "./ContextCompactCard";
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
      blocking_compact_ratio: 0.95,
      active_work_plan_injection_enabled: false,
      active_work_plan_injection_interval_ratio: 0.05,
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
  maxInputLength,
  onReady,
}: {
  maxInputLength: number;
  onReady: (methods: UseFormReturn<AgentConfigFormValues>) => void;
}) {
  const methods = useForm<AgentConfigFormValues>({
    defaultValues: defaultValues(),
  });
  onReady(methods);
  return (
    <FormProvider {...methods}>
      <ContextCompactCard maxInputLength={maxInputLength} />
    </FormProvider>
  );
}

describe("ContextCompactCard", () => {
  it("derives compact thresholds from Rust context_compact ratios and max input length", () => {
    let methods: UseFormReturn<AgentConfigFormValues> | undefined;

    render(
      <Harness
        maxInputLength={128_000}
        onReady={(nextMethods) => {
          methods = nextMethods;
        }}
      />,
    );

    expect(screen.getByDisplayValue("121,600")).toBeDisabled();
    expect(screen.getByDisplayValue("64,000")).toBeDisabled();
    expect(screen.getByDisplayValue("25,600")).toBeDisabled();

    const sliders = screen.getAllByRole("slider");
    expect(
      sliders.map((slider) => slider.getAttribute("aria-valuenow")),
    ).toEqual(["4", "0.95", "0.05", "0.5", "0.2", "0", "4"]);
    expect(sliders[1]).toHaveAttribute("aria-valuemax", "0.95");

    const disableButtons = screen.getAllByRole("button", {
      name: "common.disable",
    });
    const enableButtons = screen.getAllByRole("button", {
      name: "common.enable",
    });
    expect(enableButtons[0]).toHaveAttribute("aria-pressed", "true");
    expect(enableButtons[1]).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(disableButtons[0]);
    expect(methods?.getValues("context_compact.context_compact_enabled")).toBe(
      false,
    );

    fireEvent.keyDown(sliders[5], { key: "ArrowRight" });
    expect(
      methods?.getValues("context_compact.compact_with_thinking_block"),
    ).toBe(true);
    expect(methods?.getValues("context_compact.thinking_turns_to_keep")).toBe(
      1,
    );
  });

  it("leaves derived thresholds empty when model context length is unavailable", () => {
    render(<Harness maxInputLength={0} onReady={() => {}} />);

    expect(
      screen.getByPlaceholderText(
        "agentConfig.contextCompactThresholdPlaceholder",
      ),
    ).toHaveValue("");
    expect(
      screen.getByPlaceholderText(
        "agentConfig.earlyCompactionTargetThresholdPlaceholder",
      ),
    ).toHaveValue("");
    expect(
      screen.getByPlaceholderText(
        "agentConfig.contextCompactReserveThresholdPlaceholder",
      ),
    ).toHaveValue("");
  });
});
