import { EnableDisableSwitch, NumberInput } from "@/ui";
import { Controller, useFormContext } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { SliderWithValue } from "./SliderWithValue";
import { RuaCard } from "@/components/RuaCard";
import { ConfigField } from "./ConfigField";
import type { AgentConfigFormValues } from "../useAgentConfig";
import styles from "../index.module.less";

export function ToolResultCompactCard() {
  const { t } = useTranslation();
  const { control, watch } = useFormContext<AgentConfigFormValues>();
  const projectionEnabled = watch(
    "tool_result_compact.projection_truncation_enabled",
  );

  return (
    <RuaCard
      className={styles.formCard}
      title={t("agentConfig.toolResultCompactTitle")}
      style={{ marginTop: 16 }}
    >
      <Controller
        name="tool_result_compact.enabled"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.toolResultCompactEnabled")}
            tooltip={t("agentConfig.toolResultCompactEnabledTooltip")}
          >
            <EnableDisableSwitch
              enabled={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="tool_result_compact.projection_truncation_enabled"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.toolResultProjectionTruncationEnabled")}
            tooltip={t(
              "agentConfig.toolResultProjectionTruncationEnabledTooltip",
            )}
          >
            <EnableDisableSwitch
              enabled={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="tool_result_compact.projection_truncation_batch_size"
        control={control}
        defaultValue={1}
        rules={{
          required: t(
            "agentConfig.toolResultProjectionTruncationBatchSizeRequired",
          ),
          min: {
            value: 1,
            message: t(
              "agentConfig.toolResultProjectionTruncationBatchSizeRequired",
            ),
          },
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.toolResultProjectionTruncationBatchSize")}
            tooltip={t(
              "agentConfig.toolResultProjectionTruncationBatchSizeTooltip",
            )}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              aria-label={t(
                "agentConfig.toolResultProjectionTruncationBatchSize",
              )}
              style={{ width: "100%" }}
              min={1}
              step={1}
              inputStep={1}
              disabled={!projectionEnabled}
              value={field.value}
              onChange={(next) =>
                field.onChange(Math.max(1, Math.floor(next ?? 1)))
              }
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="tool_result_compact.recent_n"
        control={control}
        rules={{
          required: t("agentConfig.toolResultCompactRecentNRequired"),
          min: {
            value: 1,
            message: t("agentConfig.toolResultCompactRecentNRequired"),
          },
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.toolResultCompactRecentN")}
            tooltip={t("agentConfig.toolResultCompactRecentNTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              min={1}
              step={1}
              disabled={!projectionEnabled}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="tool_result_compact.old_max_bytes"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.toolResultCompactOldMaxBytes")}
            tooltip={t("agentConfig.toolResultCompactOldMaxBytesTooltip")}
          >
            <NumberInput
              style={{ width: "100%" }}
              min={1000}
              step={1000}
              disabled={!projectionEnabled}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="tool_result_compact.execution_output_max_tokens"
        control={control}
        rules={{
          required: t("agentConfig.toolResultCompactRecentThresholdRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.toolResultCompactRecentThreshold")}
            tooltip={t("agentConfig.toolResultCompactRecentThresholdTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              min={1000}
              step={1000}
              placeholder={t(
                "agentConfig.toolResultCompactRecentThresholdPlaceholder",
              )}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="tool_result_compact.retention_days"
        control={control}
        rules={{
          required: t("agentConfig.toolResultCompactRetentionDaysRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.toolResultCompactRetentionDays")}
            tooltip={t("agentConfig.toolResultCompactRetentionDaysTooltip")}
            error={fieldState.error?.message}
            required
          >
            <SliderWithValue
              min={1}
              max={10}
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
