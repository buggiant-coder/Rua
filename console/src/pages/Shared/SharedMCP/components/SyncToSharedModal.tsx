import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { sharedMcpApi } from "../../../../api";
import type { MCPClientInfo } from "../../../../api/types";
import type { SharedMcpSyncResult } from "../../../../api/modules/sharedMcp";
import { EmptyState } from "@/components/EmptyState";
import skillStyles from "../../../Agent/Skills/index.module.less";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import { Btn, Modal, toast } from "@/ui";

interface SyncToSharedModalProps {
  open: boolean;
  onClose: () => void;
  agentId: string;
  clients: MCPClientInfo[];
}

export function SyncToSharedModal({
  open,
  onClose,
  agentId,
  clients,
}: SyncToSharedModalProps) {
  const { t } = useTranslation();
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [conflictInfo, setConflictInfo] = useState<{
    results: SharedMcpSyncResult[];
    pendingKeys: string[];
  } | null>(null);

  useEffect(() => {
    if (open) {
      setSelectedKeys([]);
      setConflictInfo(null);
    }
  }, [open]);

  const toggleKey = (key: string) => {
    setSelectedKeys((prev) =>
      prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
    );
  };

  const handleSync = useCallback(
    async (conflictMode: string = "ask") => {
      if (selectedKeys.length === 0) return;
      setSyncing(true);
      try {
        const keysToSend = conflictInfo
          ? conflictInfo.pendingKeys
          : selectedKeys;
        const resp = await sharedMcpApi.sync(agentId, keysToSend, conflictMode);
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
          onClose();
        }
      } catch {
        toast.error(t("sharedMcp.syncFailed"));
      } finally {
        setSyncing(false);
      }
    },
    [agentId, selectedKeys, conflictInfo, t, onClose],
  );

  return (
    <>
      <Modal
        title={t("sharedMcp.syncToShared")}
        open={open && !conflictInfo}
        onClose={onClose}
        onOk={() => handleSync("ask")}
        okButtonProps={{
          disabled: selectedKeys.length === 0,
          loading: syncing,
        }}
        okText={`${t("sharedMcp.syncToShared")} (${selectedKeys.length})`}
        width={600}
      >
        {clients.length === 0 ? (
          <EmptyState description={t("mcp.emptyState")} />
        ) : (
          <div className={skillStyles.pickerSection}>
            <div className={skillStyles.pickerHeader}>
              <div className={skillStyles.pickerLabel}>
                {t("sharedMcp.selectClients")}
              </div>
              <div className={skillStyles.bulkActions}>
                <Btn size="sm" onClick={() => setSelectedKeys([])}>
                  {t("skills.clearSelection")}
                </Btn>
                <Btn
                  size="sm"
                  onClick={() => setSelectedKeys(clients.map((c) => c.key))}
                >
                  {t("skills.selectAll")}
                </Btn>
              </div>
            </div>

            <div
              className={`${skillStyles.pickerGrid} ${skillStyles.compactPickerGrid}`}
            >
              {clients.map((client) => {
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
