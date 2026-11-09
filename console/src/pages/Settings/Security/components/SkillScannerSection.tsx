import { getSelectedAgentId } from "@/api/authHeaders";
import { useState, useCallback, useRef } from "react";
import type { TagProps } from "@/ui/Tag";
import { RuaCard } from "@/components/RuaCard";
import { EmptyState } from "@/components/EmptyState";
import { Trash2, ShieldCheck, Eye } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useSkillScanner } from "../useSkillScanner";
import type {
  BlockedSkillRecord,
  BlockedSkillFinding,
  SkillScannerWhitelistEntry,
  SkillScannerMode,
  ManualScanResponse,
} from "../../../../api/modules/security";
import { securityApi } from "../../../../api/modules/security";
import { skillApi } from "../../../../api/modules/skill";
import { useTheme } from "../../../../contexts/ThemeContext";
import styles from "../index.module.less";
import {
  Btn,
  Modal,
  NumberInput,
  Select,
  Table,
  Tabs,
  Tag,
  Toggle,
  Tooltip,
  toast,
} from "@/ui";
function FindingsModal({
  findings,
  skillName,
  open,
  onClose,
}: {
  findings: BlockedSkillFinding[];
  skillName: string;
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();

  const severityColor = (s: string): TagProps["color"] => {
    switch (s.toUpperCase()) {
      case "CRITICAL":
        return "danger";
      case "HIGH":
        return "warning";
      case "MEDIUM":
        return "warning";
      case "LOW":
        return "primary";
      default:
        return "default";
    }
  };

  return (
    <Modal
      className="modal-fullscreen-mobile"
      title={`${t(
        "security.skillScanner.scanAlerts.viewFindings",
      )} - ${skillName}`}
      open={open}
      onClose={onClose}
      footer={null}
      width={700}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        {findings.map((finding, idx) => (
          <div
            key={idx}
            style={{
              border: "1px solid var(--border)",
              borderRadius: "var(--radius-sm)",
              padding: "12px 16px",
            }}
          >
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                marginBottom: 8,
              }}
            >
              <Tag
                color={severityColor(finding.severity)}
                style={{ margin: 0 }}
              >
                {finding.severity}
              </Tag>
              <span style={{ fontWeight: 600, fontSize: 16 }}>
                {t(`security.skillScanner.rules.${finding.rule_id}.title`, {
                  defaultValue: finding.title,
                })}
              </span>
            </div>
            <div
              style={{ color: "var(--text-2)", fontSize: 14, marginBottom: 6 }}
            >
              {finding.line_number
                ? `${finding.file_path}:${finding.line_number}`
                : finding.file_path}
            </div>
            <div
              style={{ color: "var(--text-1)", fontSize: 14, marginBottom: 4 }}
            >
              {t(`security.skillScanner.rules.${finding.rule_id}.description`, {
                defaultValue: finding.description,
              })}
            </div>
            <div style={{ color: "var(--text-3)", fontSize: 12 }}>
              {finding.rule_id}
            </div>
          </div>
        ))}
      </div>
    </Modal>
  );
}

function SourceTag({
  source,
  t,
}: {
  source?: string;
  t: (key: string, opts?: Record<string, string>) => string;
}) {
  if (!source) return null;
  if (source === "pool") {
    return (
      <Tag color="primary" style={{ margin: 0, fontSize: 11 }}>
        {t("security.skillScanner.source.pool")}
      </Tag>
    );
  }
  if (source.startsWith("workspace:")) {
    const agentId = source.slice("workspace:".length);
    return (
      <Tag color="success" style={{ margin: 0, fontSize: 11 }}>
        {t("security.skillScanner.source.workspace", { agentId })}
      </Tag>
    );
  }
  return null;
}

export function SkillScannerSection() {
  const { t } = useTranslation();
  const { isDark } = useTheme();
  const darkBtnStyle = isDark ? { color: "var(--text-2)" } : undefined;
  const {
    config,
    blockedHistory,
    whitelist,
    loading,
    updateConfig,
    addToWhitelist,
    removeFromWhitelist,
    removeBlockedEntry,
    clearBlockedHistory,
    fetchAll,
  } = useSkillScanner();
  const [activeInnerTab, setActiveInnerTab] = useState("scanAlerts");
  const [saving, setSaving] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [scanResult, setScanResult] = useState<ManualScanResponse | null>(null);
  const [findingsModal, setFindingsModal] = useState<{
    open: boolean;
    findings: BlockedSkillFinding[];
    skillName: string;
  }>({ open: false, findings: [], skillName: "" });

  const handleModeChange = useCallback(
    async (mode: SkillScannerMode) => {
      setSaving(true);
      const ok = await updateConfig({ mode });
      if (ok) toast.success(t("security.skillScanner.saveSuccess"));
      else toast.error(t("security.skillScanner.saveFailed"));
      setSaving(false);
    },
    [updateConfig, t],
  );

  const lastActiveModeRef = useRef<SkillScannerMode>("block");
  if (config && config.mode !== "off") {
    lastActiveModeRef.current = config.mode;
  }

  const handleEnabledToggle = useCallback(
    async (checked: boolean) => {
      const nextMode: SkillScannerMode = checked
        ? lastActiveModeRef.current
        : "off";
      setSaving(true);
      const ok = await updateConfig({ mode: nextMode });
      if (ok) toast.success(t("security.skillScanner.saveSuccess"));
      else toast.error(t("security.skillScanner.saveFailed"));
      setSaving(false);
    },
    [updateConfig, t],
  );

  const [pendingTimeout, setPendingTimeout] = useState<number | null>(null);

  const handleTimeoutBlur = useCallback(async () => {
    const value = pendingTimeout;
    if (value === null || value < 5 || value > 300) {
      setPendingTimeout(null);
      return;
    }
    setSaving(true);
    const ok = await updateConfig({ timeout: value });
    if (ok) toast.success(t("security.skillScanner.saveSuccess"));
    else toast.error(t("security.skillScanner.saveFailed"));
    setPendingTimeout(null);
    setSaving(false);
  }, [pendingTimeout, updateConfig, t]);

  const handleAllowSkill = useCallback(
    async (record: BlockedSkillRecord, index: number) => {
      const ok = await addToWhitelist(record.skill_name, record.content_hash);
      if (ok) {
        toast.success(t("security.skillScanner.whitelist.addSuccess"));
        await removeBlockedEntry(index);
      } else {
        toast.error(t("security.skillScanner.whitelist.addFailed"));
      }
    },
    [addToWhitelist, removeBlockedEntry, t],
  );

  const handleRemoveWhitelist = useCallback(
    async (skillName: string) => {
      Modal.confirm({
        title: t("security.skillScanner.whitelist.removeConfirm"),
        content: t("security.skillScanner.whitelist.removeWillDisable"),
        onOk: async () => {
          const ok = await removeFromWhitelist(skillName);
          if (!ok) {
            toast.error(t("security.skillScanner.whitelist.removeFailed"));
            return;
          }
          try {
            await skillApi.disableSkill(skillName, getSelectedAgentId());
            toast.success(
              t("security.skillScanner.whitelist.removeAndDisabled"),
            );
          } catch {
            toast.success(t("security.skillScanner.whitelist.removeSuccess"));
          }
        },
      });
    },
    [removeFromWhitelist, t],
  );

  const handleClearHistory = useCallback(() => {
    Modal.confirm({
      title: t("security.skillScanner.scanAlerts.clearConfirm"),
      onOk: async () => {
        await clearBlockedHistory();
      },
    });
  }, [clearBlockedHistory, t]);

  const handleManualScan = useCallback(async () => {
    setScanning(true);
    setScanResult(null);
    try {
      const result = await securityApi.triggerScan();
      setScanResult(result);
      if (result.issues > 0) {
        toast.warning(
          t("security.skillScanner.manualScan.resultWithIssues", {
            scanned: result.scanned,
            issues: result.issues,
          }),
        );
      } else {
        toast.success(
          t("security.skillScanner.manualScan.resultClean", {
            scanned: result.scanned,
          }),
        );
      }
      await fetchAll();
    } catch {
      toast.error(t("security.skillScanner.manualScan.failed"));
    } finally {
      setScanning(false);
    }
  }, [fetchAll, t]);

  if (loading || !config) return null;

  const enabled = config.mode !== "off";

  const blockedColumns = [
    {
      title: t("security.skillScanner.scanAlerts.skillName"),
      dataIndex: "skill_name",
      key: "skill_name",
      width: 220,
      stackLine: 1,
      render: (_: string, record: BlockedSkillRecord) => (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            flexWrap: "wrap",
            gap: 4,
            minWidth: 0,
          }}
        >
          <span style={{ overflowWrap: "anywhere" }}>{record.skill_name}</span>
          <SourceTag source={record.source} t={t} />
        </div>
      ),
    },
    {
      title: t("security.skillScanner.scanAlerts.action"),
      dataIndex: "action",
      key: "action",
      width: 100,
      stackLine: 1,
      render: (action: string) => (
        <Tag color={action === "blocked" ? "danger" : "warning"}>
          {action === "blocked"
            ? t("security.skillScanner.scanAlerts.actionBlocked")
            : t("security.skillScanner.scanAlerts.actionWarned")}
        </Tag>
      ),
    },
    {
      title: t("security.skillScanner.scanAlerts.time"),
      dataIndex: "blocked_at",
      key: "blocked_at",
      width: 180,
      stackLine: 1,
      render: (val: string) => {
        try {
          return new Date(val).toLocaleString();
        } catch {
          return val;
        }
      },
    },
    {
      title: t("security.skillScanner.scanAlerts.actions"),
      key: "actions",
      width: 200,
      stackLine: 1,
      render: (_: unknown, record: BlockedSkillRecord, index: number) => (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            flexWrap: "wrap",
            gap: "var(--space-2)",
          }}
        >
          <Tooltip title={t("security.skillScanner.scanAlerts.viewFindings")}>
            <Btn
              variant="outline"
              style={darkBtnStyle}
              onClick={() =>
                setFindingsModal({
                  open: true,
                  findings: record.findings,
                  skillName: record.skill_name,
                })
              }
            >
              <Eye size={14} />
            </Btn>
          </Tooltip>
          <Tooltip title={t("security.skillScanner.scanAlerts.allowSkill")}>
            <Btn
              variant="outline"
              style={darkBtnStyle}
              onClick={() => handleAllowSkill(record, index)}
            >
              <ShieldCheck size={14} />
            </Btn>
          </Tooltip>
          <Tooltip title={t("security.skillScanner.scanAlerts.remove")}>
            <Btn variant="danger" onClick={() => removeBlockedEntry(index)}>
              <Trash2 size={14} />
            </Btn>
          </Tooltip>
        </div>
      ),
    },
  ];

  const whitelistColumns = [
    {
      title: t("security.skillScanner.whitelist.skillName"),
      dataIndex: "skill_name",
      key: "skill_name",
      width: 200,
      stackLine: 1,
    },
    {
      title: t("security.skillScanner.whitelist.contentHash"),
      dataIndex: "content_hash",
      key: "content_hash",
      width: 200,
      stackLine: 2,
      render: (hash: string) =>
        hash ? (
          <Tooltip title={hash}>
            <code className={styles.codeHash}>{hash}</code>
          </Tooltip>
        ) : (
          <span style={{ color: "var(--text-3)" }}>any</span>
        ),
    },
    {
      title: t("security.skillScanner.whitelist.addedAt"),
      dataIndex: "added_at",
      key: "added_at",
      width: 180,
      stackLine: 1,
      render: (val: string) => {
        try {
          return new Date(val).toLocaleString();
        } catch {
          return val;
        }
      },
    },
    {
      title: t("security.skillScanner.whitelist.actions"),
      key: "actions",
      width: 100,
      stackLine: 1,
      render: (_: unknown, record: SkillScannerWhitelistEntry) => (
        <Tooltip title={t("security.skillScanner.whitelist.remove")}>
          <Btn
            variant="danger"
            onClick={() => handleRemoveWhitelist(record.skill_name)}
          >
            <Trash2 size={14} />
          </Btn>
        </Tooltip>
      ),
    },
  ];

  return (
    <div className={styles.skillScannerRoot}>
      <RuaCard className={styles.formCard}>
        <div className={styles.skillScannerEnableRow}>
          <span className={styles.skillScannerEnableLabel}>
            {t("security.skillScanner.enableLabel")}
          </span>
          <Toggle
            checked={enabled}
            onChange={(e) => handleEnabledToggle(e.target.checked)}
            disabled={saving}
          />
        </div>

        <div className={styles.skillScannerConfig}>
          <div className={styles.skillScannerConfigItem}>
            <Tooltip title={t("security.skillScanner.modeTooltip")}>
              <span className={styles.skillScannerLabel}>
                {t("security.skillScanner.mode")}
              </span>
            </Tooltip>
            <Select
              value={enabled ? config.mode : "block"}
              onChange={handleModeChange}
              disabled={saving || !enabled}
              className={styles.skillScannerControl}
              options={[
                {
                  value: "block",
                  label: t("security.skillScanner.modeBlock"),
                },
                { value: "warn", label: t("security.skillScanner.modeWarn") },
              ]}
            />
          </div>

          <div className={styles.skillScannerConfigItem}>
            <Tooltip title={t("security.skillScanner.timeoutTooltip")}>
              <span className={styles.skillScannerLabel}>
                {t("security.skillScanner.timeout")}
              </span>
            </Tooltip>
            <NumberInput
              min={5}
              max={300}
              value={pendingTimeout ?? config.timeout}
              onChange={(v) => setPendingTimeout(v)}
              onBlur={handleTimeoutBlur}
              onPressEnter={handleTimeoutBlur}
              disabled={!enabled}
              className={styles.skillScannerControl}
            />
          </div>
        </div>

        <div style={{ marginTop: 12 }}>
          <Btn loading={scanning} onClick={handleManualScan}>
            {scanning
              ? t("security.skillScanner.manualScan.scanning")
              : t("security.skillScanner.manualScan.button")}
          </Btn>
          {scanResult && !scanning && (
            <span style={{ marginLeft: 12, color: "var(--text-2)" }}>
              {scanResult.issues > 0
                ? t("security.skillScanner.manualScan.resultWithIssues", {
                    scanned: scanResult.scanned,
                    issues: scanResult.issues,
                  })
                : t("security.skillScanner.manualScan.resultClean", {
                    scanned: scanResult.scanned,
                  })}
            </span>
          )}
        </div>
      </RuaCard>

      <Tabs
        className={styles.innerTabs}
        activeKey={activeInnerTab}
        onChange={setActiveInnerTab}
        items={[
          {
            key: "scanAlerts",
            label: (
              <span>
                {t("security.skillScanner.scanAlerts.title")}
                {blockedHistory.length > 0 && (
                  <span className={styles.tabBadge}>
                    {blockedHistory.length}
                  </span>
                )}
              </span>
            ),
            children: (
              <div className={styles.tabPanelContent}>
                {blockedHistory.length > 0 && (
                  <div className={styles.tabPanelHeader}>
                    <Btn
                      size="sm"
                      variant="danger"
                      onClick={handleClearHistory}
                    >
                      {t("security.skillScanner.scanAlerts.clearAll")}
                    </Btn>
                  </div>
                )}
                <RuaCard className={styles.tableCard} bare>
                  <Table
                    dataSource={blockedHistory}
                    columns={blockedColumns}
                    rowKey={(_, idx) => String(idx)}
                    pagination={false}
                    responsive="stack"
                    size="sm"
                    locale={{
                      emptyText: (
                        <EmptyState
                          description={t(
                            "security.skillScanner.scanAlerts.empty",
                          )}
                        />
                      ),
                    }}
                  />
                </RuaCard>
              </div>
            ),
          },
          {
            key: "whitelist",
            label: (
              <span>
                {t("security.skillScanner.whitelist.title")}
                {whitelist.length > 0 && (
                  <span className={styles.tabBadge}>{whitelist.length}</span>
                )}
              </span>
            ),
            children: (
              <div className={styles.tabPanelContent}>
                <RuaCard className={styles.tableCard} bare>
                  <Table
                    dataSource={whitelist}
                    columns={whitelistColumns}
                    rowKey="skill_name"
                    pagination={false}
                    responsive="stack"
                    size="sm"
                    locale={{
                      emptyText: (
                        <EmptyState
                          description={t(
                            "security.skillScanner.whitelist.empty",
                          )}
                        />
                      ),
                    }}
                  />
                </RuaCard>
              </div>
            ),
          },
        ]}
      />

      <FindingsModal
        findings={findingsModal.findings}
        skillName={findingsModal.skillName}
        open={findingsModal.open}
        onClose={() =>
          setFindingsModal({ open: false, findings: [], skillName: "" })
        }
      />
    </div>
  );
}
