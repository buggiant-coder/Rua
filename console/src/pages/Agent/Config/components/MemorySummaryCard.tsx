import { EnableDisableSwitch } from "@/ui";
import { Controller, useFormContext } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { RuaCard } from "@/components/RuaCard";
import { InputWithExample } from "@/components/InputWithExample";
import { ConfigField } from "./ConfigField";
import type { AgentConfigFormValues } from "../useAgentConfig";
import styles from "../index.module.less";

export function MemorySummaryCard() {
  const { t } = useTranslation();
  const { control } = useFormContext<AgentConfigFormValues>();

  return (
    <RuaCard
      bare
      className={styles.formCard}
      title={t("agentConfig.memorySummaryTitle")}
    >
      <Controller
        name="memory_summary.memory_summary_enabled"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.memorySummaryEnabled")}
            tooltip={t("agentConfig.memorySummaryEnabledTooltip")}
          >
            <EnableDisableSwitch
              enabled={field.value}
              onChange={field.onChange}
            />
          </ConfigField>
        )}
      />

      <Controller
        name="memory_summary.dream_cron"
        control={control}
        render={({ field }) => (
          <ConfigField
            label={t("agentConfig.dreamCron")}
            tooltip={t("agentConfig.dreamCronTooltip")}
          >
            <InputWithExample
              placeholder={t("agentConfig.dreamCronPlaceholder")}
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
