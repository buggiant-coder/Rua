import type { ReactNode } from "react";
import { ChevronDown, HelpCircle } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Btn, EnableDisableSwitch, NumberInput, Slider, Tooltip } from "@/ui";
import { ThinkingRetentionSlider } from "@/components/ThinkingRetentionSlider";
import type {
  ChatRunningConfig,
  ContextCompactConfig,
  ToolResultCompactConfig,
} from "@/api/types";
import "./styles.css";

export type RunningConfigSectionId =
  | "runtime"
  | "compaction"
  | "aggressive"
  | "token";

interface RunningConfigOptionsProps {
  value: ChatRunningConfig;
  showCompactionMasterControls?: boolean;
  runtimeExtras?: ReactNode;
  openSections: Record<RunningConfigSectionId, boolean>;
  onToggleSection: (section: RunningConfigSectionId) => void;
  onRunningChange: (
    key: "max_iters" | "auto_continue_on_text_only" | "max_input_length",
    value: number | boolean,
  ) => void;
  onContextChange: (
    key: keyof ContextCompactConfig,
    value: ContextCompactConfig[keyof ContextCompactConfig],
  ) => void;
  onToolResultChange: (
    key: keyof ToolResultCompactConfig,
    value: ToolResultCompactConfig[keyof ToolResultCompactConfig],
  ) => void;
  onThinkingTurnsChange: (value: number) => void;
}

export function RunningConfigOptions({
  value,
  showCompactionMasterControls = false,
  runtimeExtras,
  openSections,
  onToggleSection,
  onRunningChange,
  onContextChange,
  onToolResultChange,
  onThinkingTurnsChange,
}: RunningConfigOptionsProps) {
  const { t } = useTranslation();
  const context = value.context_compact;
  const tools = value.tool_result_compact;
  const thinkingTurnsToKeep = context.compact_with_thinking_block
    ? numberOr(context.thinking_turns_to_keep, 1)
    : 0;

  return (
    <>
      <ChatOptionSection
        title={t("chat.options.agentRuntime")}
        open={openSections.runtime}
        onToggle={() => onToggleSection("runtime")}
      >
        <ChatOptionRow
          label={t("agentConfig.maxIters")}
          tooltip={t("agentConfig.maxItersTooltip")}
        >
          <NumberInput
            min={0}
            value={value.max_iters}
            onChange={(next) =>
              onRunningChange("max_iters", clampNumber(next, 0))
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.autoContinueOnTextOnly")}
          tooltip={t("agentConfig.autoContinueOnTextOnlyTooltip")}
        >
          <EnableDisableSwitch
            enabled={value.auto_continue_on_text_only}
            onChange={(enabled) =>
              onRunningChange("auto_continue_on_text_only", enabled)
            }
          />
        </ChatOptionRow>
        {runtimeExtras}
      </ChatOptionSection>

      <ChatOptionSection
        title={t("chat.options.contextCompaction", {
          defaultValue: "Context compaction",
        })}
        open={openSections.compaction}
        onToggle={() => onToggleSection("compaction")}
      >
        {showCompactionMasterControls && (
          <ChatOptionRow
            label={t("agentConfig.contextCompactEnabled")}
            tooltip={t("agentConfig.contextCompactEnabledTooltip")}
          >
            <EnableDisableSwitch
              enabled={context.context_compact_enabled}
              onChange={(enabled) =>
                onContextChange("context_compact_enabled", enabled)
              }
            />
          </ChatOptionRow>
        )}
        <ChatOptionRow
          label={t("agentConfig.maxContextLength")}
          tooltip={t("agentConfig.maxContextLengthTooltip")}
          value={
            <span className="rua-owned-chat-options-presets">
              {(
                [
                  { label: "100K", value: 100_000 },
                  { label: "200K", value: 200_000 },
                  { label: "1M", value: 1_000_000 },
                ] as const
              ).map((preset) => (
                <Btn
                  key={preset.label}
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    onRunningChange("max_input_length", preset.value)
                  }
                >
                  {preset.label}
                </Btn>
              ))}
            </span>
          }
        >
          <NumberInput
            min={1000}
            step={1024}
            value={value.max_input_length}
            onChange={(next) =>
              onRunningChange("max_input_length", clampNumber(next, 1000))
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.contextCompactRatio")}
          tooltip={t("agentConfig.contextCompactRatioTooltip")}
        >
          <Slider
            min={0.4}
            max={0.95}
            step={0.01}
            value={numberOr(context.blocking_compact_ratio, 0.9)}
            onChange={(next: number) =>
              onContextChange("blocking_compact_ratio", next)
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.contextCompactReserveRatio")}
          tooltip={t("agentConfig.contextCompactReserveRatioTooltip")}
        >
          <Slider
            min={0.05}
            max={0.4}
            step={0.01}
            value={numberOr(context.context_reserve_ratio, 0.1)}
            onChange={(next: number) =>
              onContextChange("context_reserve_ratio", next)
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.minKeepMessages")}
          tooltip={t("agentConfig.minKeepMessagesTooltip")}
        >
          <Slider
            min={1}
            max={20}
            step={1}
            value={numberOr(context.min_keep_messages, 5)}
            onChange={(next: number) =>
              onContextChange("min_keep_messages", next)
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.compactEnhancedPrompt", {
            defaultValue: "Enhanced compaction prompt",
          })}
          tooltip={t("agentConfig.compactEnhancedPromptTooltip", {
            defaultValue: "Use a richer summarization prompt when compacting.",
          })}
        >
          <EnableDisableSwitch
            enabled={context.compact_enhanced_prompt}
            onChange={(enabled) =>
              onContextChange("compact_enhanced_prompt", enabled)
            }
          />
        </ChatOptionRow>
        {showCompactionMasterControls && (
          <ChatOptionRow
            label={t("agentConfig.toolResultCompactEnabled")}
            tooltip={t("agentConfig.toolResultCompactEnabledTooltip")}
          >
            <EnableDisableSwitch
              enabled={tools.enabled}
              onChange={(enabled) => onToolResultChange("enabled", enabled)}
            />
          </ChatOptionRow>
        )}
        <ChatOptionRow
          label={t("agentConfig.toolResultCompactRecentThreshold")}
          tooltip={t("agentConfig.toolResultCompactRecentThresholdTooltip")}
        >
          <NumberInput
            min={1000}
            step={1000}
            value={tools.execution_output_max_tokens}
            onChange={(next) =>
              onToolResultChange(
                "execution_output_max_tokens",
                clampNumber(next, 1000),
              )
            }
          />
        </ChatOptionRow>
      </ChatOptionSection>

      <ChatOptionSection
        title={t("chat.options.aggressiveContext", {
          defaultValue: "Aggressive context management",
        })}
        open={openSections.aggressive}
        onToggle={() => onToggleSection("aggressive")}
      >
        <ChatOptionRow
          label={t("agentConfig.historyReplacementEnabled")}
          tooltip={t("agentConfig.historyReplacementEnabledTooltip")}
        >
          <EnableDisableSwitch
            enabled={context.history_replacement_enabled}
            onChange={(enabled) =>
              onContextChange("history_replacement_enabled", enabled)
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.activeWorkPlanInjectionEnabled")}
          tooltip={t("agentConfig.activeWorkPlanInjectionEnabledTooltip")}
        >
          <EnableDisableSwitch
            enabled={Boolean(context.active_work_plan_injection_enabled)}
            onChange={(enabled) =>
              onContextChange("active_work_plan_injection_enabled", enabled)
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.activeWorkPlanInjectionInterval")}
          tooltip={t("agentConfig.activeWorkPlanInjectionIntervalTooltip")}
          disabled={!context.active_work_plan_injection_enabled}
          value={t("agentConfig.activeWorkPlanInjectionIntervalValue", {
            percent: Math.round(
              numberOr(
                context.active_work_plan_injection_interval_ratio,
                0.05,
              ) * 100,
            ),
            tokens: Math.round(
              value.max_input_length *
                numberOr(
                  context.active_work_plan_injection_interval_ratio,
                  0.05,
                ),
            ).toLocaleString(),
          })}
        >
          <Slider
            min={0.01}
            max={0.25}
            step={0.01}
            disabled={!context.active_work_plan_injection_enabled}
            value={numberOr(
              context.active_work_plan_injection_interval_ratio,
              0.05,
            )}
            onChange={(next: number) =>
              onContextChange("active_work_plan_injection_interval_ratio", next)
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.toolResultProjectionTruncationEnabled")}
          tooltip={t(
            "agentConfig.toolResultProjectionTruncationEnabledTooltip",
          )}
        >
          <EnableDisableSwitch
            enabled={tools.projection_truncation_enabled}
            onChange={(enabled) =>
              onToolResultChange("projection_truncation_enabled", enabled)
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.toolResultProjectionTruncationBatchSize")}
          tooltip={t(
            "agentConfig.toolResultProjectionTruncationBatchSizeTooltip",
          )}
          disabled={!tools.projection_truncation_enabled}
        >
          <NumberInput
            aria-label={t(
              "agentConfig.toolResultProjectionTruncationBatchSize",
            )}
            min={1}
            step={1}
            inputStep={1}
            disabled={!tools.projection_truncation_enabled}
            value={numberOr(tools.projection_truncation_batch_size, 1)}
            onChange={(next) =>
              onToolResultChange(
                "projection_truncation_batch_size",
                Math.floor(clampNumber(next, 1)),
              )
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.toolResultCompactRecentN")}
          tooltip={t("agentConfig.toolResultCompactRecentNTooltip")}
          disabled={!tools.projection_truncation_enabled}
        >
          <NumberInput
            min={1}
            step={1}
            disabled={!tools.projection_truncation_enabled}
            value={numberOr(tools.recent_n, 2)}
            onChange={(next) =>
              onToolResultChange("recent_n", clampNumber(next, 1))
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.toolResultCompactOldMaxBytes", {
            defaultValue: "Older tool-result byte limit",
          })}
          tooltip={t("agentConfig.toolResultCompactOldMaxBytesTooltip", {
            defaultValue:
              "Byte limit applied to tool results beyond the recent window.",
          })}
          disabled={!tools.projection_truncation_enabled}
        >
          <NumberInput
            min={1000}
            step={1000}
            disabled={!tools.projection_truncation_enabled}
            value={tools.old_max_bytes}
            onChange={(next) =>
              onToolResultChange("old_max_bytes", clampNumber(next, 1000))
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.thinkingTurnsToKeep")}
          tooltip={t("agentConfig.thinkingTurnsToKeepTooltip")}
          value={
            thinkingTurnsToKeep === -1
              ? t("agentConfig.thinkingTurnsAll")
              : thinkingTurnsToKeep === 0
              ? t("agentConfig.thinkingTurnsNone")
              : t("agentConfig.thinkingTurnsCount", {
                  count: thinkingTurnsToKeep,
                })
          }
        >
          <ThinkingRetentionSlider
            value={thinkingTurnsToKeep}
            onChange={onThinkingTurnsChange}
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.earlyBackgroundCompactionEnabled")}
          tooltip={t("agentConfig.earlyBackgroundCompactionEnabledTooltip")}
        >
          <EnableDisableSwitch
            enabled={context.early_background_compaction_enabled}
            onChange={(enabled) =>
              onContextChange("early_background_compaction_enabled", enabled)
            }
          />
        </ChatOptionRow>
        <ChatOptionRow
          label={t("agentConfig.earlyCompactionTargetRatio")}
          tooltip={t("agentConfig.earlyCompactionTargetRatioTooltip")}
          disabled={!context.early_background_compaction_enabled}
        >
          <Slider
            min={0.1}
            max={0.9}
            step={0.01}
            disabled={!context.early_background_compaction_enabled}
            value={numberOr(context.early_compaction_target_ratio, 0.5)}
            onChange={(next: number) =>
              onContextChange("early_compaction_target_ratio", next)
            }
          />
        </ChatOptionRow>
      </ChatOptionSection>

      <ChatOptionSection
        title={t("chat.options.tokenEstimation", {
          defaultValue: "Token estimation",
        })}
        open={openSections.token}
        onToggle={() => onToggleSection("token")}
      >
        <ChatOptionRow
          label={t("agentConfig.tokenCountEstimateDivisor")}
          tooltip={t("agentConfig.tokenCountEstimateDivisorTooltip")}
        >
          <Slider
            min={2}
            max={5}
            step={0.25}
            value={numberOr(context.token_count_estimate_divisor, 4)}
            onChange={(next: number) =>
              onContextChange("token_count_estimate_divisor", next)
            }
          />
        </ChatOptionRow>
      </ChatOptionSection>
    </>
  );
}

export function ChatOptionsLayout({ children }: { children: ReactNode }) {
  return <div className="rua-owned-chat-options">{children}</div>;
}

export function ChatOptionSection({
  title,
  summary,
  open,
  onToggle,
  children,
}: {
  title: ReactNode;
  summary?: ReactNode;
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <section
      className="rua-owned-chat-options-section"
      data-open={open ? "true" : "false"}
    >
      <button
        type="button"
        className="rua-owned-chat-options-section-header"
        aria-expanded={open}
        onClick={onToggle}
      >
        <ChevronDown
          size={16}
          className="rua-owned-chat-options-section-chevron"
          aria-hidden
        />
        <span className="rua-owned-chat-options-section-title">{title}</span>
        {summary !== undefined && (
          <span className="rua-owned-chat-options-section-summary">
            {summary}
          </span>
        )}
      </button>
      {open && <div className="rua-owned-chat-options-grid">{children}</div>}
    </section>
  );
}

export function ChatOptionRow({
  label,
  tooltip,
  value,
  hint,
  disabled,
  children,
}: {
  label: ReactNode;
  tooltip?: ReactNode;
  value?: ReactNode;
  hint?: ReactNode;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <div
      className="rua-owned-chat-options-row"
      data-disabled={disabled ? "true" : undefined}
    >
      <div className="rua-owned-chat-options-label">
        <span className="rua-owned-chat-options-label-main">
          <span>{label}</span>
          {tooltip && (
            <Tooltip title={tooltip}>
              <button
                type="button"
                className="rua-owned-chat-options-help"
                aria-label="配置说明"
              >
                <HelpCircle size={14} />
              </button>
            </Tooltip>
          )}
        </span>
        {value !== undefined && (
          <span className="rua-owned-chat-options-value">{value}</span>
        )}
      </div>
      <div className="rua-owned-chat-options-control">{children}</div>
      {hint !== undefined && (
        <div className="rua-owned-chat-options-hint">{hint}</div>
      )}
    </div>
  );
}

function numberOr(value: number | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function clampNumber(value: number | null, min: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return min;
  return Math.max(min, value);
}
