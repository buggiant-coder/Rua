import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight, RefreshCw, Repeat2 } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";

import { EmptyState } from "@/components/EmptyState";
import DirectoryPicker from "@/components/DirectoryPicker";
import {
  DeviceConnectivityTag,
  deviceConnectivityLabel,
} from "@/components/DeviceConnectivity";
import { Page, PageBody } from "@/components/Page";
import { PageHeader } from "@/components/PageHeader";
import { Btn, Loading, toast } from "@/ui";
import type { DeviceDto } from "@/api/modules/sync";
import {
  syncTransferApi,
  type SyncTransferEntry,
  type SyncTransferJob,
} from "@/api/modules/syncTransfer";
import {
  notifyMultiDeviceFeaturesChanged,
  useMultiDeviceFeatureAvailability,
} from "@/hooks/useMultiDeviceFeatureAvailability";
import styles from "./index.module.less";

type SelectedSource = {
  path: string;
  name: string;
  size?: number;
  kind: SyncTransferEntry["kind"];
};

function parentDirectory(path: string): string | null {
  const clean = path.replace(/[\\/]+$/, "");
  const separator = Math.max(clean.lastIndexOf("/"), clean.lastIndexOf("\\"));
  if (separator < 0) return null;
  if (separator === 0) return clean.slice(0, 1);
  return clean.slice(0, separator);
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

function deviceLabel(device: DeviceDto | undefined, fallback: string): string {
  return device?.friendly_name || device?.label || fallback;
}

export default function AirdropTransferPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const {
    devices,
    loaded: devicesLoaded,
    available: multiDeviceFeaturesAvailable,
  } = useMultiDeviceFeatureAvailability();
  const [sourceDeviceId, setSourceDeviceId] = useState("");
  const [targetDeviceId, setTargetDeviceId] = useState("");
  const [sourcePath, setSourcePath] = useState("~");
  const [targetPath, setTargetPath] = useState("~");
  const [sourceEntries, setSourceEntries] = useState<SyncTransferEntry[]>([]);
  const [targetEntries, setTargetEntries] = useState<SyncTransferEntry[]>([]);
  const [selected, setSelected] = useState<Record<string, SelectedSource>>({});
  const [jobs, setJobs] = useState<SyncTransferJob[]>([]);
  const [loadingSource, setLoadingSource] = useState(false);
  const [loadingTarget, setLoadingTarget] = useState(false);
  const [sending, setSending] = useState(false);

  const sourceDevice = devices.find(
    (device) => device.device_id === sourceDeviceId,
  );
  const targetDevice = devices.find(
    (device) => device.device_id === targetDeviceId,
  );
  const selectedItems = Object.values(selected);
  const selectedBytes = selectedItems.reduce(
    (sum, item) => sum + (item.size ?? 0),
    0,
  );

  const loadJobs = useCallback(async () => {
    try {
      setJobs(await syncTransferApi.listJobs());
    } catch {}
  }, []);

  const loadDirectory = useCallback(
    async (role: "source" | "target", deviceId: string, path: string) => {
      if (!deviceId) return;
      if (role === "source") setLoadingSource(true);
      else setLoadingTarget(true);
      try {
        const result = await syncTransferApi.listDirectory(deviceId, path);
        if (role === "source") {
          setSourcePath(result.path);
          setSourceEntries(result.entries);
        } else {
          setTargetPath(result.path);
          setTargetEntries(result.entries);
        }
      } catch (error) {
        if (role === "source") {
          setSourceEntries([]);
        } else {
          setTargetEntries([]);
        }
        toast.error(
          error instanceof Error
            ? error.message
            : t("airdropTransfer.listFailed"),
        );
      } finally {
        if (role === "source") setLoadingSource(false);
        else setLoadingTarget(false);
      }
    },
    [t],
  );

  const browseSource = useCallback(
    async (path: string) => {
      const result = await syncTransferApi.listDirectory(
        sourceDeviceId,
        path.trim() || "~",
      );
      return {
        path: result.path,
        parent: parentDirectory(result.path),
        data: result,
        directories: result.entries
          .filter((entry) => entry.kind === "directory")
          .map((entry) => ({ name: entry.name, path: entry.path })),
      };
    },
    [sourceDeviceId],
  );

  const browseTarget = useCallback(
    async (path: string) => {
      const result = await syncTransferApi.listDirectory(
        targetDeviceId,
        path.trim() || "~",
      );
      return {
        path: result.path,
        parent: parentDirectory(result.path),
        data: result,
        directories: result.entries
          .filter((entry) => entry.kind === "directory")
          .map((entry) => ({ name: entry.name, path: entry.path })),
      };
    },
    [targetDeviceId],
  );

  useEffect(() => {
    if (!multiDeviceFeaturesAvailable) return;
    setSourceDeviceId((current) =>
      devices.some((device) => device.device_id === current)
        ? current
        : devices[0]?.device_id || "",
    );
    setTargetDeviceId((current) =>
      devices.some((device) => device.device_id === current)
        ? current
        : devices.find((device) => !device.is_self)?.device_id ||
          devices[0]?.device_id ||
          "",
    );
  }, [devices, multiDeviceFeaturesAvailable]);

  useEffect(() => {
    if (!multiDeviceFeaturesAvailable) return;
    loadJobs();
  }, [loadJobs, multiDeviceFeaturesAvailable]);

  useEffect(() => {
    if (!multiDeviceFeaturesAvailable) return;
    loadDirectory("source", sourceDeviceId, "~");
  }, [loadDirectory, multiDeviceFeaturesAvailable, sourceDeviceId]);

  useEffect(() => {
    if (!multiDeviceFeaturesAvailable) return;
    loadDirectory("target", targetDeviceId, "~");
  }, [loadDirectory, multiDeviceFeaturesAvailable, targetDeviceId]);

  useEffect(() => {
    if (!multiDeviceFeaturesAvailable) return;
    const id = window.setInterval(loadJobs, 2000);
    return () => window.clearInterval(id);
  }, [loadJobs, multiDeviceFeaturesAvailable]);

  const toggleSelected = (entry: SyncTransferEntry) => {
    setSelected((current) => {
      const next = { ...current };
      if (next[entry.path]) {
        delete next[entry.path];
      } else {
        next[entry.path] = {
          path: entry.path,
          name: entry.name,
          size: entry.size,
          kind: entry.kind,
        };
      }
      return next;
    });
  };

  const startTransfer = async () => {
    if (!sourceDeviceId || !targetDeviceId || selectedItems.length === 0)
      return;
    setSending(true);
    try {
      const job = await syncTransferApi.send({
        source_device_id: sourceDeviceId,
        source_paths: selectedItems.map((item) => item.path),
        target_device_id: targetDeviceId,
        target_dir: targetPath,
        conflict: "rename",
      });
      setJobs((current) => [
        job,
        ...current.filter((item) => item.job_id !== job.job_id),
      ]);
      toast.success(t("airdropTransfer.started"));
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("airdropTransfer.sendFailed"),
      );
    } finally {
      setSending(false);
    }
  };

  const swapDirection = () => {
    setSourceDeviceId(targetDeviceId);
    setTargetDeviceId(sourceDeviceId);
    setSourcePath(targetPath);
    setTargetPath(sourcePath);
    setSourceEntries(targetEntries);
    setTargetEntries(sourceEntries);
    setSelected({});
  };

  const handleSourceDeviceChange = (deviceId: string) => {
    setSourceDeviceId(deviceId);
    setSourcePath("~");
    setSourceEntries([]);
    setSelected({});
  };

  const handleTargetDeviceChange = (deviceId: string) => {
    setTargetDeviceId(deviceId);
    setTargetPath("~");
    setTargetEntries([]);
  };

  const renderSourceRow = (entry: SyncTransferEntry) => {
    const checked = Boolean(selected[entry.path]);
    return (
      <div
        key={entry.path}
        className={`${styles.row} ${checked ? styles.rowSelected : ""}`}
      >
        <input
          type="checkbox"
          checked={checked}
          onChange={() => toggleSelected(entry)}
          aria-label={entry.name}
        />
        <div className={styles.iconBox}>
          {entry.kind === "directory" ? "D" : "F"}
        </div>
        <div>
          <div className={styles.name}>{entry.name}</div>
          <div className={styles.meta}>
            {entry.kind === "directory"
              ? t("airdropTransfer.directory")
              : formatBytes(entry.size ?? 0)}
          </div>
        </div>
        {entry.kind === "directory" ? (
          <Btn
            variant="ghost"
            icon={<ChevronRight size={16} />}
            onClick={() => setSourcePath(entry.path)}
          />
        ) : (
          <span />
        )}
      </div>
    );
  };

  const renderTargetRow = (entry: SyncTransferEntry) => (
    <div key={entry.path} className={`${styles.row} ${styles.rowNoCheck}`}>
      <div className={styles.iconBox}>
        {entry.kind === "directory" ? "D" : "F"}
      </div>
      <div>
        <div className={styles.name}>{entry.name}</div>
        <div className={styles.meta}>
          {entry.kind === "directory"
            ? t("airdropTransfer.directory")
            : formatBytes(entry.size ?? 0)}
        </div>
      </div>
      {entry.kind === "directory" ? (
        <Btn
          variant="ghost"
          icon={<ChevronRight size={16} />}
          onClick={() => setTargetPath(entry.path)}
        />
      ) : (
        <span />
      )}
    </div>
  );

  const summary = useMemo(
    () =>
      `${deviceLabel(sourceDevice, sourceDeviceId || "-")} · ${
        selectedItems.length
      } · ${formatBytes(selectedBytes)}`,
    [selectedBytes, selectedItems.length, sourceDevice, sourceDeviceId],
  );

  if (!devicesLoaded) {
    return (
      <Page>
        <PageHeader current={t("nav.airdropTransfer")} />
        <PageBody>
          <Loading />
        </PageBody>
      </Page>
    );
  }

  if (!multiDeviceFeaturesAvailable) {
    return (
      <Page>
        <PageHeader current={t("nav.airdropTransfer")} />
        <PageBody>
          <EmptyState
            variant="not-available"
            description={t("airdropTransfer.unavailable")}
            action={
              <Btn
                variant="filled"
                onClick={() => navigate("/settings/device-sync")}
              >
                {t("airdropTransfer.goToDeviceSync")}
              </Btn>
            }
          />
        </PageBody>
      </Page>
    );
  }

  return (
    <Page>
      <PageHeader current={t("nav.airdropTransfer")} />
      <PageBody>
        <div className={styles.summary}>
          <div>
            <div className={styles.summaryLine}>
              <span className={styles.pill}>{summary}</span>
              <span className={styles.arrow}>→</span>
              <span className={styles.pill}>
                {deviceLabel(targetDevice, targetDeviceId || "-")} ·{" "}
                {targetPath}
              </span>
            </div>
          </div>
          <div className={styles.summaryActions}>
            <Btn
              variant="ghost"
              icon={<RefreshCw size={16} />}
              onClick={notifyMultiDeviceFeaturesChanged}
            />
            <Btn
              variant="ghost"
              icon={<Repeat2 size={16} />}
              onClick={swapDirection}
            >
              {t("airdropTransfer.swap")}
            </Btn>
            <Btn variant="ghost" onClick={() => setSelected({})}>
              {t("airdropTransfer.clear")}
            </Btn>
            <Btn
              type="button"
              disabled={
                !sourceDeviceId || !targetDeviceId || selectedItems.length === 0
              }
              loading={sending}
              onClick={startTransfer}
            >
              {t("airdropTransfer.start")}
            </Btn>
          </div>
        </div>

        <div className={styles.grid}>
          <section className={styles.panel}>
            <div className={styles.panelTop}>
              <div>
                <div className={styles.roleTitle}>
                  {t("airdropTransfer.sourceDevice")}
                </div>
                <div className={styles.roleHint}>
                  {t("airdropTransfer.sourceHint")}
                </div>
              </div>
              {sourceDevice && (
                <DeviceConnectivityTag
                  state={sourceDevice.connectivity_state}
                />
              )}
            </div>
            <div className={styles.deviceRow}>
              <select
                className={styles.select}
                value={sourceDeviceId}
                onChange={(event) =>
                  handleSourceDeviceChange(event.target.value)
                }
              >
                {devices.map((device) => (
                  <option key={device.device_id} value={device.device_id}>
                    {deviceLabel(device, device.device_id)} ·{" "}
                    {deviceConnectivityLabel(t, device.connectivity_state)}
                  </option>
                ))}
              </select>
              <Btn
                icon={<RefreshCw size={16} />}
                onClick={() =>
                  loadDirectory("source", sourceDeviceId, sourcePath)
                }
              />
            </div>
            <DirectoryPicker
              value={sourcePath}
              onChange={setSourcePath}
              browse={browseSource}
              disabled={!sourceDeviceId}
              placeholder="~"
              openLabel={t("airdropTransfer.browseSource", "浏览源目录")}
              closeLabel={t("airdropTransfer.closeBrowser", "关闭目录浏览")}
              emptyLabel={t("airdropTransfer.noDirectories", "没有子目录")}
              onListingChange={(listing) => {
                if (!listing.data) return;
                setSourcePath(listing.path);
                setSourceEntries(listing.data.entries);
                setSelected({});
              }}
              onError={(error) =>
                toast.error(
                  error instanceof Error ? error.message : String(error),
                )
              }
            />
            <div className={styles.list}>
              {loadingSource ? <Loading /> : sourceEntries.map(renderSourceRow)}
            </div>
          </section>

          <section className={styles.panel}>
            <div className={styles.panelTop}>
              <div>
                <div className={styles.roleTitle}>
                  {t("airdropTransfer.targetDevice")}
                </div>
                <div className={styles.roleHint}>
                  {t("airdropTransfer.targetHint")}
                </div>
              </div>
              {targetDevice && (
                <DeviceConnectivityTag
                  state={targetDevice.connectivity_state}
                />
              )}
            </div>
            <div className={styles.deviceRow}>
              <select
                className={styles.select}
                value={targetDeviceId}
                onChange={(event) =>
                  handleTargetDeviceChange(event.target.value)
                }
              >
                {devices.map((device) => (
                  <option key={device.device_id} value={device.device_id}>
                    {deviceLabel(device, device.device_id)} ·{" "}
                    {deviceConnectivityLabel(t, device.connectivity_state)}
                  </option>
                ))}
              </select>
              <Btn
                icon={<RefreshCw size={16} />}
                onClick={() =>
                  loadDirectory("target", targetDeviceId, targetPath)
                }
              />
            </div>
            <DirectoryPicker
              value={targetPath}
              onChange={setTargetPath}
              browse={browseTarget}
              disabled={!targetDeviceId}
              placeholder="~"
              openLabel={t("airdropTransfer.browseTarget", "浏览目标目录")}
              closeLabel={t("airdropTransfer.closeBrowser", "关闭目录浏览")}
              emptyLabel={t("airdropTransfer.noDirectories", "没有子目录")}
              onListingChange={(listing) => {
                if (!listing.data) return;
                setTargetPath(listing.path);
                setTargetEntries(listing.data.entries);
              }}
              onError={(error) =>
                toast.error(
                  error instanceof Error ? error.message : String(error),
                )
              }
            />
            <div className={styles.list}>
              {loadingTarget ? <Loading /> : targetEntries.map(renderTargetRow)}
            </div>
          </section>
        </div>

        <section className={styles.queue}>
          <div className={styles.queueHeader}>{t("airdropTransfer.queue")}</div>
          {jobs.map((job) => (
            <div key={job.job_id} className={styles.jobRow}>
              <div>
                <div className={styles.name}>
                  {deviceLabel(
                    devices.find(
                      (device) => device.device_id === job.source_device_id,
                    ),
                    job.source_device_id,
                  )}
                  {" → "}
                  {deviceLabel(
                    devices.find(
                      (device) => device.device_id === job.target_device_id,
                    ),
                    job.target_device_id,
                  )}
                </div>
                <div
                  className={
                    job.status === "failed" ? styles.error : styles.meta
                  }
                >
                  {job.status} · {formatBytes(job.bytes_written)} /{" "}
                  {formatBytes(job.total_bytes)}
                  {job.error ? ` · ${job.error}` : ""}
                </div>
              </div>
              {job.status === "running" ? (
                <Btn
                  onClick={() =>
                    syncTransferApi.cancelJob(job.job_id).then(loadJobs)
                  }
                >
                  {t("common.cancel")}
                </Btn>
              ) : null}
            </div>
          ))}
        </section>
      </PageBody>
    </Page>
  );
}
