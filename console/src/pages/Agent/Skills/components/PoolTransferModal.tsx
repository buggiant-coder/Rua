import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { PoolSkillSpec, SkillSpec } from "../../../../api/types";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import { Btn, Modal, Tooltip } from "@/ui";

interface PoolTransferModalProps {
  mode: "upload" | "download" | null;
  skills: SkillSpec[];
  poolSkills: PoolSkillSpec[];
  onClose: () => void;
  onUpload: (skillNames: string[]) => Promise<void>;
  onDownload: (poolSkillNames: string[]) => Promise<void>;
}

export function PoolTransferModal({
  mode,
  skills,
  poolSkills,
  onClose,
  onUpload,
  onDownload,
}: PoolTransferModalProps) {
  const { t } = useTranslation();
  const [workspaceSkillNames, setWorkspaceSkillNames] = useState<string[]>([]);
  const [poolSkillNames, setPoolSkillNames] = useState<string[]>([]);

  useEffect(() => {
    if (mode !== null) {
      setWorkspaceSkillNames([]);
      setPoolSkillNames([]);
    }
  }, [mode]);

  const handleCancel = () => {
    onClose();
  };

  const handleOk = async () => {
    if (mode === "upload") {
      await onUpload(workspaceSkillNames);
    } else {
      await onDownload(poolSkillNames);
    }
  };

  const isUpload = mode === "upload";
  const selectedNames = isUpload ? workspaceSkillNames : poolSkillNames;
  const setSelectedNames = isUpload
    ? setWorkspaceSkillNames
    : setPoolSkillNames;
  const items = isUpload ? skills : poolSkills;
  const hasSelection = selectedNames.length > 0;
  const builtinNames = items
    .filter((item) => item.source === "builtin")
    .map((item) => item.name);

  return (
    <Modal
      open={mode !== null}
      onClose={handleCancel}
      title={isUpload ? t("skills.uploadToPool") : t("skills.downloadFromPool")}
      footer={
        <div className={styles.modalFooter}>
          <Btn onClick={handleCancel} className={styles.modalCancelButton}>
            {t("common.cancel")}
          </Btn>
          <Btn
            onClick={handleOk}
            disabled={!hasSelection}
            className={styles.modalOkButton}
          >
            {t("common.confirm")}
          </Btn>
        </div>
      }
      width={600}
      className={styles.poolTransferModal}
    >
      <div className={styles.pickerSection}>
        <div className={styles.pickerHeader}>
          <div className={styles.pickerLabel}>
            {isUpload
              ? t("skills.selectWorkspaceSkill")
              : t("skills.selectPoolItem")}
          </div>
          <div className={styles.bulkActions}>
            <Btn
              onClick={() => setSelectedNames(items.map((s) => s.name))}
              className={styles.bulkActionButton}
            >
              {t("skills.selectAll")}
            </Btn>
            {!isUpload && (
              <Btn
                onClick={() => setSelectedNames(builtinNames)}
                disabled={builtinNames.length === 0}
                className={styles.bulkActionButton}
              >
                {t("agent.selectBuiltin")}
              </Btn>
            )}
            <Btn
              onClick={() => setSelectedNames([])}
              className={styles.bulkActionButton}
            >
              {t("skills.clearSelection")}
            </Btn>
          </div>
        </div>

        <div className={`${styles.pickerGrid} ${styles.compactPickerGrid}`}>
          {items.map((skill) => {
            const selected = selectedNames.includes(skill.name);
            return (
              <div
                key={skill.name}
                className={`${styles.pickerCard} ${styles.compactPickerCard} ${
                  selected ? styles.pickerCardSelected : ""
                }`}
                onClick={() =>
                  setSelectedNames(
                    selected
                      ? selectedNames.filter((n) => n !== skill.name)
                      : [...selectedNames, skill.name],
                  )
                }
              >
                {selected && (
                  <span
                    className={`${styles.pickerCheck} ${styles.compactPickerCheck}`}
                  >
                    <Check />
                  </span>
                )}
                <Tooltip title={skill.name}>
                  <div
                    className={`${styles.pickerCardTitle} ${styles.compactPickerTitle}`}
                  >
                    {skill.name}
                  </div>
                </Tooltip>
              </div>
            );
          })}
        </div>
      </div>
    </Modal>
  );
}
