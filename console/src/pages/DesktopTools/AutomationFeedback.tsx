import { AlertCircle } from "lucide-react";
import { useTranslation } from "react-i18next";
import { hasBundleIntegrityError } from "./automationStatus";
import styles from "./index.module.less";

export function AutomationStatus({
  label,
  tone = "neutral",
}: {
  label: string;
  tone?: "neutral" | "success" | "warning" | "danger";
}) {
  const tones = {
    neutral: styles.statusNeutral,
    success: styles.statusOk,
    warning: styles.statusWarning,
    danger: styles.statusDanger,
  };
  return (
    <span role="status" className={`${styles.status} ${tones[tone]}`}>
      {label}
    </span>
  );
}

export function AutomationNotice({
  messages,
  title,
  description,
  alert = false,
}: {
  messages: string[];
  title: string;
  description?: string;
  alert?: boolean;
}) {
  const { t } = useTranslation();
  const integrity = hasBundleIntegrityError(messages);
  const missingHost = messages.some(
    (message) =>
      message ===
        "Browser Use is available in Rua Desktop on macOS and Windows." ||
      message ===
        "Computer Use requires Rua Desktop on the execution device." ||
      message.startsWith("The execution device has not loaded"),
  );
  return (
    <div className={styles.notice} role={alert ? "alert" : undefined}>
      <AlertCircle size={16} aria-hidden="true" />
      <div>
        <p className={styles.noticeTitle}>
          {integrity
            ? t("desktop.bundleIntegrityTitle")
            : missingHost
            ? t("desktop.hostRequiredTitle")
            : title}
        </p>
        <p className={styles.hint}>
          {integrity
            ? t("desktop.bundleIntegrityHint")
            : missingHost
            ? t("desktop.hostRequiredHint")
            : description}
        </p>
        {messages.length > 0 && (
          <details className={styles.diagnostics}>
            <summary>{t("desktop.technicalDetails")}</summary>
            {messages.map((message, index) => (
              <pre key={index}>{message}</pre>
            ))}
          </details>
        )}
      </div>
    </div>
  );
}
