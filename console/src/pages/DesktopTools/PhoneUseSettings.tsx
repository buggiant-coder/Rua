import { useEffect, useState } from "react";
import { RefreshCcw, Smartphone } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Btn } from "@/ui";
import { phoneUseApi, type PhoneUseStatus } from "@/api/modules/phoneUse";
import { AutomationNotice, AutomationStatus } from "./AutomationFeedback";
import styles from "./index.module.less";

export function PhoneUseSettings() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<PhoneUseStatus | null>(null);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    phoneUseApi
      .status()
      .then((value) => {
        if (active) setStatus(value);
      })
      .catch((reason: unknown) => {
        if (active)
          setError(reason instanceof Error ? reason.message : String(reason));
      })
      .finally(() => {
        if (active) setBusy(false);
      });
    return () => {
      active = false;
    };
  }, []);
  const refresh = async () => {
    setBusy(true);
    setError("");
    try {
      setStatus(await phoneUseApi.status());
    } catch (reason) {
      setStatus(null);
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const unsupported = error.includes(
    "Phone Use requires an Android execution device",
  );
  const permissions = [
    {
      key: "accessibility",
      label: "phone.accessibility",
      hint: "phone.accessibilityHint",
      granted: status?.accessibility,
    },
    {
      key: "camera",
      label: "phone.camera",
      hint: "phone.cameraHint",
      granted: status?.camera_permission,
    },
  ];
  const label = busy
    ? "desktop.permissionUnknown"
    : status?.state === "executing"
    ? "phone.executing"
    : status
    ? "phone.connected"
    : "phone.unavailable";
  return (
    <section className={styles.section} aria-label={t("phone.title")}>
      <div className={styles.cardHeader}>
        <div className={styles.sectionIcon}>
          <Smartphone size={20} aria-hidden="true" />
        </div>
        <div className={styles.sectionHeader}>
          <h2>{t("phone.title")}</h2>
          <AutomationStatus
            label={t(label)}
            tone={
              busy || !status
                ? "neutral"
                : status.state === "executing"
                ? "warning"
                : "success"
            }
          />
        </div>
      </div>
      <p className={styles.description}>{t("phone.description")}</p>
      {status && (
        <div className={styles.permissions}>
          {permissions.map((permission) => (
            <div key={permission.key} className={styles.permissionRow}>
              <div>
                <span className={styles.permissionName}>
                  {t(permission.label)}
                </span>
                <p className={styles.hint}>{t(permission.hint)}</p>
              </div>
              <span
                className={
                  permission.granted
                    ? styles.permissionGranted
                    : styles.permissionMissing
                }
              >
                {t(permission.granted ? "phone.granted" : "phone.required")}
              </span>
            </div>
          ))}
        </div>
      )}
      {error ? (
        <AutomationNotice
          messages={unsupported ? [] : [error]}
          title={t(
            unsupported ? "phone.hostRequiredTitle" : "phone.unavailableTitle",
          )}
          description={t(
            unsupported ? "phone.hostRequiredHint" : "phone.unavailableHint",
          )}
          alert={!unsupported}
        />
      ) : (
        status && <p className={styles.hint}>{t("phone.permissionHint")}</p>
      )}
      <div className={styles.cardFooter}>
        <Btn
          variant="outline"
          size="sm"
          disabled={busy}
          loading={busy}
          icon={<RefreshCcw size={14} />}
          onClick={() => void refresh()}
        >
          {t("desktop.browserRecheck")}
        </Btn>
      </div>
    </section>
  );
}
