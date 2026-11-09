import { useEffect, useState } from "react";
import { Monitor, RefreshCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Btn } from "@/ui";
import {
  desktopAutomationApi,
  type ComputerUseStatus,
} from "@/api/modules/desktopAutomation";
import { AutomationNotice, AutomationStatus } from "./AutomationFeedback";
import { hasBundleIntegrityError } from "./automationStatus";
import styles from "./index.module.less";

export function ComputerUseSettings() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<ComputerUseStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    desktopAutomationApi
      .computerStatus()
      .then((value) => {
        if (active) setStatus(value);
      })
      .catch((reason) => {
        if (active) setError(String(reason));
      });
    return () => {
      active = false;
    };
  }, []);
  const recheck = async (restart = false) => {
    setBusy(true);
    setError("");
    try {
      setStatus(
        await (restart
          ? desktopAutomationApi.restartComputer()
          : desktopAutomationApi.recheckComputer()),
      );
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const permissions = async (
    setting?: "accessibility" | "screen_recording",
  ) => {
    setBusy(true);
    setError("");
    try {
      if (setting)
        await desktopAutomationApi.openComputerPermissionSettings(setting);
      else await desktopAutomationApi.requestComputerPermissions();
      setStatus(await desktopAutomationApi.computerStatus());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const integrity = hasBundleIntegrityError(status?.diagnostics ?? []);
  const label = integrity
    ? "desktop.bundleInvalid"
    : status?.state === "ready"
    ? "desktop.browserReady"
    : status?.state === "permission_required"
    ? "desktop.computerPermissionRequired"
    : status?.state === "restart_required"
    ? "desktop.computerRestartRequired"
    : !status && !error
    ? "desktop.checking"
    : "desktop.browserUnavailable";
  const permissionRows =
    status?.platform === "macos" && status.permissions && !integrity
      ? [
          {
            setting: "accessibility" as const,
            label: "desktop.accessibility",
            hint: "desktop.accessibilityHint",
            action: "desktop.computerAccessibility",
            granted: status.permissions.accessibility,
          },
          {
            setting: "screen_recording" as const,
            label: "desktop.screenRecording",
            hint: "desktop.screenRecordingHint",
            action: "desktop.computerScreenRecording",
            granted: status.permissions.screen_recording,
          },
        ]
      : [];

  return (
    <section className={styles.section} aria-label={t("desktop.computerTitle")}>
      <div className={styles.cardHeader}>
        <div className={styles.sectionIcon}>
          <Monitor size={20} aria-hidden="true" />
        </div>
        <div className={styles.sectionHeader}>
          <h2>{t("desktop.computerTitle")}</h2>
          <AutomationStatus
            label={t(label)}
            tone={
              integrity
                ? "danger"
                : status?.state === "ready"
                ? "success"
                : status?.state === "permission_required" ||
                  status?.state === "restart_required"
                ? "warning"
                : "neutral"
            }
          />
        </div>
      </div>
      <p className={styles.description}>{t("desktop.computerDescription")}</p>
      {permissionRows.length > 0 && (
        <div className={styles.permissions}>
          {permissionRows.map((row) => (
            <div className={styles.permissionRow} key={row.setting}>
              <div>
                <span className={styles.permissionName}>{t(row.label)}</span>
                <p className={styles.hint}>{t(row.hint)}</p>
              </div>
              <span
                className={
                  row.granted
                    ? styles.permissionGranted
                    : styles.permissionMissing
                }
              >
                {t(
                  row.granted === null
                    ? "desktop.permissionUnknown"
                    : row.granted
                    ? "desktop.permissionGranted"
                    : "desktop.permissionMissing",
                )}
              </span>
              <Btn
                variant="ghost"
                size="sm"
                disabled={busy}
                aria-label={t(row.action)}
                onClick={() => void permissions(row.setting)}
              >
                {t("desktop.openSettings")}
              </Btn>
            </div>
          ))}
        </div>
      )}
      {status?.state === "permission_required" && permissionRows.length > 0 && (
        <p className={styles.hint}>{t("desktop.computerPermissionHint")}</p>
      )}
      {status &&
        (status.state === "unavailable" ||
          status.state === "restart_required") && (
          <AutomationNotice
            messages={status.diagnostics}
            title={t(
              status.state === "restart_required"
                ? "desktop.computerRestartRequired"
                : "desktop.browserUnavailable",
            )}
            description={t(
              status.state === "restart_required"
                ? "desktop.computerRestartHint"
                : "desktop.computerUnavailableHint",
            )}
          />
        )}
      {error && (
        <AutomationNotice
          alert
          messages={[error]}
          title={t("desktop.operationFailed")}
          description={t("desktop.computerRetryHint")}
        />
      )}
      <div className={styles.cardFooter}>
        <div className={styles.actions}>
          {status?.state === "permission_required" &&
            permissionRows.length > 0 && (
              <Btn size="sm" disabled={busy} onClick={() => void permissions()}>
                {t("desktop.computerGrant")}
              </Btn>
            )}
          {status?.state === "restart_required" && (
            <Btn size="sm" disabled={busy} onClick={() => void recheck(true)}>
              {t("desktop.computerRestart")}
            </Btn>
          )}
          <Btn
            variant="outline"
            size="sm"
            disabled={busy}
            loading={busy}
            icon={<RefreshCcw size={14} />}
            onClick={() => void recheck()}
          >
            {t("desktop.browserRecheck")}
          </Btn>
        </div>
        {status?.device_id && (
          <span className={styles.metadata}>
            {t("desktop.browserExecutionDevice", { device: status.device_id })}
          </span>
        )}
      </div>
    </section>
  );
}
