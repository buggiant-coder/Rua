import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { sharedMcpApi } from "../../../../api";
import type { MCPClientInfo } from "../../../../api/types";
import type { SharedMcpBroadcastResult } from "../../../../api/modules/sharedMcp";
import { EmptyState } from "@/components/EmptyState";
import skillStyles from "../../../Agent/Skills/index.module.less";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import { Btn, Modal, toast } from "@/ui";

interface LoadFromSharedModalProps {
  open: boolean;
  onClose: () => void;
  agentId: string;
  onLoaded: () => void;
}

export function LoadFromSharedModal({
  open,
  onClose,
  agentId,
  onLoaded,
}: LoadFromSharedModalProps) {
  const { t } = useTranslation();
  const [sharedClients, setSharedClients] = useState<MCPClientInfo[]>([]);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [broadcasting, setBroadcasting] = useState(false);
  const [conflictInfo, setConflictInfo] = useState<{
    results: SharedMcpBroadcastResult[];
    pendingKeys: string[];
  } | null>(null);

  useEffect(() => {
    if (open) {
      setLoading(true);
      sharedMcpApi
        .list()
        .then((data) => setSharedClients(data))
        .catch(() => toast.error(t("sharedMcp.loadError")))
        .finally(() => setLoading(false));
      setSelectedKeys([]);
      setConflictInfo(null);
    }
  }, [open]);

  const toggleKey = (key: string) => {
    setSelectedKeys((prev) =>
      prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
    );
  };

  const handleLoad = useCallback(
    async (conflictMode: string = "ask") => {
      if (selectedKeys.length === 0) return;
      setBroadcasting(true);
      try {
        const keysToSend = conflictInfo
          ? conflictInfo.pendingKeys
          : selectedKeys;
        const resp = await sharedMcpApi.broadcast(
          keysToSend,
          [agentId],
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
            toast.error(t("sharedMcp.broadcastFailed"));
          } else {
            toast.success(t("sharedMcp.broadcastSuccess"));
          }
          setConflictInfo(null);
          onLoaded();
          onClose();
        }
      } catch {
        toast.error(t("sharedMcp.broadcastFailed"));
      } finally {
        setBroadcasting(false);
      }
    },
    [selectedKeys, conflictInfo, agentId, t, onClose, onLoaded],
  );

  return (
    <>
      <Modal
        title={t("sharedMcp.loadFromShared")}
        open={open && !conflictInfo}
        onClose={onClose}
        onOk={() => handleLoad("ask")}
        okButtonProps={{
          disabled: selectedKeys.length === 0,
          loading: broadcasting,
        }}
        okText={`${t("sharedMcp.loadFromShared")} (${selectedKeys.length})`}
        width={640}
      >
        {loading ? (
          <div className={styles.loading}>
            <p>{t("common.loading")}</p>
          </div>
        ) : sharedClients.length === 0 ? (
          <EmptyState description={t("sharedMcp.emptyState")} />
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
                      setSelectedKeys(sharedClients.map((c) => c.key))
                    }
                  >
                    {t("agent.selectAll")}
                  </Btn>
                  <Btn size="sm" onClick={() => setSelectedKeys([])}>
                    {t("skills.clearSelection")}
                  </Btn>
                </div>
              </div>
            </div>

            <div
              className={`${skillStyles.pickerGrid} ${skillStyles.compactPickerGrid}`}
            >
              {sharedClients.map((client) => {
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
            <Btn onClick={() => handleLoad("skip")} loading={broadcasting}>
              {t("sharedMcp.skip")}
            </Btn>
            <Btn
              variant="danger"
              onClick={() => handleLoad("overwrite")}
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
