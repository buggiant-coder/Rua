import { useEffect, useId, useState } from "react";
import { Globe, RefreshCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Btn, TextInput } from "@/ui";
import {
  desktopAutomationApi,
  type BrowserUseStatus,
} from "@/api/modules/desktopAutomation";
import { AutomationNotice, AutomationStatus } from "./AutomationFeedback";
import { hasBundleIntegrityError } from "./automationStatus";
import styles from "./index.module.less";

export function BrowserUseSettings() {
  const { t } = useTranslation();
  const pathId = useId();
  const [status, setStatus] = useState<BrowserUseStatus | null>(null);
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const accept = (value: BrowserUseStatus) => {
    setStatus(value);
    setPath(value.selected_path ?? "");
  };
  useEffect(() => {
    let active = true;
    desktopAutomationApi
      .browserStatus()
      .then((value) => {
        if (active) accept(value);
      })
      .catch((reason) => {
        if (active) setError(String(reason));
      });
    return () => {
      active = false;
    };
  }, []);

  const run = async (operation: () => Promise<BrowserUseStatus>) => {
    setBusy(true);
    setError("");
    try {
      accept(await operation());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const integrity = hasBundleIntegrityError(status?.diagnostics ?? []);
  const label = integrity
    ? "desktop.bundleInvalid"
    : status?.available
    ? "desktop.browserReady"
    : !status && !error
    ? "desktop.checking"
    : "desktop.browserUnavailable";

  return (
    <section className={styles.section} aria-label={t("desktop.browserTitle")}>
      <div className={styles.cardHeader}>
        <div className={styles.sectionIcon}>
          <Globe size={20} aria-hidden="true" />
        </div>
        <div className={styles.sectionHeader}>
          <h2>{t("desktop.browserTitle")}</h2>
          <AutomationStatus
            label={t(label)}
            tone={
              integrity ? "danger" : status?.available ? "success" : "neutral"
            }
          />
        </div>
      </div>
      <p className={styles.description}>{t("desktop.browserDescription")}</p>
      {status?.resolved_path && (
        <div className={styles.browserSelection}>
          <div className={styles.selectionLabel}>
            <span>{t("desktop.currentBrowser")}</span>
            <span>
              {t(
                status.selected_path
                  ? "desktop.browserManual"
                  : "desktop.browserDetected",
              )}
            </span>
          </div>
          <code className={styles.browserExecutable}>
            {status.resolved_path}
          </code>
        </div>
      )}
      {status && !status.available && (
        <AutomationNotice
          messages={status.diagnostics}
          title={t("desktop.browserUnavailable")}
          description={t(
            status.resolved_path
              ? "desktop.browserDriverHint"
              : "desktop.browserMissingHint",
          )}
        />
      )}
      {error && (
        <AutomationNotice
          alert
          messages={[error]}
          title={t("desktop.operationFailed")}
          description={t("desktop.browserRetryHint")}
        />
      )}
      <details className={styles.pathEditor}>
        <summary>
          {t(
            status?.resolved_path
              ? "desktop.browserChange"
              : "desktop.browserSelect",
          )}
        </summary>
        <div className={styles.browserPath}>
          <label htmlFor={pathId}>{t("desktop.browserPath")}</label>
          <TextInput
            id={pathId}
            value={path}
            onChange={(event) => setPath(event.target.value)}
            placeholder={
              status?.resolved_path ?? t("desktop.browserPathPlaceholder")
            }
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
          />
          <p className={styles.hint}>{t("desktop.browserChangeHint")}</p>
          <div className={styles.actions}>
            <Btn
              size="sm"
              disabled={busy || !path.trim()}
              onClick={() =>
                void run(() => desktopAutomationApi.selectBrowser(path.trim()))
              }
            >
              {t("desktop.browserSave")}
            </Btn>
            <Btn
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() =>
                void run(() => desktopAutomationApi.selectBrowser(null))
              }
            >
              {t("desktop.browserAutomatic")}
            </Btn>
          </div>
        </div>
      </details>
      <div className={styles.cardFooter}>
        <Btn
          variant="outline"
          size="sm"
          disabled={busy}
          loading={busy}
          icon={<RefreshCcw size={14} />}
          onClick={() => void run(desktopAutomationApi.recheckBrowser)}
        >
          {t("desktop.browserRecheck")}
        </Btn>
        {status?.device_id && (
          <span className={styles.metadata}>
            {t("desktop.browserExecutionDevice", { device: status.device_id })}
          </span>
        )}
      </div>
    </section>
  );
}
