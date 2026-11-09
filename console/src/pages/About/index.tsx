import { useCallback, useEffect, useState } from "react";
import {
  Check,
  Download,
  DownloadCloud,
  RefreshCcw,
  ServerCog,
  Terminal,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { EmptyState } from "@/components/EmptyState";
import { Page, PageBody } from "@/components/Page";
import { PageHeader } from "@/components/PageHeader";
import { Btn, Modal } from "@/ui";
import toast from "@/ui/Toast";
import {
  checkDesktopUpdate,
  getDesktopInstallStatus,
  getDesktopUpdateStatus,
  installDesktopCommandLineTool,
  installDesktopUpdate,
  migrateDesktopLegacyRuntime,
  type DesktopInstallStatus,
  type DesktopUpdateStatus,
} from "@/api/modules/desktopInstallation";
import styles from "./index.module.less";

export default function AboutPage() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<DesktopInstallStatus | null>(null);
  const [updateStatus, setUpdateStatus] = useState<DesktopUpdateStatus | null>(
    null,
  );
  const [loading, setLoading] = useState(true);
  const [version, setVersion] = useState<string | null>(null);
  const [action, setAction] = useState<
    "install" | "migrate" | "check-update" | "install-update" | null
  >(null);
  const [error, setError] = useState("");
  const [migrationOpen, setMigrationOpen] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [installStatus, updaterStatus] = await Promise.all([
        getDesktopInstallStatus(),
        getDesktopUpdateStatus(),
      ]);
      setStatus(installStatus);
      setUpdateStatus(updaterStatus);
      setVersion(updaterStatus.currentVersion);
    } catch (loadError) {
      setError(errorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let active = true;
    // Updates are accepted before the resident restarts; poll the shared status.
    const timer = window.setInterval(() => {
      void getDesktopUpdateStatus()
        .then((value) => {
          if (active) setUpdateStatus(value);
        })
        .catch(() => {
          // The updater may briefly replace the resident process.
        });
    }, 2000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, []);

  const installCli = async () => {
    setAction("install");
    try {
      setStatus(await installDesktopCommandLineTool());
      toast.success(t("desktop.cliInstallSuccess"));
    } catch (installError) {
      toast.error(errorMessage(installError));
    } finally {
      setAction(null);
    }
  };

  const migrateLegacy = async () => {
    setAction("migrate");
    try {
      await migrateDesktopLegacyRuntime();
      setMigrationOpen(false);
      toast.success(t("desktop.migrationStarted"));
    } catch (migrationError) {
      toast.error(errorMessage(migrationError));
    } finally {
      setAction(null);
    }
  };

  const checkUpdate = async () => {
    setAction("check-update");
    try {
      setUpdateStatus(await checkDesktopUpdate());
    } catch (updateError) {
      toast.error(errorMessage(updateError));
    } finally {
      setAction(null);
    }
  };

  const installUpdate = async () => {
    setAction("install-update");
    try {
      await installDesktopUpdate();
      setUpdateStatus((current) =>
        current ? { ...current, phase: "downloading" } : current,
      );
      toast.success(t("desktop.updateStarted"));
    } catch (updateError) {
      toast.error(errorMessage(updateError));
    } finally {
      setAction(null);
    }
  };

  return (
    <Page>
      <PageHeader
        current={t("about.title")}
        extra={
          <Btn
            variant="outline"
            size="sm"
            icon={<RefreshCcw size={16} />}
            onClick={() => void refresh()}
            loading={loading}
          >
            {t("common.refresh")}
          </Btn>
        }
      />
      <PageBody className={styles.body}>
        <div className={styles.content}>
          <div className={styles.intro}>
            <h1>{t("about.title")}</h1>
            <p>{t("about.description")}</p>
            {version && (
              <p className={styles.version}>
                {t("about.version", { version })}
              </p>
            )}
          </div>
          {error ? (
            <EmptyState
              variant="error"
              description={error}
              onRetry={() => void refresh()}
            />
          ) : null}

          {!error && status ? (
            <>
              <div className={styles.integrationGrid}>
                {updateStatus ? (
                  <section className={styles.section}>
                    <div className={styles.cardHeader}>
                      <div className={styles.sectionIcon}>
                        <DownloadCloud size={20} aria-hidden="true" />
                      </div>
                      <div className={styles.sectionHeader}>
                        <h2>{t("desktop.updateTitle")}</h2>
                        <StatusLabel
                          ok={updateStatus.phase === "up_to_date"}
                          warning={
                            updateStatus.phase === "available" ||
                            updateStatus.phase === "error"
                          }
                          text={t(`desktop.updatePhase.${updateStatus.phase}`)}
                        />
                      </div>
                    </div>
                    <p className={styles.description}>
                      {updateStatus.phase === "unavailable"
                        ? t("about.updateHostUnavailable")
                        : updateStatus.availableVersion
                        ? t("desktop.updateAvailableDescription", {
                            current: updateStatus.currentVersion,
                            available: updateStatus.availableVersion,
                          })
                        : t("desktop.updateDescription", {
                            version: updateStatus.currentVersion,
                          })}
                    </p>
                    {updateStatus.error ? (
                      <p className={styles.warning}>{updateStatus.error}</p>
                    ) : null}
                    <div className={styles.cardFooter}>
                      <div className={styles.actions}>
                        <Btn
                          variant="outline"
                          size="sm"
                          icon={<RefreshCcw size={14} />}
                          onClick={() => void checkUpdate()}
                          loading={action === "check-update"}
                          disabled={
                            action !== null ||
                            [
                              "unavailable",
                              "checking",
                              "downloading",
                              "installing",
                            ].includes(updateStatus.phase)
                          }
                        >
                          {t("desktop.checkForUpdates")}
                        </Btn>
                        {updateStatus.phase === "available" ? (
                          <Btn
                            size="sm"
                            icon={<Download size={14} />}
                            onClick={() => void installUpdate()}
                            loading={action === "install-update"}
                            disabled={action !== null}
                          >
                            {t("desktop.installUpdate")}
                          </Btn>
                        ) : null}
                      </div>
                    </div>
                  </section>
                ) : null}
                <section className={styles.section}>
                  <div className={styles.cardHeader}>
                    <div className={styles.sectionIcon}>
                      <Terminal size={20} aria-hidden="true" />
                    </div>
                    <div className={styles.sectionHeader}>
                      <h2>{t("desktop.cliTitle")}</h2>
                      {status.platform !== "windows" ? (
                        <StatusLabel
                          ok={status.cliInstalled}
                          warning={status.cliConflict}
                          text={
                            status.cliInstalled
                              ? t("desktop.installed")
                              : status.cliConflict
                              ? t("desktop.conflict")
                              : t("desktop.notInstalled")
                          }
                        />
                      ) : (
                        <StatusLabel ok text={t("desktop.installerManaged")} />
                      )}
                    </div>
                  </div>
                  <p className={styles.description}>
                    {t(
                      status.platform !== "windows"
                        ? "desktop.cliDescription"
                        : "desktop.windowsCliManaged",
                    )}
                  </p>
                  {status.platform !== "windows" ? (
                    <>
                      <code className={styles.path}>{status.cliPath}</code>
                      {status.cliConflict ? (
                        <p className={styles.warning}>
                          {t("desktop.cliConflictDescription")}
                        </p>
                      ) : null}
                      <div className={styles.cardFooter}>
                        <Btn
                          size="sm"
                          variant="outline"
                          icon={
                            status.cliInstalled ? (
                              <Check size={14} />
                            ) : (
                              <Download size={14} />
                            )
                          }
                          onClick={() => void installCli()}
                          loading={action === "install"}
                          disabled={
                            action !== null ||
                            status.cliInstalled ||
                            status.cliConflict
                          }
                        >
                          {status.cliInstalled
                            ? t("desktop.installed")
                            : t("desktop.installCli")}
                        </Btn>
                      </div>
                    </>
                  ) : null}
                </section>
              </div>
              {status.legacyRuntime ? (
                <section className={styles.section}>
                  <div className={styles.sectionIcon}>
                    <ServerCog size={20} />
                  </div>
                  <div className={styles.sectionBody}>
                    <div className={styles.sectionHeader}>
                      <div>
                        <h2>{t("desktop.legacyTitle")}</h2>
                        <p>
                          {t("desktop.legacyDescription", {
                            port: status.legacyPort,
                          })}
                        </p>
                      </div>
                      <StatusLabel warning text={t("desktop.attached")} />
                    </div>
                    <div className={styles.actions}>
                      <Btn
                        onClick={() => setMigrationOpen(true)}
                        disabled={action !== null}
                      >
                        {t("desktop.migrate")}
                      </Btn>
                    </div>
                  </div>
                </section>
              ) : null}
            </>
          ) : null}
        </div>
      </PageBody>

      <Modal
        open={migrationOpen}
        onClose={() => setMigrationOpen(false)}
        title={t("desktop.migrationConfirmTitle")}
        onOk={migrateLegacy}
        okText={t("desktop.migrate")}
        cancelText={t("common.cancel")}
        confirmLoading={action === "migrate"}
        maskClosable={action !== "migrate"}
      >
        <p className={styles.modalText}>
          {t("desktop.migrationConfirmDescription")}
        </p>
        <p className={styles.modalText}>{t("desktop.dataPreserved")}</p>
      </Modal>
    </Page>
  );
}

function StatusLabel({
  ok = false,
  warning = false,
  text,
}: {
  ok?: boolean;
  warning?: boolean;
  text: string;
}) {
  const className = warning
    ? styles.statusWarning
    : ok
    ? styles.statusOk
    : styles.statusNeutral;
  return <span className={`${styles.status} ${className}`}>{text}</span>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
