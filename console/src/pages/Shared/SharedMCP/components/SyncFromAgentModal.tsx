import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { agentsApi, sharedMcpApi } from "../../../../api";
import type { MCPClientInfo } from "../../../../api/types";
import type { SharedMcpSyncResult } from "../../../../api/modules/sharedMcp";
import type { AgentSummary } from "../../../../api/types/agents";
import { getAgentDisplayName } from "../../../../utils/agentDisplayName";
import { EmptyState } from "@/components/EmptyState";
import skillStyles from "../../../Agent/Skills/index.module.less";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import { Btn, Modal, Select, toast } from "@/ui";

interface SyncFromAgentModalProps {
  open: boolean;
  onClose: () => void;
  onSynced: () => void;
}

export function SyncFromAgentModal({
  open,
  onClose,
  onSynced,
}: SyncFromAgentModalProps) {
  const { t } = useTranslation();
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [selectedAgentId, setSelectedAgentId] = useState<string>("");
  const [agentClients, setAgentClients] = useState<MCPClientInfo[]>([]);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const [loadingAgents, setLoadingAgents] = useState(false);
  const [loadingClients, setLoadingClients] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [conflictInfo, setConflictInfo] = useState<{
    results: SharedMcpSyncResult[];
    pendingKeys: string[];
  } | null>(null);

  useEffect(() => {
    if (open) {
      setLoadingAgents(true);
      agentsApi
        .listAgents()
        .then((resp) => setAgents(resp.agents || []))
        .catch(() => toast.error(t("sharedMcp.loadAgentsFailed")))
        .finally(() => setLoadingAgents(false));
      setSelectedAgentId("");
      setAgentClients([]);
      setSelectedKeys([]);
      setConflictInfo(null);
    }
  }, [open]);

  useEffect(() => {
    if (!selectedAgentId) {
      setAgentClients([]);
      setSelectedKeys([]);
      return;
    }
    setLoadingClients(true);
    sharedMcpApi
      .listAgentMcp(selectedAgentId)
      .then((data) => setAgentClients(data))
      .catch(() => {
        setAgentClients([]);
        toast.error(t("sharedMcp.loadAgentMcpFailed"));
      })
      .finally(() => setLoadingClients(false));
  }, [selectedAgentId]);

  const toggleKey = (key: string) => {
    setSelectedKeys((prev) =>
      prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
    );
  };

  const handleSync = useCallback(
    async (conflictMode: string = "ask") => {
      if (!selectedAgentId || selectedKeys.length === 0) return;
      setSyncing(true);
      try {
        const keysToSend = conflictInfo
          ? conflictInfo.pendingKeys
          : selectedKeys;
        const resp = await sharedMcpApi.sync(
          selectedAgentId,
          keysToSend,
          conflictMode,
        );
        const results = resp.results || [];

        const conflicts = results.filter((r) => r.status === "conflict");
        if (conflicts.length > 0 && conflictMode === "ask") {
          setConflictInfo({
            results: conflicts,
            pendingKeys: conflicts.map((c) => c.client_key),
          });
        } else {
          const errors = results.filter((r) => r.status === "error");
          if (errors.length > 0) {
            toast.error(t("sharedMcp.syncFailed"));
          } else {
            toast.success(t("sharedMcp.syncSuccess"));
          }
          setConflictInfo(null);
          onSynced();
          onClose();
        }
      } catch {
        toast.error(t("sharedMcp.syncFailed"));
      } finally {
        setSyncing(false);
      }
    },
    [selectedAgentId, selectedKeys, conflictInfo, t, onClose, onSynced],
  );

  return (
    <>
      <Modal
        title={t("sharedMcp.syncFromAgent")}
        open={open && !conflictInfo}
        onClose={onClose}
        onOk={() => handleSync("ask")}
        okButtonProps={{
          disabled: selectedKeys.length === 0,
          loading: syncing,
        }}
        okText={`${t("sharedMcp.syncFromAgent")} (${selectedKeys.length})`}
        width={640}
      >
        <div style={{ display: "grid", gap: 12 }}>
          <div className={skillStyles.pickerSection}>
            <div className={skillStyles.pickerLabel}>
              {t("sharedMcp.selectAgent")}
            </div>
            <Select
              style={{ width: "100%" }}
              placeholder={t("sharedMcp.selectAgent")}
              value={selectedAgentId || undefined}
              onChange={(val: string) => setSelectedAgentId(val)}
              loading={loadingAgents}
              options={agents.map((a) => ({
                value: a.id,
                label: getAgentDisplayName({ id: a.id, name: a.name ?? "" }),
              }))}
            />
          </div>

          {selectedAgentId && (
            <div className={skillStyles.pickerSection}>
              <div className={skillStyles.pickerHeader}>
                <div className={skillStyles.pickerLabel}>
                  {t("sharedMcp.selectClientsToSync")}
                </div>
                <div className={skillStyles.bulkActions}>
                  <Btn
                    size="sm"
                    onClick={() =>
                      setSelectedKeys(agentClients.map((c) => c.key))
                    }
                  >
                    {t("agent.selectAll")}
                  </Btn>
                  <Btn size="sm" onClick={() => setSelectedKeys([])}>
                    {t("skills.clearSelection")}
                  </Btn>
                </div>
              </div>

              {loadingClients ? (
                <div className={styles.loading}>
                  <p>{t("common.loading")}</p>
                </div>
              ) : agentClients.length === 0 ? (
                <EmptyState description={t("sharedMcp.noAgentMcp")} />
              ) : (
                <div
                  className={`${skillStyles.pickerGrid} ${skillStyles.compactPickerGrid}`}
                >
                  {agentClients.map((client) => {
                    const selected = selectedKeys.includes(client.key);
                    return (
                      <div
                        key={client.key}
                        className={`${skillStyles.pickerCard} ${
                          skillStyles.compactPickerCard
                        } ${selected ? skillStyles.pickerCardSelected : ""}`}
                        onClick={() => toggleKey(client.key)}
                      >
                        {selected && (
                          <span
                            className={`${skillStyles.pickerCheck} ${skillStyles.compactPickerCheck}`}
                          >
                            <Check />
                          </span>
                        )}
                        <div
                          className={`${skillStyles.pickerCardTitle} ${skillStyles.compactPickerTitle}`}
                        >
                          {client.name}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          )}
        </div>
      </Modal>

      <Modal
        title={t("sharedMcp.conflictTitle")}
        open={!!conflictInfo}
        onClose={() => {
          setConflictInfo(null);
          onClose();
        }}
        footer={
          <div className={styles.modalFooter}>
            <Btn
              onClick={() => {
                setConflictInfo(null);
                onClose();
              }}
            >
              {t("common.cancel")}
            </Btn>
            <Btn onClick={() => handleSync("skip")} loading={syncing}>
              {t("sharedMcp.skip")}
            </Btn>
            <Btn
              variant="danger"
              onClick={() => handleSync("overwrite")}
              loading={syncing}
            >
              {t("sharedMcp.overwrite")}
            </Btn>
          </div>
        }
        width={500}
      >
        {conflictInfo && (
          <p>
            {t("sharedMcp.conflictMessage", {
              name: conflictInfo.pendingKeys.join(", "),
            })}
          </p>
        )}
      </Modal>
    </>
  );
}
