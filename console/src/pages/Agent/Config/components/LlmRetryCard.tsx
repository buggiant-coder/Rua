import { EnableDisableSwitch, NumberInput } from "@/ui";
import { Controller, useFormContext } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { RuaCard } from "@/components/RuaCard";
import { ConfigField } from "./ConfigField";
import type { AgentConfigFormValues } from "../useAgentConfig";
import styles from "../index.module.less";

interface LlmRetryCardProps {
  llmRetryEnabled?: boolean;
}

export function LlmRetryCard({ llmRetryEnabled = true }: LlmRetryCardProps) {
  const { t } = useTranslation();
  const { control, getValues } = useFormContext<AgentConfigFormValues>();

  return (
    <RuaCard
      bare
      className={styles.formCard}
      title={t("agentConfig.llmRetryTitle")}
    >
      <Controller
        name="llm_retry_enabled"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.llmRetryEnabled")}
            tooltip={t("agentConfig.llmRetryEnabledTooltip")}
          >
            <EnableDisableSwitch
              enabled={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <div className={styles.llmRetryRow}>
        <Controller
          name="llm_max_retries"
          control={control}
          rules={{
            required: t("agentConfig.llmMaxRetriesRequired"),
            min: { value: 1, message: t("agentConfig.llmMaxRetriesMin") },
          }}
          render={({ field, fieldState }) => (
            <ConfigField
              label={t("agentConfig.llmMaxRetries")}
              tooltip={t("agentConfig.llmMaxRetriesTooltip")}
              error={fieldState.error?.message}
              required
              className={styles.llmRetryField}
            >
              <NumberInput
                style={{ width: "100%" }}
                min={1}
                step={1}
                disabled={!llmRetryEnabled}
                placeholder={t("agentConfig.llmMaxRetriesPlaceholder")}
                value={field.value}
                onChange={field.onChange}
                onBlur={field.onBlur}
              />
            </ConfigField>
          )}
        />

        <Controller
          name="llm_backoff_base"
          control={control}
          rules={{
            required: t("agentConfig.llmBackoffBaseRequired"),
            min: { value: 0.1, message: t("agentConfig.llmBackoffBaseMin") },
          }}
          render={({ field, fieldState }) => (
            <ConfigField
              label={t("agentConfig.llmBackoffBase")}
              tooltip={t("agentConfig.llmBackoffBaseTooltip")}
              error={fieldState.error?.message}
              required
              className={styles.llmRetryField}
            >
              <NumberInput
                style={{ width: "100%" }}
                step={0.1}
                disabled={!llmRetryEnabled}
                placeholder={t("agentConfig.llmBackoffBasePlaceholder")}
                value={field.value}
                onChange={field.onChange}
                onBlur={field.onBlur}
              />
            </ConfigField>
          )}
        />

        <Controller
          name="llm_backoff_cap"
          control={control}
          rules={{
            required: t("agentConfig.llmBackoffCapRequired"),
            min: { value: 0.5, message: t("agentConfig.llmBackoffCapMin") },
            validate: (value) => {
              const backoffBase = getValues("llm_backoff_base");
              if (
                typeof value !== "number" ||
                typeof backoffBase !== "number" ||
                value >= backoffBase
              ) {
                return true;
              }
              return t("agentConfig.llmBackoffCapGteBase");
            },
          }}
          render={({ field, fieldState }) => (
            <ConfigField
              label={t("agentConfig.llmBackoffCap")}
              tooltip={t("agentConfig.llmBackoffCapTooltip")}
              error={fieldState.error?.message}
              required
              className={styles.llmRetryField}
            >
              <NumberInput
                style={{ width: "100%" }}
                step={0.5}
                disabled={!llmRetryEnabled}
                placeholder={t("agentConfig.llmBackoffCapPlaceholder")}
                value={field.value}
                onChange={field.onChange}
                onBlur={field.onBlur}
              />
            </ConfigField>
          )}
        />
      </div>
    </RuaCard>
  );
}
