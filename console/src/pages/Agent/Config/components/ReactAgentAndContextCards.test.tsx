import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { FormProvider, useForm } from "react-hook-form";
import type { AgentConfigFormValues } from "../useAgentConfig";
import { ReactAgentCard } from "./ReactAgentCard";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock("@/components/RuaCard", () => ({
  RuaCard: ({
    children,
    title,
  }: {
    children?: ReactNode;
    title?: ReactNode;
  }) => (
    <section>
      <h2>{title}</h2>
      {children}
    </section>
  ),
}));

vi.mock("@/ui", () => ({
  EnableDisableSwitch: ({
    disabled,
    enabled,
    onChange,
  }: {
    disabled?: boolean;
    enabled: boolean;
    onChange: (enabled: boolean) => void;
  }) => (
    <span>
      <button
        type="button"
        disabled={disabled}
        aria-pressed={!enabled}
        aria-label="common.disable"
        onClick={() => onChange(false)}
      />
      <button
        type="button"
        disabled={disabled}
        aria-pressed={enabled}
        aria-label="common.enable"
        onClick={() => onChange(true)}
      />
    </span>
  ),
  InfoBox: ({ title }: { title?: ReactNode }) => <p>{title}</p>,
  NumberInput: ({
    disabled,
    max,
    min,
    onBlur,
    onChange,
    placeholder,
    step,
    value,
  }: {
    disabled?: boolean;
    max?: number;
    min?: number;
    onBlur?: () => void;
    onChange?: (value: number) => void;
    placeholder?: string;
    step?: number;
    value?: number;
  }) => (
    <input
      disabled={disabled}
      max={max}
      min={min}
      placeholder={placeholder}
      step={step}
      type="number"
      value={value ?? ""}
      onBlur={onBlur}
      onChange={(event) => onChange?.(Number(event.target.value))}
    />
  ),
  Select: ({
    disabled,
    loading,
    onChange,
    options,
    placeholder,
    value,
  }: {
    disabled?: boolean;
    loading?: boolean;
    onChange?: (value: string) => void;
    options?: Array<{ label: ReactNode; value: string }>;
    placeholder?: string;
    value?: string;
  }) => (
    <select
      aria-label={placeholder ?? String(value ?? "")}
      disabled={disabled || loading}
      value={value ?? ""}
      onChange={(event) => onChange?.(event.target.value)}
    >
      {(options ?? []).map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  ),
  Slider: ({
    ariaValueMax,
    ariaValueMin,
    ariaValueNow,
    disabled,
    max = 100,
    min = 0,
    onChange,
    value,
  }: {
    ariaValueMax?: number;
    ariaValueMin?: number;
    ariaValueNow?: number;
    disabled?: boolean;
    max?: number;
    min?: number;
    onChange?: (value: number) => void;
    value?: number;
  }) => (
    <div
      role="slider"
      aria-disabled={disabled}
      aria-valuemax={ariaValueMax ?? max}
      aria-valuemin={ariaValueMin ?? min}
      aria-valuenow={ariaValueNow ?? value}
      tabIndex={0}
      onKeyDown={(event) => {
        if (event.key === "Home") onChange?.(min);
        if (event.key === "End") onChange?.(max);
      }}
    />
  ),
  Toggle: ({
    checked,
    disabled,
    onChange,
  }: {
    checked?: boolean;
    disabled?: boolean;
    onChange?: (event: { target: { checked: boolean } }) => void;
  }) => (
    <input
      aria-label="toggle"
      checked={checked}
      disabled={disabled}
      type="checkbox"
      onChange={(event) =>
        onChange?.({ target: { checked: event.target.checked } })
      }
    />
  ),
  Tooltip: ({ children }: { children?: ReactNode }) => <>{children}</>,
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
      history_replacement_enabled: false,
      early_background_compaction_enabled: true,
      early_compaction_target_ratio: 0.5,
      context_reserve_ratio: 0.2,
      compact_with_thinking_block: false,
      compact_enhanced_prompt: true,
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

function Harness({ children }: { children: ReactNode }) {
  const methods = useForm<AgentConfigFormValues>({
    defaultValues: defaultValues(),
  });
  return <FormProvider {...methods}>{children}</FormProvider>;
}

describe("ReactAgentCard", () => {
  it("keeps Rust language/timezone as callback-owned settings", () => {
    const onLanguageChange = vi.fn();
    const onTimezoneChange = vi.fn();

    render(
      <Harness>
        <ReactAgentCard
          language="en"
          savingLang={false}
          onLanguageChange={onLanguageChange}
          timezone="UTC"
          savingTimezone={false}
          onTimezoneChange={onTimezoneChange}
        />
      </Harness>,
    );

    expect(screen.getByText("agentConfig.reactAgentTitle")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("en"), {
      target: { value: "zh" },
    });
    expect(onLanguageChange).toHaveBeenCalledWith("zh");

    fireEvent.change(screen.getByLabelText("agentConfig.selectTimezone"), {
      target: { value: "Asia/Shanghai" },
    });
    expect(onTimezoneChange).toHaveBeenCalledWith("Asia/Shanghai");

    expect(
      screen.queryByText("agentConfig.memoryManagerBackend"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("agentConfig.maxIters")).not.toBeInTheDocument();
    expect(
      screen.queryByText("agentConfig.autoContinueOnTextOnly"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("agentConfig.maxContextLength"),
    ).not.toBeInTheDocument();
  });

  it("disables language and timezone controls while those Rust settings are saving", () => {
    render(
      <Harness>
        <ReactAgentCard
          language="en"
          savingLang
          onLanguageChange={vi.fn()}
          timezone="UTC"
          savingTimezone
          onTimezoneChange={vi.fn()}
        />
      </Harness>,
    );

    expect(screen.getByLabelText("en")).toBeDisabled();
    expect(screen.getByLabelText("agentConfig.selectTimezone")).toBeDisabled();
  });
});
