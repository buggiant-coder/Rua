import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { agentsApi, sharedMcpApi } from "../../../../api";
import type { SharedMcpBroadcastResult } from "../../../../api/modules/sharedMcp";
import type { AgentSummary } from "../../../../api/types/agents";
import type { MCPClientInfo } from "../../../../api/types";
import { getAgentDisplayName } from "../../../../utils/agentDisplayName";
import skillStyles from "../../../Agent/Skills/index.module.less";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import { Btn, Modal, toast } from "@/ui";

interface BroadcastModalProps {
  open: boolean;
  onClose: () => void;

  clients: MCPClientInfo[];

  defaultSelectedKeys: string[];
}

export function BroadcastModal({
  open,
  onClose,
  clients,
  defaultSelectedKeys,
}: BroadcastModalProps) {
  const { t } = useTranslation();
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [selectedAgentIds, setSelectedAgentIds] = useState<string[]>([]);
  const [selectedMcpKeys, setSelectedMcpKeys] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [broadcasting, setBroadcasting] = useState(false);
  const [conflictInfo, setConflictInfo] = useState<{
    results: SharedMcpBroadcastResult[];
    pendingKeys: string[];
    pendingAgentIds: string[];
  } | null>(null);

  useEffect(() => {
    if (open) {
      setLoading(true);
      agentsApi
        .listAgents()
        .then((resp) => setAgents(resp.agents || []))
        .catch(() => toast.error(t("sharedMcp.loadAgentsFailed")))
        .finally(() => setLoading(false));
      setSelectedAgentIds([]);
      setSelectedMcpKeys(defaultSelectedKeys);
      setConflictInfo(null);
    }
  }, [open, defaultSelectedKeys]);

  const handleBroadcast = useCallback(
    async (conflictMode: string = "ask") => {
      if (selectedAgentIds.length === 0 || selectedMcpKeys.length === 0) return;
      setBroadcasting(true);
      try {
        const keysToSend = conflictInfo
          ? conflictInfo.pendingKeys
          : selectedMcpKeys;
        const agentsToSend = conflictInfo
          ? conflictInfo.pendingAgentIds
          : selectedAgentIds;
        const resp = await sharedMcpApi.broadcast(
          keysToSend,
          agentsToSend,
          conflictMode,
        );
        const results = resp.results || [];

        const conflicts = results.filter((r) => r.status === "conflict");
        if (conflicts.length > 0 && conflictMode === "ask") {
          setConflictInfo({
            results: conflicts,
            pendingKeys: [...new Set(conflicts.map((c) => c.client_key))],
            pendingAgentIds: [
              ...new Set(
                conflicts.filter((c) => c.agent_id).map((c) => c.agent_id!),
              ),
            ],
          });
        } else {
          const errors = results.filter((r) => r.status === "error");
          if (errors.length > 0) {
            toast.error(t("sharedMcp.broadcastFailed"));
          } else {
            toast.success(t("sharedMcp.broadcastSuccess"));
          }
          setConflictInfo(null);
          onClose();
        }
      } catch {
        toast.error(t("sharedMcp.broadcastFailed"));
      } finally {
        setBroadcasting(false);
      }
    },
    [selectedMcpKeys, selectedAgentIds, conflictInfo, t, onClose],
  );

  const toggleAgent = (agentId: string) => {
    setSelectedAgentIds((prev) =>
      prev.includes(agentId)
        ? prev.filter((id) => id !== agentId)
        : [...prev, agentId],
    );
  };

  const toggleMcp = (key: string) => {
    setSelectedMcpKeys((prev) =>
      prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
    );
  };

  return (
    <>
      <Modal
        title={t("sharedMcp.broadcastToAgents")}
        open={open && !conflictInfo}
        onClose={onClose}
        onOk={() => handleBroadcast("ask")}
        okButtonProps={{
          disabled:
            selectedAgentIds.length === 0 || selectedMcpKeys.length === 0,
          loading: broadcasting,
        }}
        okText={`${t("sharedMcp.broadcastToAgents")} (${
          selectedMcpKeys.length
        } → ${selectedAgentIds.length})`}
        width={640}
      >
        {loading ? (
          <div className={styles.loading}>
            <p>{t("common.loading")}</p>
          </div>
        ) : (
          <div style={{ display: "grid", gap: 12 }}>
            <div className={skillStyles.pickerSection}>
              <div className={skillStyles.pickerHeader}>
                <div className={skillStyles.pickerLabel}>
                  {t("sharedMcp.selectClients")}
                </div>
                <div className={skillStyles.bulkActions}>
                  <Btn
                    size="sm"
                    onClick={() =>
                      setSelectedMcpKeys(clients.map((c) => c.key))
                    }
                  >
                    {t("agent.selectAll")}
                  </Btn>
                  <Btn size="sm" onClick={() => setSelectedMcpKeys([])}>
                    {t("skills.clearSelection")}
                  </Btn>
                </div>
              </div>
            </div>

            <div
              className={`${skillStyles.pickerGrid} ${skillStyles.compactPickerGrid}`}
            >
              {clients.map((client) => {
                const selected = selectedMcpKeys.includes(client.key);
                return (
                  <div
                    key={client.key}
                    className={`${skillStyles.pickerCard} ${
                      skillStyles.compactPickerCard
                    } ${selected ? skillStyles.pickerCardSelected : ""}`}
                    onClick={() => toggleMcp(client.key)}
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
                      {client.name || client.key}
                    </div>
                  </div>
                );
              })}
            </div>

            <div className={skillStyles.pickerSection}>
              <div className={skillStyles.pickerHeader}>
                <div className={skillStyles.pickerLabel}>
                  {t("sharedMcp.selectAgents")}
                </div>
                <div className={skillStyles.bulkActions}>
                  <Btn
                    size="sm"
                    onClick={() => setSelectedAgentIds(agents.map((a) => a.id))}
                  >
                    {t("agent.selectAll")}
                  </Btn>
                  <Btn size="sm" onClick={() => setSelectedAgentIds([])}>
                    {t("skills.clearSelection")}
                  </Btn>
                </div>
              </div>
            </div>

            <div
              className={`${skillStyles.pickerGrid} ${skillStyles.compactPickerGrid}`}
            >
              {agents.map((agent) => {
                const selected = selectedAgentIds.includes(agent.id);
                return (
                  <div
                    key={agent.id}
                    className={`${skillStyles.pickerCard} ${
                      skillStyles.compactPickerCard
                    } ${selected ? skillStyles.pickerCardSelected : ""}`}
                    onClick={() => toggleAgent(agent.id)}
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
                      {getAgentDisplayName({
                        id: agent.id,
                        name: agent.name ?? "",
                      })}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}
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
            <Btn onClick={() => handleBroadcast("skip")} loading={broadcasting}>
              {t("sharedMcp.skip")}
            </Btn>
            <Btn
              variant="danger"
              onClick={() => handleBroadcast("overwrite")}
              loading={broadcasting}
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
