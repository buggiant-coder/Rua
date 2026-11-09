import { Controller, useFormContext } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { RuaCard } from "@/components/RuaCard";
import { ConfigField } from "./ConfigField";
import { InputWithExample } from "../../../../components/InputWithExample";
import type { AgentConfigFormValues } from "../useAgentConfig";
import styles from "../index.module.less";
import { EnableDisableSwitch, InfoBox, NumberInput, TextInput } from "@/ui";

export function EmbeddingConfigCard() {
  const { t } = useTranslation();
  const { control, watch } = useFormContext<AgentConfigFormValues>();

  const baseUrl = watch("embedding_config.base_url");
  const modelName = watch("embedding_config.model_name");
  const embeddingEnabled = !!(baseUrl?.trim() && modelName?.trim());

  return (
    <RuaCard
      bare
      className={styles.formCard}
      title={t("agentConfig.embeddingConfigTitle")}
    >
      <InfoBox
        type="warning"
        title={`${t("agentConfig.embeddingEnableHint")} ${t(
          "agentConfig.embeddingRestartWarning",
        )}`}
        style={{ marginBottom: 16 }}
      />

      <Controller
        name="embedding_config.base_url"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.embeddingBaseUrl")}
            tooltip={t("agentConfig.embeddingBaseUrlTooltip")}
          >
            <InputWithExample
              placeholder={t("agentConfig.embeddingBaseUrlPlaceholder")}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="embedding_config.model_name"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.embeddingModelName")}
            tooltip={t("agentConfig.embeddingModelNameTooltip")}
          >
            <TextInput
              placeholder={t("agentConfig.embeddingModelNamePlaceholder")}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="embedding_config.api_key"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.embeddingApiKey")}
            tooltip={t("agentConfig.embeddingApiKeyTooltip")}
          >
            <TextInput
              type="password"
              placeholder={t("agentConfig.embeddingApiKeyPlaceholder")}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="embedding_config.dimensions"
        control={control}
        rules={{
          required: t("agentConfig.embeddingDimensionsRequired"),
          min: { value: 1, message: t("agentConfig.embeddingDimensionsMin") },
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.embeddingDimensions")}
            tooltip={t("agentConfig.embeddingDimensionsTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              min={1}
              step={256}
              disabled={!embeddingEnabled}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="embedding_config.enable_cache"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.embeddingEnableCache")}
            tooltip={t("agentConfig.embeddingEnableCacheTooltip")}
          >
            <EnableDisableSwitch
              disabled={!embeddingEnabled}
              enabled={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="embedding_config.max_cache_size"
        control={control}
        rules={{
          required: t("agentConfig.embeddingMaxCacheSizeRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.embeddingMaxCacheSize")}
            tooltip={t("agentConfig.embeddingMaxCacheSizeTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              min={1}
              step={100}
              disabled={!embeddingEnabled}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="embedding_config.max_input_length"
        control={control}
        rules={{
          required: t("agentConfig.embeddingMaxInputLengthRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.embeddingMaxInputLength")}
            tooltip={t("agentConfig.embeddingMaxInputLengthTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              min={1}
              step={1024}
              disabled={!embeddingEnabled}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="embedding_config.max_batch_size"
        control={control}
        rules={{
          required: t("agentConfig.embeddingMaxBatchSizeRequired"),
        }}
        render={({ field, fieldState }) => (
          <ConfigField
            label={t("agentConfig.embeddingMaxBatchSize")}
            tooltip={t("agentConfig.embeddingMaxBatchSizeTooltip")}
            error={fieldState.error?.message}
            required
          >
            <NumberInput
              style={{ width: "100%" }}
              min={1}
              step={1}
              disabled={!embeddingEnabled}
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
