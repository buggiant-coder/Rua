import { EnableDisableSwitch, TextInput } from "@/ui";
import { Controller, useFormContext } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { SliderWithValue } from "./SliderWithValue";
import { RuaCard } from "@/components/RuaCard";
import { ConfigField } from "./ConfigField";
import type { AgentConfigFormValues } from "../useAgentConfig";
import styles from "../index.module.less";
import { ThinkingRetentionSlider } from "@/components/ThinkingRetentionSlider";

interface ContextCompactCardProps {
  maxInputLength: number;
}

export function ContextCompactCard({
  maxInputLength,
}: ContextCompactCardProps) {
  const { t } = useTranslation();
  const { control, setValue, watch } = useFormContext<AgentConfigFormValues>();

  const blockingCompactRatio = watch("context_compact.blocking_compact_ratio");
  const earlyCompactionTargetRatio = watch(
    "context_compact.early_compaction_target_ratio",
  );
  const activeWorkPlanInjectionEnabled = watch(
    "context_compact.active_work_plan_injection_enabled",
  );
  const memoryReserveRatio = watch("context_compact.context_reserve_ratio");
  const thinkingTurnsToKeep = watch(
    "context_compact.compact_with_thinking_block",
  )
    ? watch("context_compact.thinking_turns_to_keep") ?? 1
    : 0;

  const blockingCompactThreshold = Math.floor(
    (maxInputLength ?? 0) * (blockingCompactRatio ?? 0),
  );
  const earlyCompactionTargetThreshold = Math.floor(
    (maxInputLength ?? 0) * (earlyCompactionTargetRatio ?? 0),
  );
  const contextCompactReserveThreshold = Math.floor(
    (maxInputLength ?? 0) * (memoryReserveRatio ?? 0),
  );

  return (
    <RuaCard
      className={styles.formCard}
      title={t("agentConfig.contextCompactTitle")}
      style={{ marginTop: 16 }}
    >
      <Controller
        name="context_compact.context_compact_enabled"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.contextCompactEnabled")}
            tooltip={t("agentConfig.contextCompactEnabledTooltip")}
          >
            <EnableDisableSwitch
              enabled={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="context_compact.token_count_estimate_divisor"
        control={control}
        rules={{
          required: t("agentConfig.tokenCountEstimateDivisorRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.tokenCountEstimateDivisor")}
            tooltip={t("agentConfig.tokenCountEstimateDivisorTooltip")}
            error={fieldState.error?.message}
            required
          >
            <SliderWithValue
              min={2}
              max={5}
              step={0.25}
              value={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="context_compact.blocking_compact_ratio"
        control={control}
        rules={{
          required: t("agentConfig.contextCompactRatioRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.contextCompactRatio")}
            tooltip={t("agentConfig.contextCompactRatioTooltip")}
            error={fieldState.error?.message}
            required
          >
            <SliderWithValue
              min={0.3}
              max={0.95}
              step={0.01}
              value={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <ConfigField
        label={t("agentConfig.contextCompactThreshold")}
        tooltip={t("agentConfig.contextCompactThresholdTooltip")}
      >
        <TextInput
          disabled
          value={
            blockingCompactThreshold > 0
              ? blockingCompactThreshold.toLocaleString()
              : ""
          }
          placeholder={t("agentConfig.contextCompactThresholdPlaceholder")}
        />
      </ConfigField>

      <Controller
        name="context_compact.early_background_compaction_enabled"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.earlyBackgroundCompactionEnabled")}
            tooltip={t("agentConfig.earlyBackgroundCompactionEnabledTooltip")}
          >
            <EnableDisableSwitch
              enabled={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="context_compact.active_work_plan_injection_enabled"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.activeWorkPlanInjectionEnabled")}
            tooltip={t("agentConfig.activeWorkPlanInjectionEnabledTooltip")}
          >
            <EnableDisableSwitch
              enabled={Boolean(field.value)}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="context_compact.active_work_plan_injection_interval_ratio"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.activeWorkPlanInjectionInterval")}
            tooltip={t("agentConfig.activeWorkPlanInjectionIntervalTooltip")}
          >
            <SliderWithValue
              min={0.01}
              max={0.25}
              step={0.01}
              value={field.value ?? 0.05}
              onChange={field.onChange}
              disabled={!activeWorkPlanInjectionEnabled}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="context_compact.early_compaction_target_ratio"
        control={control}
        rules={{
          required: t("agentConfig.earlyCompactionTargetRatioRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.earlyCompactionTargetRatio")}
            tooltip={t("agentConfig.earlyCompactionTargetRatioTooltip")}
            error={fieldState.error?.message}
            required
          >
            <SliderWithValue
              min={0.1}
              max={0.9}
              step={0.01}
              value={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <ConfigField
        label={t("agentConfig.earlyCompactionTargetThreshold")}
        tooltip={t("agentConfig.earlyCompactionTargetThresholdTooltip")}
      >
        <TextInput
          disabled
          value={
            earlyCompactionTargetThreshold > 0
              ? earlyCompactionTargetThreshold.toLocaleString()
              : ""
          }
          placeholder={t(
            "agentConfig.earlyCompactionTargetThresholdPlaceholder",
          )}
        />
      </ConfigField>

      <Controller
        name="context_compact.context_reserve_ratio"
        control={control}
        rules={{
          required: t("agentConfig.contextCompactReserveRatioRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.contextCompactReserveRatio")}
            tooltip={t("agentConfig.contextCompactReserveRatioTooltip")}
            error={fieldState.error?.message}
            required
          >
            <SliderWithValue
              min={0.05}
              max={0.3}
              step={0.01}
              value={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <ConfigField
        label={t("agentConfig.contextCompactReserveThreshold")}
        tooltip={t("agentConfig.contextCompactReserveThresholdTooltip")}
      >
        <TextInput
          disabled
          value={
            contextCompactReserveThreshold > 0
              ? contextCompactReserveThreshold.toLocaleString()
              : ""
          }
          placeholder={t(
            "agentConfig.contextCompactReserveThresholdPlaceholder",
          )}
        />
      </ConfigField>

      <Controller
        name="context_compact.thinking_turns_to_keep"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.thinkingTurnsToKeep")}
            tooltip={t("agentConfig.thinkingTurnsToKeepTooltip")}
          >
            <ThinkingRetentionSlider
              value={thinkingTurnsToKeep}
              onChange={(value) => {
                field.onChange(value);
                setValue(
                  "context_compact.compact_with_thinking_block",
                  value !== 0,
                  { shouldDirty: true },
                );
              }}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="context_compact.min_keep_messages"
        control={control}
        rules={{
          required: t("agentConfig.minKeepMessagesRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.minKeepMessages")}
            tooltip={t("agentConfig.minKeepMessagesTooltip")}
            error={fieldState.error?.message}
            required
          >
            <SliderWithValue
              min={1}
              max={20}
              step={1}
              value={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />
    </RuaCard>
  );
}
