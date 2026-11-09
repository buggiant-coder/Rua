import { NumberInput } from "@/ui";
import { Controller, useFormContext } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { RuaCard } from "@/components/RuaCard";
import { ConfigField } from "./ConfigField";
import type { AgentConfigFormValues } from "../useAgentConfig";
import styles from "../index.module.less";

export function LlmRateLimiterCard() {
  const { t } = useTranslation();
  const { control, getValues } = useFormContext<AgentConfigFormValues>();

  return (
    <RuaCard
      bare
      className={styles.formCard}
      title={t("agentConfig.llmRateLimiterTitle")}
    >
      <Controller
        name="llm_max_concurrent"
        control={control}
        rules={{
          required: t("agentConfig.llmMaxConcurrentRequired"),
          min: { value: 1, message: t("agentConfig.llmMaxConcurrentRange") },
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.llmMaxConcurrent")}
            tooltip={t("agentConfig.llmMaxConcurrentTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              min={1}
              step={1}
              placeholder={t("agentConfig.llmMaxConcurrentPlaceholder")}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="llm_max_qpm"
        control={control}
        rules={{
          required: t("agentConfig.llmMaxQpmRequired"),
          min: { value: 0, message: t("agentConfig.llmMaxQpmRange") },
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.llmMaxQpm")}
            tooltip={t("agentConfig.llmMaxQpmTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              min={0}
              step={10}
              placeholder={t("agentConfig.llmMaxQpmPlaceholder")}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="llm_rate_limit_pause"
        control={control}
        rules={{
          required: t("agentConfig.llmRateLimitPauseRequired"),
          min: { value: 1.0, message: t("agentConfig.llmRateLimitPauseMin") },
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.llmRateLimitPause")}
            tooltip={t("agentConfig.llmRateLimitPauseTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              step={0.5}
              placeholder={t("agentConfig.llmRateLimitPausePlaceholder")}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="llm_rate_limit_jitter"
        control={control}
        rules={{
          required: t("agentConfig.llmRateLimitJitterRequired"),
          min: { value: 0.0, message: t("agentConfig.llmRateLimitJitterMin") },
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.llmRateLimitJitter")}
            tooltip={t("agentConfig.llmRateLimitJitterTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              step={0.5}
              placeholder={t("agentConfig.llmRateLimitJitterPlaceholder")}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="llm_acquire_timeout"
        control={control}
        rules={{
          required: t("agentConfig.llmAcquireTimeoutRequired"),
          min: {
            value: 10.0,
            message: t("agentConfig.llmAcquireTimeoutMin"),
          },
          validate: (value) => {
            const pause = getValues("llm_rate_limit_pause");
            const jitter = getValues("llm_rate_limit_jitter");
            if (
              typeof value !== "number" ||
              typeof pause !== "number" ||
              typeof jitter !== "number" ||
              value > pause + jitter
            ) {
              return true;
            }
            return t("agentConfig.llmAcquireTimeoutGtPauseJitter");
          },
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.llmAcquireTimeout")}
            tooltip={t("agentConfig.llmAcquireTimeoutTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              step={10}
              placeholder={t("agentConfig.llmAcquireTimeoutPlaceholder")}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />
    </RuaCard>
  );
}
