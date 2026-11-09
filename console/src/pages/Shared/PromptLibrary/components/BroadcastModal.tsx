import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { MarkdownFile } from "../../../../api/types";
import type { AgentSummary } from "../../../../api/types/agents";
import { agentsApi } from "../../../../api/modules/agents";
import { getAgentDisplayName } from "../../../../utils/agentDisplayName";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import { Btn, Modal } from "@/ui";

interface BroadcastModalProps {
  open: boolean;
  files: MarkdownFile[];
  initialFiles: string[];
  onClose: () => void;
  onConfirm: (files: string[], agentIds: string[]) => Promise<void>;
}

export function BroadcastModal({
  open,
  files,
  initialFiles,
  onClose,
  onConfirm,
}: BroadcastModalProps) {
  const { t } = useTranslation();
  const [selectedFiles, setSelectedFiles] = useState<string[]>(initialFiles);
  const [selectedAgentIds, setSelectedAgentIds] = useState<string[]>([]);
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [confirming, setConfirming] = useState(false);

  useEffect(() => {
    if (open) {
      setSelectedFiles(initialFiles);
      setSelectedAgentIds([]);
      agentsApi
        .listAgents()
        .then((res) => setAgents(res.agents))
        .catch(() => {});
    }
  }, [open, initialFiles]);

  const handleConfirm = async () => {
    setConfirming(true);
    try {
      await onConfirm(selectedFiles, selectedAgentIds);
    } finally {
      setConfirming(false);
    }
  };

  const toggleFile = (filename: string) => {
    setSelectedFiles((prev) =>
      prev.includes(filename)
        ? prev.filter((f) => f !== filename)
        : [...prev, filename],
    );
  };

  const toggleAgent = (agentId: string) => {
    setSelectedAgentIds((prev) =>
      prev.includes(agentId)
        ? prev.filter((id) => id !== agentId)
        : [...prev, agentId],
    );
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      onOk={handleConfirm}
      confirmLoading={confirming}
      okButtonProps={{
        disabled: selectedFiles.length === 0 || selectedAgentIds.length === 0,
      }}
      title={t("promptLibrary.broadcast")}
      width={640}
    >
      <div style={{ display: "grid", gap: 12 }}>
        {}
        <div className={styles.pickerSection}>
          <div className={styles.pickerHeader}>
            <div className={styles.pickerLabel}>
              {t("promptLibrary.selectFiles")}
            </div>
            <div className={styles.bulkActions}>
              <Btn
                size="sm"
                onClick={() => setSelectedFiles(files.map((f) => f.filename))}
              >
                {t("agent.selectAll")}
              </Btn>
              <Btn size="sm" onClick={() => setSelectedFiles([])}>
                {t("skills.clearSelection")}
              </Btn>
            </div>
          </div>
        </div>

        <div className={styles.pickerGrid}>
          {files.map((file) => {
            const selected = selectedFiles.includes(file.filename);
            return (
              <div
                key={file.filename}
                className={`${styles.pickerCard} ${
                  selected ? styles.pickerCardSelected : ""
                }`}
                onClick={() => toggleFile(file.filename)}
              >
                {selected && (
                  <span className={styles.pickerCheck}>
                    <Check />
                  </span>
                )}
                <div className={styles.pickerCardTitle}>
                  {file.filename.replace(/\.md$/i, "")}
                </div>
              </div>
            );
          })}
        </div>

        {}
        <div className={styles.pickerSection}>
          <div className={styles.pickerHeader}>
            <div className={styles.pickerLabel}>
              {t("promptLibrary.selectAgents")}
            </div>
            <div className={styles.bulkActions}>
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

        <div className={styles.pickerGrid}>
          {agents.map((agent) => {
            const selected = selectedAgentIds.includes(agent.id);
            return (
              <div
                key={agent.id}
                className={`${styles.pickerCard} ${
                  selected ? styles.pickerCardSelected : ""
                }`}
                onClick={() => toggleAgent(agent.id)}
              >
                {selected && (
                  <span className={styles.pickerCheck}>
                    <Check />
                  </span>
                )}
                <div className={styles.pickerCardTitle}>
                  {getAgentDisplayName(agent)}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </Modal>
  );
}
