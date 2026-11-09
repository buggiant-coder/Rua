import { useState, useEffect, useCallback, useRef } from "react";
import type { ColumnDef } from "@/ui/Table";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";

import {
  syncApi,
  devicesApi,
  pairingApi,
  conflictsApi,
  conflictPolicyApi,
  type SyncConfigResponse,
  type SyncErrorReason,
  type SyncPowerPolicyConfig,
  type SyncPowerPolicyMode,
  type SyncPowerPolicyResponse,
  type SyncDiagnosticsResponse,
  type PeerHealthDiagnosticEntry,
  type DeviceSessionPath,
  type SyncRouteKind,
  type DeviceDto,
  type ConflictEntry,
  type ConflictPolicy,
} from "../../../api/modules/sync";
import { notifyMultiDeviceFeaturesChanged } from "../../../hooks/useMultiDeviceFeatureAvailability";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { EmptyState } from "@/components/EmptyState";
import { DeviceConnectivityTag } from "@/components/DeviceConnectivity";
import { LogIn, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import {
  Btn,
  Loading,
  Modal,
  NumberInput,
  Radio,
  RadioGroup,
  Table,
  Tag,
  TextInput,
  Tooltip,
  Toggle,
  toast,
} from "@/ui";

const SYNC_ENABLED_CACHE_KEY = "rua.deviceSync.enabled";

function readCachedSyncEnabled(): boolean | null {
  try {
    const value = window.localStorage.getItem(SYNC_ENABLED_CACHE_KEY);
    if (value === "true") return true;
    if (value === "false") return false;
  } catch {}
  return null;
}

function writeCachedSyncEnabled(enabled: boolean) {
  try {
    window.localStorage.setItem(SYNC_ENABLED_CACHE_KEY, String(enabled));
  } catch {}
}

function formatBytes(bytes?: number): string {
  const value = bytes ?? 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024)
    return `${(value / 1024).toFixed(value < 10 * 1024 ? 1 : 0)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

function tr(
  t: TFunction,
  key: string,
  defaultValue: string,
  values: Record<string, string | number> = {},
): string {
  let translated = String(t(key, { defaultValue, ...values }));
  if (translated === key) {
    translated = defaultValue;
  }
  for (const [name, value] of Object.entries(values)) {
    translated = translated.split(`{{${name}}}`).join(String(value));
  }
  return translated;
}

function formatAgo(ms: number | undefined, t: TFunction): string {
  if (!ms) return "-";
  const diff = Math.max(0, Date.now() - ms);
  if (diff < 60_000) {
    return tr(t, "deviceSync.agoSeconds", "{{count}}s ago", {
      count: Math.max(1, Math.round(diff / 1000)),
    });
  }
  if (diff < 3_600_000) {
    return tr(t, "deviceSync.agoMinutes", "{{count}}m ago", {
      count: Math.round(diff / 60_000),
    });
  }
  if (diff < 86_400_000) {
    return tr(t, "deviceSync.agoHours", "{{count}}h ago", {
      count: Math.round(diff / 3_600_000),
    });
  }
  return new Date(ms).toLocaleString();
}

function routeLabel(
  route: SyncRouteKind | DeviceSessionPath | undefined,
  t: TFunction,
): string {
  switch (route) {
    case "direct":
      return t("deviceSync.routeDirect", "Direct");
    case "relay":
      return t("deviceSync.routeRelay", "Relay");
    case "mixed":
      return t("deviceSync.routeMixed", "Mixed");
    case "other":
    case "unknown":
    default:
      return t("deviceSync.routeNotMeasured", "Not measured");
  }
}

function routeTagColor(
  route?: SyncRouteKind | DeviceSessionPath,
): "success" | "warning" | "default" {
  if (route === "direct") return "success";
  if (route === "relay" || route === "mixed") return "warning";
  return "default";
}

function totalDirectBytes(
  diagnostics?: SyncDiagnosticsResponse | null,
): number {
  const rolling = diagnostics?.transport.rolling;
  return (rolling?.direct_tx_bytes ?? 0) + (rolling?.direct_rx_bytes ?? 0);
}

function totalRelayBytes(diagnostics?: SyncDiagnosticsResponse | null): number {
  const rolling = diagnostics?.transport.rolling;
  return (rolling?.relay_tx_bytes ?? 0) + (rolling?.relay_rx_bytes ?? 0);
}

function totalInboundEntries(
  diagnostics?: SyncDiagnosticsResponse | null,
): number {
  const counts = diagnostics?.inbox.counts;
  return (
    (counts?.pending ?? 0) +
    (counts?.retryable ?? 0) +
    (counts?.dormant ?? 0) +
    (counts?.applied ?? 0) +
    (counts?.failed ?? 0)
  );
}

function ProgressPanel({
  diagnostics,
  enabled,
}: {
  diagnostics: SyncDiagnosticsResponse | null;
  enabled?: boolean | null;
}) {
  const { t } = useTranslation();
  if (!diagnostics && enabled === false) {
    return (
      <div
        style={{
          border: "1px solid var(--border)",
          borderRadius: "var(--radius-md)",
          padding: "var(--space-3)",
          background: "var(--surface)",
          display: "flex",
          flexDirection: "column",
          gap: 8,
        }}
      >
        <Tag>{t("deviceSync.progressDisabled", "Disabled")}</Tag>
        <div style={{ fontWeight: 700, fontSize: 16 }}>
          {t("deviceSync.progressDisabledTitle", "多设备同步未启用")}
        </div>
        <div style={{ color: "var(--text-2)", fontSize: 13 }}>
          {t(
            "deviceSync.progressDisabledCopy",
            "启用同步后才会开始连接设备、传输和应用变更。",
          )}
        </div>
      </div>
    );
  }
  if (!diagnostics) {
    return (
      <div
        style={{
          border: "1px solid var(--border)",
          borderRadius: "var(--radius-md)",
          padding: "var(--space-3)",
          background: "var(--surface)",
        }}
      >
        <Tag>{t("deviceSync.progressLoading", "Loading sync progress")}</Tag>
      </div>
    );
  }

  const progress = diagnostics.progress;
  const lastCycle = diagnostics.transport.last_cycle;
  const rolling = diagnostics.transport.rolling;
  const direct = totalDirectBytes(diagnostics);
  const relay = totalRelayBytes(diagnostics);
  const total = direct + relay;
  const directRatio = total > 0 ? Math.round((direct / total) * 100) : 0;
  const totalReceived = totalInboundEntries(diagnostics);
  const totalSent = rolling.events_sent ?? 0;
  const applyPending = progress.pending_inbound + progress.retryable_inbound;
  const waitingContent = progress.dormant_inbound;
  const relayStatus =
    diagnostics.peers.total_peers > 0
      ? t("deviceSync.relayReady", "ready")
      : t("deviceSync.relayNoPeers", "no peers");
  const routeTiming = lastCycle?.direct_rtt_ms
    ? tr(t, "deviceSync.directRtt", "{{ms}}ms direct", {
        ms: lastCycle.direct_rtt_ms,
      })
    : lastCycle?.relay_rtt_ms
    ? tr(t, "deviceSync.relayRtt", "{{ms}}ms relay", {
        ms: lastCycle.relay_rtt_ms,
      })
    : t("deviceSync.notMeasuredLower", "not measured");
  const stateMeta = (() => {
    switch (progress.state) {
      case "disabled":
        return {
          color: "default" as const,
          label: t("deviceSync.progressDisabled", "Disabled"),
          title: t("deviceSync.progressDisabledTitle", "多设备同步未启用"),
          copy: t(
            "deviceSync.progressDisabledCopy",
            "启用同步后才会开始连接设备、传输和应用变更。",
          ),
        };
      case "syncing":
        return {
          color: "primary" as const,
          label: t("deviceSync.progressSyncing", "Syncing"),
          title: t("deviceSync.progressSyncingTitle", "正在和设备交换数据"),
          copy: t(
            "deviceSync.progressSyncingCopy",
            "Sync is transferring data. Local apply may continue after transfer completes.",
          ),
        };
      case "not_paired":
        return {
          color: "default" as const,
          label: t("deviceSync.progressNotPaired", "Not paired"),
          title: t(
            "deviceSync.progressNotPairedTitle",
            "No paired devices yet",
          ),
          copy: t(
            "deviceSync.progressNotPairedCopy",
            "Add or join a device before multi-device sync has peers to exchange data with.",
          ),
        };
      case "identity_mismatch":
        return {
          color: "danger" as const,
          label: t("deviceSync.progressIdentityMismatch", "Identity mismatch"),
          title: t(
            "deviceSync.progressIdentityMismatchTitle",
            "Sync identity needs repair",
          ),
          copy: t(
            "deviceSync.progressIdentityMismatchCopy",
            "This device's local identity does not match the bound sync network.",
          ),
        };
      case "up_to_date":
        return {
          color: "success" as const,
          label: t("deviceSync.progressUpToDate", "Up to date"),
          title: t("deviceSync.progressUpToDateTitle", "本设备当前已补全"),
          copy: t(
            "deviceSync.progressUpToDateCopy",
            "本设备当前没有待应用、失败或冲突的同步数据；不表示其他设备已完成本地应用。",
          ),
        };
      case "applying_changes":
        return {
          color: "warning" as const,
          label: t("deviceSync.progressApplying", "Applying changes"),
          title: t("deviceSync.progressApplyingTitle", "还有数据正在本机应用"),
          copy: t(
            "deviceSync.progressApplyingCopy",
            "传输可能已经完成，但本地文件写入和索引刷新还没有全部落地。",
          ),
        };
      case "waiting_for_content":
        return {
          color: "warning" as const,
          label: t("deviceSync.progressWaitingContent", "Waiting for content"),
          title: t(
            "deviceSync.progressWaitingContentTitle",
            "有入站数据正在等待内容",
          ),
          copy: t(
            "deviceSync.progressWaitingContentCopy",
            "本机已看到远端变更索引，但对应内容还没有读到；等其他设备在线补齐后会继续应用。",
          ),
        };
      case "waiting_for_device":
        return {
          color: "warning" as const,
          label: t(
            "deviceSync.progressWaitingDevice",
            "Waiting for another device",
          ),
          title: t(
            "deviceSync.progressWaitingDeviceTitle",
            "本设备当前已补全，正在等待其他设备上线",
          ),
          copy: t(
            "deviceSync.progressWaitingDeviceCopy",
            "本机没有待应用、失败或冲突的数据；最近一次传输未能连接到其他设备，设备上线后会自动继续同步。",
          ),
        };
      case "needs_attention":
      default:
        return {
          color: "danger" as const,
          label: t("deviceSync.progressNeedsAttention", "Needs attention"),
          title: t("deviceSync.progressNeedsAttentionTitle", "同步需要处理"),
          copy: t(
            "deviceSync.progressNeedsAttentionCopy",
            "存在失败的入站数据、冲突或最近失败的同步周期。",
          ),
        };
    }
  })();

  return (
    <div
      style={{
        border: "1px solid var(--border)",
        borderRadius: "var(--radius-md)",
        padding: "var(--space-3)",
        background: "var(--surface)",
        display: "flex",
        flexDirection: "column",
        gap: "var(--space-3)",
      }}
    >
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "flex-start",
          gap: "var(--space-3)",
          flexWrap: "wrap",
        }}
      >
        <div>
          <Tag color={stateMeta.color}>{stateMeta.label}</Tag>
          <div style={{ fontWeight: 700, fontSize: 16, marginTop: 8 }}>
            {stateMeta.title}
          </div>
          <div style={{ color: "var(--text-2)", fontSize: 13, marginTop: 4 }}>
            {stateMeta.copy}
          </div>
        </div>
        <div
          style={{ display: "flex", gap: "var(--space-2)", flexWrap: "wrap" }}
        >
          <Tag>
            {t("deviceSync.lastSync", "Last sync")}:{" "}
            {formatAgo(lastCycle?.completed_at_ms, t)}
          </Tag>
          <Tag>
            {t("deviceSync.homeRelay", "Relay")}: {relayStatus}
          </Tag>
        </div>
      </div>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))",
          gap: "var(--space-2)",
        }}
      >
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: "var(--radius-md)",
            padding: "var(--space-2)",
            background: "var(--surface-2)",
          }}
        >
          <div style={{ color: "var(--text-2)", fontSize: 12 }}>
            {t("deviceSync.cumulativeExchange", "Cumulative exchange")}
          </div>
          <div style={{ fontWeight: 700, marginTop: 4 }}>
            {tr(
              t,
              "deviceSync.cumulativeEventCounts",
              "{{received}} received / {{sent}} sent",
              {
                received: totalReceived,
                sent: totalSent,
              },
            )}
          </div>
          <div style={{ color: "var(--text-2)", fontSize: 12, marginTop: 4 }}>
            {tr(
              t,
              "deviceSync.cumulativeCycleCounts",
              "{{cycles}} sync attempts, {{failures}} failed",
              {
                cycles: rolling.cycles,
                failures: rolling.failures,
              },
            )}
          </div>
        </div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: "var(--radius-md)",
            padding: "var(--space-2)",
            background: "var(--surface-2)",
          }}
        >
          <div style={{ color: "var(--text-2)", fontSize: 12 }}>
            {t("deviceSync.applyQueue", "Apply queue")}
          </div>
          <div style={{ fontWeight: 700, marginTop: 4 }}>
            {tr(t, "deviceSync.applyPendingCount", "{{count}} pending", {
              count: applyPending,
            })}
          </div>
          <div style={{ color: "var(--text-2)", fontSize: 12, marginTop: 4 }}>
            {tr(
              t,
              "deviceSync.applyWaitingContentCount",
              "{{count}} waiting for content",
              {
                count: waitingContent,
              },
            )}
          </div>
          <div style={{ color: "var(--text-2)", fontSize: 12, marginTop: 4 }}>
            {tr(
              t,
              "deviceSync.applyFailureConflictCounts",
              "{{failed}} failed, {{conflicts}} conflicts",
              {
                failed: progress.failed_inbound,
                conflicts: progress.pending_conflicts,
              },
            )}
          </div>
        </div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: "var(--radius-md)",
            padding: "var(--space-2)",
            background: "var(--surface-2)",
          }}
        >
          <div style={{ color: "var(--text-2)", fontSize: 12 }}>
            {t("deviceSync.lastRoute", "Last route")}
          </div>
          <div style={{ fontWeight: 700, marginTop: 4 }}>
            {routeLabel(lastCycle?.route, t)}
          </div>
          <div style={{ color: "var(--text-2)", fontSize: 12, marginTop: 4 }}>
            {routeTiming}
          </div>
        </div>
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: "var(--radius-md)",
            padding: "var(--space-2)",
            background: "var(--surface-2)",
          }}
        >
          <div style={{ color: "var(--text-2)", fontSize: 12 }}>
            {t("deviceSync.directRatio", "Direct ratio")}
          </div>
          <div style={{ fontWeight: 700, marginTop: 4 }}>{directRatio}%</div>
          <div style={{ color: "var(--text-2)", fontSize: 12, marginTop: 4 }}>
            {tr(
              t,
              "deviceSync.directTotalBytes",
              "{{direct}} direct / {{total}} total",
              {
                direct: formatBytes(direct),
                total: formatBytes(total),
              },
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function SyncStatusSection({
  diagnostics,
  onConfigChange,
  onDiagnosticsChange,
  refreshKey,
}: {
  diagnostics: SyncDiagnosticsResponse | null;
  onConfigChange?: (enabled: boolean) => void;
  onDiagnosticsChange?: (diagnostics: SyncDiagnosticsResponse | null) => void;
  refreshKey?: number;
}) {
  const { t } = useTranslation();
  const [configEnabled, setConfigEnabled] = useState<boolean | null>(() =>
    readCachedSyncEnabled(),
  );
  const [configLoading, setConfigLoading] = useState(configEnabled === null);
  const [toggling, setToggling] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [retryingFailed, setRetryingFailed] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);

  const applyConfig = useCallback(
    (config: SyncConfigResponse) => {
      setConfigEnabled(config.enabled);
      writeCachedSyncEnabled(config.enabled);
      onConfigChange?.(config.enabled);
    },
    [onConfigChange],
  );

  const fetchConfig = useCallback(async () => {
    if (configEnabled === null) {
      setConfigLoading(true);
    }
    try {
      applyConfig(await syncApi.getConfig());
    } catch {
      if (configEnabled === null) {
        onConfigChange?.(false);
      }
    } finally {
      setConfigLoading(false);
    }
  }, [applyConfig, configEnabled, onConfigChange]);

  const fetchStatus = useCallback(async () => {
    try {
      await syncApi.getStatus();
    } catch {}
  }, []);

  const fetchDiagnostics = useCallback(async () => {
    if (!configEnabled) {
      onDiagnosticsChange?.(null);
      return;
    }
    try {
      onDiagnosticsChange?.(await syncApi.getDiagnostics());
    } catch {
      onDiagnosticsChange?.(null);
    }
  }, [configEnabled, onDiagnosticsChange]);

  useEffect(() => {
    if (configEnabled !== null) {
      onConfigChange?.(configEnabled);
    }
  }, [configEnabled, onConfigChange]);

  useEffect(() => {
    fetchConfig();
    fetchStatus();
    fetchDiagnostics();
  }, [fetchConfig, fetchStatus, fetchDiagnostics, refreshKey]);

  useEffect(() => {
    if (!configEnabled) return;
    const interval = window.setInterval(fetchDiagnostics, 5000);
    return () => window.clearInterval(interval);
  }, [configEnabled, fetchDiagnostics]);

  const reasonLabels: Record<SyncErrorReason, string> = {
    not_paired: t("sync.notPaired", "未配对设备"),
    no_eligible_peer: t("sync.noEligiblePeer", "当前没有可连接的设备"),
    peer_unreachable: t("sync.peerUnreachable", "对端设备不可达"),
    signature_failed: t("sync.signatureFailed", "签名验证失败"),
    decrypt_failed: t("sync.decryptFailed", "解密失败"),
    timeout: t("sync.timeout", "同步超时"),
    repair_scan_failed: t("sync.repairScanFailed", "本机完整检查失败"),
    inbox_drain_failed: t("sync.inboxDrainFailed", "本机应用同步数据失败"),
  };

  const handleSyncNow = async () => {
    setSyncing(true);
    setSyncError(null);
    try {
      await syncApi.triggerSyncCycle();
      await fetchDiagnostics();
    } catch (err: unknown) {
      const reason =
        (err as { reason?: SyncErrorReason }).reason || "peer_unreachable";
      setSyncError(reasonLabels[reason] || reason);
      await fetchDiagnostics();
    } finally {
      setSyncing(false);
    }
  };

  const handleRetryFailed = async () => {
    setRetryingFailed(true);
    try {
      const { retried } = await syncApi.retryFailedInbox();
      await fetchDiagnostics();
      toast.success(
        tr(
          t,
          "deviceSync.retryFailedSuccess",
          "Retrying {{count}} failed items",
          {
            count: retried,
          },
        ),
      );
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : "Failed to retry sync items",
      );
    } finally {
      setRetryingFailed(false);
    }
  };

  const handleToggle = async (checked: boolean) => {
    const previous = configEnabled ?? false;
    if (!checked) {
      Modal.confirm({
        title: t("sync.disableTitle", "Disable Sync"),
        content: t(
          "sync.disableConfirm",
          "Disabling sync stops synchronization with other devices. Your local data will be preserved.",
        ),
        okText: t("common.confirm", "Confirm"),
        danger: true,
        cancelText: t("common.cancel", "Cancel"),
        onOk: async () => {
          setToggling(true);
          try {
            await syncApi.disable();
            applyConfig({ enabled: false });
            notifyMultiDeviceFeaturesChanged();
            toast.success(t("sync.disabled", "Sync disabled"));
            await fetchStatus();
          } catch (err) {
            applyConfig({ enabled: previous });
            toast.error(
              err instanceof Error ? err.message : "Failed to disable sync",
            );
          } finally {
            setToggling(false);
          }
        },
      });
      return;
    }

    Modal.confirm({
      title: t("sync.enableTitle", "Enable Sync"),
      content: t(
        "sync.firewallHint",
        "Enabling multi-device sync requires network access. Your system may prompt you to allow incoming connections (firewall permission). Please click Allow to ensure devices can connect directly.",
      ),
      okText: t("common.confirm", "Confirm"),
      cancelText: t("common.cancel", "Cancel"),
      onOk: async () => {
        setToggling(true);
        try {
          await syncApi.enable();
          applyConfig({ enabled: true });
          notifyMultiDeviceFeaturesChanged();
          toast.success(t("sync.enabled", "Sync enabled"));
          await fetchStatus();
        } catch (err) {
          applyConfig({ enabled: previous });
          toast.error(
            err instanceof Error ? err.message : "Failed to enable sync",
          );
        } finally {
          setToggling(false);
        }
      },
    });
  };

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "var(--space-3)",
        width: "100%",
      }}
    >
      <div
        style={{ display: "flex", alignItems: "center", gap: "var(--space-3)" }}
      >
        {configLoading && configEnabled === null ? (
          <Loading size="sm" />
        ) : (
          <Toggle
            role="switch"
            aria-label={t("sync.multiDeviceSync", "Multi-device Sync")}
            checked={configEnabled ?? false}
            onChange={(e) => handleToggle(e.target.checked)}
            disabled={toggling || configLoading}
          />
        )}
        <span style={{ fontWeight: 600 }}>
          {t("sync.multiDeviceSync", "Multi-device Sync")}
        </span>
      </div>

      {configEnabled && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "var(--space-2)",
            flexWrap: "wrap",
          }}
        >
          <Btn
            icon={
              <RefreshCw size={14} className={syncing ? "icon-spin" : ""} />
            }
            loading={syncing}
            onClick={handleSyncNow}
            disabled={syncing}
          >
            {t("sync.syncNow", "Sync now")}
          </Btn>
          {(diagnostics?.inbox.counts.failed ?? 0) > 0 && (
            <Btn
              icon={
                <RefreshCw
                  size={14}
                  className={retryingFailed ? "icon-spin" : ""}
                />
              }
              loading={retryingFailed}
              onClick={handleRetryFailed}
              disabled={retryingFailed || syncing}
            >
              {tr(
                t,
                "deviceSync.retryFailedItems",
                "Retry {{count}} failed items",
                { count: diagnostics?.inbox.counts.failed ?? 0 },
              )}
            </Btn>
          )}
          {syncError && (
            <span
              style={{
                color: "var(--danger-fg)",
                fontSize: 13,
              }}
            >
              {syncError}
            </span>
          )}
        </div>
      )}
      <ProgressPanel diagnostics={diagnostics} enabled={configEnabled} />
    </div>
  );
}

type PowerPolicyForm = Required<SyncPowerPolicyConfig>;

const DEFAULT_CUSTOM_POLICY: PowerPolicyForm = {
  mode: "custom",
  periodic_interval_secs: 600,
  background_flush_grace_secs: 60,
  allow_background_runtime: false,
  allow_background_scheduled_sync: false,
};

function policyResponseToForm(
  response: SyncPowerPolicyResponse,
): PowerPolicyForm {
  const source = response.stored_policy ?? response.effective_policy;
  return {
    mode: source.mode,
    periodic_interval_secs:
      source.periodic_interval_secs ??
      response.effective_policy.periodic_interval_secs,
    background_flush_grace_secs:
      source.background_flush_grace_secs ??
      response.effective_policy.background_flush_grace_secs,
    allow_background_runtime:
      source.allow_background_runtime ??
      response.effective_policy.allow_background_runtime,
    allow_background_scheduled_sync:
      source.allow_background_scheduled_sync ??
      response.effective_policy.allow_background_scheduled_sync,
  };
}

function PowerPolicySection() {
  const { t } = useTranslation();
  const [policy, setPolicy] = useState<SyncPowerPolicyResponse | null>(null);
  const [form, setForm] = useState<PowerPolicyForm>(DEFAULT_CUSTOM_POLICY);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  const loadPolicy = useCallback(async () => {
    setLoading(true);
    try {
      const response = await syncApi.getPowerPolicy();
      setPolicy(response);
      setForm(policyResponseToForm(response));
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : "Failed to load sync power policy",
      );
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadPolicy();
  }, [loadPolicy]);

  const modeLabels: Record<SyncPowerPolicyMode, string> = {
    power_saver: t("deviceSync.powerPolicyPowerSaver", "Power Saver"),
    balanced: t("deviceSync.powerPolicyBalanced", "Balanced"),
    realtime: t("deviceSync.powerPolicyRealtime", "Realtime"),
    custom: t("deviceSync.powerPolicyCustom", "Custom"),
  };

  const modeDescriptions: Record<SyncPowerPolicyMode, string> = {
    power_saver: t(
      "deviceSync.powerPolicyPowerSaverDesc",
      "lower battery use; remote changes may arrive only after reopening the app",
    ),
    balanced: t(
      "deviceSync.powerPolicyBalancedDesc",
      "system-scheduled background sync, not realtime",
    ),
    realtime: t(
      "deviceSync.powerPolicyRealtimeDesc",
      "may increase battery use and heat, and Android background limits can still apply",
    ),
    custom: t(
      "deviceSync.powerPolicyCustomDesc",
      "advanced scheduler controls for this device",
    ),
  };

  const handleModeChange = (mode: SyncPowerPolicyMode) => {
    setForm((current) => ({
      ...current,
      mode,
    }));
  };

  const buildRequest = (): SyncPowerPolicyConfig => {
    if (form.mode !== "custom") {
      return { mode: form.mode };
    }
    return {
      mode: "custom",
      periodic_interval_secs: form.periodic_interval_secs,
      background_flush_grace_secs: form.background_flush_grace_secs,
      allow_background_runtime: form.allow_background_runtime,
      allow_background_scheduled_sync: form.allow_background_scheduled_sync,
    };
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      const response = await syncApi.updatePowerPolicy(buildRequest());
      setPolicy(response);
      setForm(policyResponseToForm(response));
      toast.success(t("deviceSync.powerPolicySaved", "Power policy saved"));
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : "Failed to save sync power policy",
      );
    } finally {
      setSaving(false);
    }
  };

  const setNumberField =
    (field: "periodic_interval_secs" | "background_flush_grace_secs") =>
    (value: number | null) => {
      const min = field === "periodic_interval_secs" ? 1 : 0;
      setForm((current) => ({
        ...current,
        [field]: Math.max(min, value ?? min),
      }));
    };

  const effective = policy?.effective_policy;

  return (
    <>
      <h3 style={{ margin: "0 0 4px" }}>
        {t("deviceSync.powerPolicyTitle", "Sync Power Policy")}
      </h3>
      <span
        style={{ color: "var(--text-2)", display: "block", marginBottom: 12 }}
      >
        {t(
          "deviceSync.powerPolicyHint",
          "Controls how this device balances sync freshness with battery and background limits.",
        )}
      </span>
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: "var(--space-3)",
        }}
      >
        <div
          style={{
            display: "flex",
            flexWrap: "wrap",
            gap: "var(--space-2)",
            alignItems: "center",
          }}
        >
          <Tag>
            {t("deviceSync.hostProfile", "Host profile")}:{" "}
            {policy?.host_profile ?? "-"}
          </Tag>
          <Tag>
            {t("deviceSync.currentMode", "Current mode")}:{" "}
            {effective ? modeLabels[effective.mode] : "-"}
          </Tag>
          {loading && <Tag>{t("common.loading", "Loading")}</Tag>}
        </div>

        <RadioGroup
          value={form.mode}
          onChange={(value) => handleModeChange(value as SyncPowerPolicyMode)}
          disabled={saving || loading}
        >
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
              gap: "var(--space-2)",
            }}
          >
            {(
              [
                "power_saver",
                "balanced",
                "realtime",
                "custom",
              ] as SyncPowerPolicyMode[]
            ).map((mode) => (
              <Radio key={mode} value={mode} aria-label={modeLabels[mode]}>
                <span style={{ fontWeight: 600 }}>{modeLabels[mode]}</span>
                <br />
                <span style={{ color: "var(--text-2)", fontSize: 12 }}>
                  {modeDescriptions[mode]}
                </span>
              </Radio>
            ))}
          </div>
        </RadioGroup>

        {form.mode === "custom" && (
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
              gap: "var(--space-3)",
            }}
          >
            <label style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              <span>
                {t(
                  "deviceSync.periodicIntervalSecs",
                  "Periodic interval seconds",
                )}
              </span>
              <NumberInput
                min={1}
                value={form.periodic_interval_secs}
                onChange={setNumberField("periodic_interval_secs")}
                aria-label={t(
                  "deviceSync.periodicIntervalSecs",
                  "Periodic interval seconds",
                )}
              />
            </label>
            <label style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              <span>
                {t(
                  "deviceSync.backgroundFlushGraceSecs",
                  "Background flush grace seconds",
                )}
              </span>
              <NumberInput
                min={0}
                value={form.background_flush_grace_secs}
                onChange={setNumberField("background_flush_grace_secs")}
                aria-label={t(
                  "deviceSync.backgroundFlushGraceSecs",
                  "Background flush grace seconds",
                )}
              />
            </label>
            <label
              style={{
                display: "flex",
                alignItems: "center",
                gap: "var(--space-2)",
              }}
            >
              <Toggle
                checked={form.allow_background_runtime}
                onChange={(e) =>
                  setForm((current) => ({
                    ...current,
                    allow_background_runtime: e.target.checked,
                  }))
                }
                aria-label={t(
                  "deviceSync.allowBackgroundRuntime",
                  "Allow background runtime",
                )}
              />
              <span>
                {t(
                  "deviceSync.allowBackgroundRuntime",
                  "Allow background runtime",
                )}
              </span>
            </label>
            <label
              style={{
                display: "flex",
                alignItems: "center",
                gap: "var(--space-2)",
              }}
            >
              <Toggle
                checked={form.allow_background_scheduled_sync}
                onChange={(e) =>
                  setForm((current) => ({
                    ...current,
                    allow_background_scheduled_sync: e.target.checked,
                  }))
                }
                aria-label={t(
                  "deviceSync.allowScheduledBackgroundSync",
                  "Allow scheduled background sync",
                )}
              />
              <span>
                {t(
                  "deviceSync.allowScheduledBackgroundSync",
                  "Allow scheduled background sync",
                )}
              </span>
            </label>
          </div>
        )}

        {effective && (
          <div style={{ color: "var(--text-2)", fontSize: 13 }}>
            <strong style={{ color: "var(--text-1)" }}>
              {t("deviceSync.effectivePolicy", "Effective policy")}
            </strong>
            {" · "}
            {t("deviceSync.effectiveInterval", "Interval: {{seconds}}s", {
              seconds: effective.periodic_interval_secs,
            })}
            {" · "}
            {t("deviceSync.effectiveFlushGrace", "Flush grace: {{seconds}}s", {
              seconds: effective.background_flush_grace_secs,
            })}
            {" · "}
            {t(
              effective.allow_background_runtime
                ? "deviceSync.backgroundRuntimeOn"
                : "deviceSync.backgroundRuntimeOff",
              effective.allow_background_runtime
                ? "Background runtime on"
                : "Background runtime off",
            )}
            {" · "}
            {t(
              effective.allow_background_scheduled_sync
                ? "deviceSync.scheduledSyncOn"
                : "deviceSync.scheduledSyncOff",
              effective.allow_background_scheduled_sync
                ? "Scheduled sync on"
                : "Scheduled sync off",
            )}
          </div>
        )}

        <div>
          <Btn
            onClick={handleSave}
            loading={saving}
            disabled={loading || saving}
          >
            {t("deviceSync.savePowerPolicy", "Save policy")}
          </Btn>
        </div>
      </div>
    </>
  );
}

function DevicesSection({
  diagnostics,
  onDeviceChange,
}: {
  diagnostics?: SyncDiagnosticsResponse | null;
  onDeviceChange?: () => void;
}) {
  const { t } = useTranslation();
  const [devices, setDevices] = useState<DeviceDto[]>([]);
  const [loading, setLoading] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [addModalOpen, setAddModalOpen] = useState(false);
  const [joinModalOpen, setJoinModalOpen] = useState(false);

  const fetchDevices = useCallback(async (showLoading = true) => {
    if (showLoading) setLoading(true);
    try {
      const list = await devicesApi.listDevices();
      setDevices(list);
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : "Failed to load devices",
      );
    } finally {
      if (showLoading) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void fetchDevices();
    return devicesApi.watchChanges?.(() => void fetchDevices(false));
  }, [fetchDevices]);

  const handleRemove = (device: DeviceDto) => {
    Modal.confirm({
      title: t("devices.removeTitle", "Remove Device"),
      content: t(
        "devices.removeConfirm",
        "After removal, this device will no longer receive new sync data. Local data will be preserved.",
      ),
      okText: t("common.delete", "Remove"),
      danger: true,
      cancelText: t("common.cancel", "Cancel"),
      onOk: async () => {
        try {
          await devicesApi.removeDevice(device.device_id);
          notifyMultiDeviceFeaturesChanged();
          toast.success(t("devices.removeSuccess", "Device removed"));
          fetchDevices();
          onDeviceChange?.();
        } catch (err) {
          toast.error(
            err instanceof Error ? err.message : "Failed to remove device",
          );
        }
      },
    });
  };

  const handleStartEdit = (device: DeviceDto) => {
    setEditingId(device.device_id);
    setEditName(device.friendly_name);
  };

  const handleSaveEdit = async (deviceId: string) => {
    if (!editName.trim()) return;
    try {
      await devicesApi.renameDevice(deviceId, editName.trim());
      toast.success(t("devices.renameSuccess", "Device renamed"));
      setEditingId(null);
      fetchDevices();
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : "Failed to rename device",
      );
    }
  };

  const handleCancelEdit = () => {
    setEditingId(null);
    setEditName("");
  };

  const findPeerDiagnostics = (
    device: DeviceDto,
  ): PeerHealthDiagnosticEntry | undefined =>
    diagnostics?.peers.peers.find(
      (peer) =>
        peer.node_id === device.public_key ||
        peer.device_id_hex === device.device_id ||
        peer.device_id_hex === device.device_id.replace(/-/g, ""),
    );

  return (
    <>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          gap: "var(--space-2)",
          marginBottom: 12,
          flexWrap: "wrap",
        }}
      >
        <h3 style={{ margin: 0 }}>{t("devices.title", "Devices")}</h3>
        <div style={{ display: "flex", gap: "var(--space-2)" }}>
          <Tooltip title={t("devices.add_device", "Add Device")}>
            <Btn
              icon={<Plus />}
              aria-label={t("devices.add_device", "Add Device")}
              onClick={() => setAddModalOpen(true)}
            />
          </Tooltip>
          <Tooltip title={t("devices.join_device", "Join Device")}>
            <Btn
              icon={<LogIn />}
              aria-label={t("devices.join_device", "Join Device")}
              onClick={() => setJoinModalOpen(true)}
            />
          </Tooltip>
          <Tooltip title={t("common.refresh", "Refresh")}>
            <Btn
              icon={<RefreshCw />}
              aria-label={t("common.refresh", "Refresh")}
              onClick={() => void fetchDevices()}
              loading={loading}
            />
          </Tooltip>
        </div>
      </div>

      {devices.length === 0 ? (
        <EmptyState description={t("devices.empty", "No paired devices")} />
      ) : (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "var(--space-2)",
            opacity: loading ? 0.7 : 1,
          }}
        >
          {devices.map((device) => {
            const peer = findPeerDiagnostics(device);
            const route = device.session_path ?? undefined;
            const directBytes = peer?.last_direct_bytes ?? 0;
            const relayBytes = peer?.last_relay_bytes ?? 0;
            const progressText = peer?.last_success_ms
              ? tr(t, "devices.lastSyncedAgo", "Last synced {{time}}", {
                  time: formatAgo(peer.last_success_ms, t),
                })
              : t("devices.neverSynced", "Never synced");
            return (
              <div
                key={device.device_id}
                style={{
                  display: "grid",
                  gridTemplateColumns: "repeat(auto-fit, minmax(170px, 1fr))",
                  gap: "var(--space-3)",
                  alignItems: "center",
                  border: "1px solid var(--border)",
                  borderRadius: "var(--radius-md)",
                  padding: "var(--space-3)",
                  background: "var(--surface)",
                }}
              >
                <div style={{ minWidth: 0 }}>
                  {editingId === device.device_id ? (
                    <div
                      style={{
                        display: "flex",
                        gap: "var(--space-2)",
                        flexWrap: "wrap",
                      }}
                    >
                      <TextInput
                        value={editName}
                        onChange={(e) => setEditName(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter")
                            handleSaveEdit(device.device_id);
                        }}
                        autoFocus
                      />
                      <Btn
                        size="sm"
                        onClick={() => handleSaveEdit(device.device_id)}
                      >
                        {t("common.save", "Save")}
                      </Btn>
                      <Btn size="sm" onClick={handleCancelEdit}>
                        {t("common.cancel", "Cancel")}
                      </Btn>
                    </div>
                  ) : (
                    <>
                      <div style={{ fontWeight: 700 }}>
                        {device.friendly_name || device.device_id.slice(0, 8)}
                        {device.is_self && (
                          <Tag style={{ marginLeft: 8 }}>
                            {t("devices.this_device", "This Device")}
                          </Tag>
                        )}
                        {!device.is_self && (
                          <span style={{ marginLeft: 8, fontWeight: 400 }}>
                            <DeviceConnectivityTag
                              state={device.connectivity_state}
                            />
                          </span>
                        )}
                      </div>
                      <div
                        style={{
                          color: "var(--text-2)",
                          fontSize: 12,
                          marginTop: 3,
                        }}
                      >
                        {device.label || "-"} ·{" "}
                        {device.public_key
                          ? device.public_key.slice(0, 12)
                          : device.device_id}
                      </div>
                    </>
                  )}
                </div>
                <div>
                  <Tag color={routeTagColor(route)}>{routeLabel(route, t)}</Tag>
                  <div
                    style={{
                      color: "var(--text-2)",
                      fontSize: 12,
                      marginTop: 5,
                    }}
                  >
                    {device.session_rtt_ms != null
                      ? tr(t, "devices.rttMs", "{{ms}} ms RTT", {
                          ms: device.session_rtt_ms,
                        })
                      : route
                      ? t("devices.routeMeasuredNoRtt", "route measured")
                      : t(
                          "devices.routeNotMeasured",
                          "actual path not measured yet",
                        )}
                  </div>
                </div>
                <div>
                  <div style={{ fontWeight: 600 }}>
                    {peer?.consecutive_failures
                      ? t("devices.syncFailing", "Failing")
                      : progressText}
                  </div>
                  <div
                    style={{
                      color: "var(--text-2)",
                      fontSize: 12,
                      marginTop: 4,
                    }}
                  >
                    {tr(
                      t,
                      "devices.routeBytes",
                      "{{direct}} direct, {{relay}} relay",
                      {
                        direct: formatBytes(directBytes),
                        relay: formatBytes(relayBytes),
                      },
                    )}
                  </div>
                </div>
                <div
                  style={{
                    display: "flex",
                    gap: "var(--space-2)",
                    flexWrap: "wrap",
                    justifyContent: "flex-end",
                  }}
                >
                  <Btn
                    size="sm"
                    data-testid={`edit-device-${device.device_id}`}
                    aria-label={t("devices.renameDevice", "Rename device")}
                    icon={<Pencil />}
                    onClick={() => handleStartEdit(device)}
                  />
                  {!device.is_self && (
                    <Btn
                      size="sm"
                      variant="danger"
                      data-testid={`remove-device-${device.device_id}`}
                      aria-label={t("devices.removeDevice", "Remove device")}
                      icon={<Trash2 />}
                      onClick={() => handleRemove(device)}
                    />
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}

      <AddDeviceModal
        open={addModalOpen}
        onClose={() => {
          setAddModalOpen(false);
          fetchDevices();
        }}
      />
      <JoinDeviceModal
        open={joinModalOpen}
        onClose={() => {
          setJoinModalOpen(false);
          fetchDevices();
        }}
      />
    </>
  );
}

function AddDeviceModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [pin, setPin] = useState<string | null>(null);
  const [remainingSecs, setRemainingSecs] = useState(0);
  const [loading, setLoading] = useState(false);
  const [paired, setPaired] = useState(false);
  const [finalizing, setFinalizing] = useState(false);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const cleanup = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }, []);

  const startPairing = useCallback(async () => {
    setLoading(true);
    setPaired(false);
    setFinalizing(false);
    setPin(null);
    try {
      const res = await pairingApi.initiate();
      setPin(res.pin);
      setRemainingSecs(res.expires_in_secs);
    } catch (err: unknown) {
      const e = err as { message?: string };
      toast.error(e.message || "Failed to start pairing");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (open) {
      startPairing();
    } else {
      cleanup();
      setPin(null);
      setPaired(false);
      setFinalizing(false);
      setRemainingSecs(0);
    }
    return cleanup;
  }, [open, startPairing, cleanup]);

  useEffect(() => {
    if (!pin || remainingSecs <= 0) return;

    timerRef.current = setInterval(() => {
      setRemainingSecs((prev) => {
        if (prev <= 1) {
          if (timerRef.current) clearInterval(timerRef.current);
          return 0;
        }
        return prev - 1;
      });
    }, 1000);

    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, [pin]);

  useEffect(() => {
    if (!pin) return;

    pollRef.current = setInterval(async () => {
      try {
        const status = await pairingApi.status();
        if (status.status === "paired") {
          cleanup();
          setPaired(true);
          setFinalizing(false);
          notifyMultiDeviceFeaturesChanged();
          toast.success(
            t("devices.paired_success", "New device added successfully"),
          );
          setTimeout(() => onClose(), 1500);
        } else if (status.status === "consumed") {
          setFinalizing(true);
          if (timerRef.current) {
            clearInterval(timerRef.current);
            timerRef.current = null;
          }
          setRemainingSecs(0);
        } else if (status.status === "failed" || status.status === "expired") {
          cleanup();
          setFinalizing(false);
          toast.error(
            status.status === "expired"
              ? t("devices.pairing_expired", "PIN expired. Please try again.")
              : t(
                  "devices.pairing_failed",
                  "Pairing failed. Please try again.",
                ),
          );
          onClose();
        }
      } catch {}
    }, 2000);

    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, [pin, cleanup, t, onClose]);

  const handleCancel = async () => {
    try {
      await pairingApi.cancel();
    } catch {}
    cleanup();
    onClose();
  };

  const formatTime = (secs: number) => {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  return (
    <Modal
      open={open}
      onClose={handleCancel}
      footer={null}
      title={t("devices.add_device", "Add Device")}
      destroyOnClose
      width={420}
    >
      <div style={{ textAlign: "center", padding: "24px 0" }}>
        {paired ? (
          <h3 style={{ color: "var(--success-fg)" }}>
            {t("devices.paired_success", "New device added successfully")}
          </h3>
        ) : finalizing ? (
          <>
            <Loading />
            <h3 style={{ marginTop: 16 }}>
              {t(
                "devices.pairing_finalizing",
                "Device connected. Finishing setup...",
              )}
            </h3>
          </>
        ) : pin ? (
          <>
            <p
              style={{
                marginBottom: 16,
                fontSize: 14,
                color: "var(--text-mute)",
              }}
            >
              {t(
                "devices.show_pin_instruction",
                "Enter this PIN on your new device to pair:",
              )}
            </p>
            <div
              style={{
                fontFamily: "var(--font-mono)",
                fontSize: 44,
                fontWeight: "bold",
                letterSpacing: "0.08em",
                margin: "16px auto",
                padding: "12px 16px",
                background: "var(--surface-2)",
                borderRadius: "var(--radius-md)",
                display: "flex",
                justifyContent: "center",
                boxSizing: "border-box",
                width: "100%",
                maxWidth: 360,
                lineHeight: 1.1,
              }}
            >
              {pin}
            </div>
            <div style={{ marginTop: 12, fontSize: 16 }}>
              {remainingSecs > 0 ? (
                <span>
                  {t("devices.expires_in", "Expires in")}{" "}
                  {formatTime(remainingSecs)}
                </span>
              ) : (
                <span style={{ color: "var(--danger-fg)" }}>
                  {t("devices.pin_expired", "PIN expired")}
                </span>
              )}
            </div>
          </>
        ) : (
          <Btn loading={loading} onClick={startPairing}>
            {t("devices.generate_pin", "Generate PIN")}
          </Btn>
        )}
      </div>
    </Modal>
  );
}

type JoinErrorReason = "wrong-pin" | "expired" | "network";

function JoinDeviceModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [pin, setPin] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<{
    message: string;
    reason: JoinErrorReason;
  } | null>(null);
  const [paired, setPaired] = useState(false);

  useEffect(() => {
    if (!open) {
      setPin("");
      setLoading(false);
      setError(null);
      setPaired(false);
    }
  }, [open]);

  const handlePinChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value.replace(/\D/g, "").slice(0, 8);
    setPin(value);
    setError(null);
  };

  const handleConnect = async () => {
    if (pin.length !== 8) {
      setError({ message: "PIN must be 8 digits", reason: "wrong-pin" });
      return;
    }

    setLoading(true);
    setError(null);
    try {
      await pairingApi.join(pin);
      setPaired(true);
      notifyMultiDeviceFeaturesChanged();
      toast.success(
        t("join.success", "Successfully paired with existing device"),
      );
      setTimeout(() => onClose(), 1500);
    } catch (err: unknown) {
      const e = err as { reason?: JoinErrorReason; message?: string };
      const reason: JoinErrorReason = e.reason || "network";
      setError({ message: e.message || "Unknown error", reason });
    } finally {
      setLoading(false);
    }
  };

  const errorMessages: Record<JoinErrorReason, string> = {
    "wrong-pin": t(
      "join.error_wrong_pin",
      "Incorrect PIN. Please check and try again.",
    ),
    expired: t(
      "join.error_expired",
      "This PIN has expired. Ask the other device to generate a new one.",
    ),
    network: t(
      "join.error_network",
      "Connection failed. Check your network and try again.",
    ),
  };

  return (
    <Modal
      open={open}
      onClose={() => onClose()}
      footer={null}
      title={t("join.title", "Join My Devices")}
      destroyOnClose
      width={420}
    >
      <div style={{ textAlign: "center", padding: "24px 0" }}>
        {paired ? (
          <h3 style={{ color: "var(--success-fg)" }}>
            {t("join.success", "Successfully paired with existing device")}
          </h3>
        ) : (
          <>
            <p
              style={{
                marginBottom: 24,
                fontSize: 14,
                color: "var(--text-mute)",
              }}
            >
              {t(
                "join.instruction",
                "Enter the 8-digit PIN shown on your existing device to pair:",
              )}
            </p>
            <TextInput
              value={pin}
              onChange={handlePinChange}
              placeholder="00000000"
              maxLength={8}
              autoFocus
              style={{
                fontSize: 32,
                textAlign: "center",
                letterSpacing: "0.3em",
                fontFamily: "var(--font-mono)",
                height: 56,
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleConnect();
              }}
            />
            {error && (
              <div
                style={{
                  marginTop: 12,
                  color: "var(--danger-fg)",
                  fontSize: 13,
                }}
              >
                {errorMessages[error.reason]}
              </div>
            )}
            <div style={{ marginTop: 24 }}>
              <Btn
                size="lg"
                loading={loading}
                disabled={pin.length !== 8}
                onClick={handleConnect}
                style={{ width: "100%" }}
              >
                {t("join.connect", "Connect")}
              </Btn>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}

function ConflictPolicySection() {
  const { t } = useTranslation();
  const [policy, setPolicy] = useState<ConflictPolicy>("silent");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    conflictPolicyApi
      .get()
      .then((res) => setPolicy(res.policy))
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []);

  const handleChange = async (value: ConflictPolicy) => {
    setSaving(true);
    try {
      const res = await conflictPolicyApi.set(value);
      setPolicy(res.policy);
      toast.success(t("sync.policySaved", "Conflict policy saved"));
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : "Failed to save conflict policy",
      );
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <h3 style={{ margin: "0 0 4px" }}>
        {t("sync.conflictPolicy", "Conflict Policy")}
      </h3>
      <span
        style={{ color: "var(--text-2)", display: "block", marginBottom: 12 }}
      >
        {t(
          "sync.conflictPolicyHint",
          "Choose how conflicts are handled when the same data is modified on multiple devices.",
        )}
      </span>
      <RadioGroup
        value={policy}
        onChange={(value) => handleChange(value as ConflictPolicy)}
        disabled={saving || loading}
      >
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "var(--space-2)",
          }}
        >
          <Radio value="silent">
            <span style={{ fontWeight: 600 }}>
              {t("sync.policySilent", "Silent")}
            </span>
            <br />
            <span style={{ color: "var(--text-2)", fontSize: 12 }}>
              {t(
                "sync.policySilentDesc",
                "Conflicts are resolved automatically. The most recent change wins.",
              )}
            </span>
          </Radio>
          <Radio value="review">
            <span style={{ fontWeight: 600 }}>
              {t("sync.policyReview", "Review")}
            </span>
            <br />
            <span style={{ color: "var(--text-2)", fontSize: 12 }}>
              {t(
                "sync.policyReviewDesc",
                "Conflicts are held for manual review. You choose which version to keep.",
              )}
            </span>
          </Radio>
        </div>
      </RadioGroup>
    </>
  );
}

function ConflictsSection() {
  const { t } = useTranslation();
  const [data, setData] = useState<ConflictEntry[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    conflictsApi
      .getRecent()
      .then((res) => {
        const now = Date.now();
        const dayMs = 24 * 60 * 60 * 1000;
        setData(
          res.conflicts.filter(
            (c: ConflictEntry) => now - c.observed_at < dayMs,
          ),
        );
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []);

  const columns: ColumnDef<ConflictEntry>[] = [
    {
      title: t("deviceSync.opKind", "Op"),
      dataIndex: "op_kind",
      key: "op_kind",
      render: (v: string) => <Tag>{v}</Tag>,
    },
    {
      title: t("deviceSync.target", "Target"),
      dataIndex: "target_id",
      key: "target_id",
      ellipsis: true,
    },
    {
      title: t("deviceSync.before", "Local"),
      dataIndex: "before_value",
      key: "before_value",
      ellipsis: true,
    },
    {
      title: t("deviceSync.after", "Remote"),
      dataIndex: "after_value",
      key: "after_value",
      ellipsis: true,
    },
    {
      title: t("deviceSync.device", "Device"),
      dataIndex: "originating_device",
      key: "originating_device",
      ellipsis: true,
      width: 160,
    },
    {
      title: t("deviceSync.time", "Time"),
      dataIndex: "observed_at",
      key: "observed_at",
      width: 180,
      render: (v: number) => new Date(v).toLocaleString(),
    },
  ];

  return (
    <>
      <h3 style={{ margin: "0 0 12px" }}>
        {t("deviceSync.conflicts", "Sync Conflicts")}
      </h3>
      <span
        style={{ color: "var(--text-2)", display: "block", marginBottom: 12 }}
      >
        {t(
          "deviceSync.conflictsHint",
          "Showing conflicts from the last 24 hours.",
        )}
      </span>
      <Table<ConflictEntry>
        columns={columns}
        dataSource={data}
        rowKey="id"
        loading={loading && data.length > 0}
        pagination={{ pageSize: 20 }}
        size="sm"
      />
    </>
  );
}

function DisabledOverlay() {
  const { t } = useTranslation();
  return (
    <div
      data-testid="disabled-overlay"
      style={{
        position: "relative",
        opacity: 0.4,
        pointerEvents: "none",
        userSelect: "none",
      }}
    >
      <div style={{ textAlign: "center", padding: "40px 0" }}>
        <span style={{ color: "var(--text-2)" }}>
          {t("deviceSync.enableFirst")}
        </span>
      </div>
    </div>
  );
}

export default function DeviceSyncPage() {
  const { t } = useTranslation();
  const [syncEnabled, setSyncEnabled] = useState<boolean | null>(() =>
    readCachedSyncEnabled(),
  );
  const [statusRefreshKey, setStatusRefreshKey] = useState(0);
  const [diagnostics, setDiagnostics] =
    useState<SyncDiagnosticsResponse | null>(null);

  const handleStatusChange = useCallback((enabled: boolean) => {
    setSyncEnabled(enabled);
  }, []);

  return (
    <Page>
      <PageHeader current={t("deviceSync.title", "Device Sync")} />
      <PageBody>
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "var(--space-3)",
            width: "100%",
            padding: "var(--space-4)",
          }}
        >
          <SyncStatusSection
            diagnostics={diagnostics}
            onConfigChange={handleStatusChange}
            onDiagnosticsChange={setDiagnostics}
            refreshKey={statusRefreshKey}
          />

          {syncEnabled === false ? (
            <DisabledOverlay />
          ) : syncEnabled === true ? (
            <>
              <hr className="rua-divider" />
              <PowerPolicySection />
              <hr className="rua-divider" />
              <DevicesSection
                diagnostics={diagnostics}
                onDeviceChange={() => setStatusRefreshKey((k) => k + 1)}
              />
              <hr className="rua-divider" />
              <ConflictPolicySection />
              <hr className="rua-divider" />
              <ConflictsSection />
            </>
          ) : null}
        </div>
      </PageBody>
    </Page>
  );
}
