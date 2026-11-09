import { Select } from "@/ui";
import { useTranslation } from "react-i18next";
import { TIMEZONE_OPTIONS } from "../../../../constants/timezone";
import { RuaCard } from "@/components/RuaCard";
import { ConfigField } from "./ConfigField";
import styles from "../index.module.less";

const LANGUAGE_OPTIONS = [
  { value: "zh", label: "中文" },
  { value: "en", label: "English" },
  { value: "ru", label: "Русский" },
];

interface ReactAgentCardProps {
  language: string;
  savingLang: boolean;
  onLanguageChange: (value: string) => void;
  timezone: string;
  savingTimezone: boolean;
  onTimezoneChange: (value: string) => void;
}

export function ReactAgentCard({
  language,
  savingLang,
  onLanguageChange,
  timezone,
  savingTimezone,
  onTimezoneChange,
}: ReactAgentCardProps) {
  const { t } = useTranslation();

  return (
    <RuaCard
      bare
      className={styles.formCard}
      title={t("agentConfig.reactAgentTitle")}
    >
      <div className={styles.reactAgentRow}>
        <ConfigField
          label={t("agentConfig.language")}
          tooltip={t("agentConfig.languageTooltip")}
          className={styles.reactAgentField}
        >
          <Select
            value={language}
            options={LANGUAGE_OPTIONS}
            onChange={onLanguageChange}
            loading={savingLang}
            disabled={savingLang}
            style={{ width: "100%" }}
          />
        </ConfigField>

        <ConfigField
          label={t("agentConfig.timezone")}
          tooltip={t("agentConfig.timezoneTooltip")}
          className={styles.reactAgentField}
        >
          <Select
            showSearch
            value={timezone}
            placeholder={t("agentConfig.selectTimezone")}
            filterOption={(input, option) =>
              (option?.label?.toString() || "")
                .toLowerCase()
                .includes(input.toLowerCase())
            }
            options={TIMEZONE_OPTIONS}
            onChange={onTimezoneChange}
            loading={savingTimezone}
            disabled={savingTimezone}
            style={{ width: "100%" }}
          />
        </ConfigField>
      </div>
    </RuaCard>
  );
}
