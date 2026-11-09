import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { MarkdownFile } from "../../../../api/types";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import { Btn, Modal } from "@/ui";

interface SyncToLibraryModalProps {
  open: boolean;
  files: MarkdownFile[];
  onClose: () => void;
  onConfirm: (fileNames: string[]) => Promise<void>;
}

export function SyncToLibraryModal({
  open,
  files,
  onClose,
  onConfirm,
}: SyncToLibraryModalProps) {
  const { t } = useTranslation();
  const [selectedNames, setSelectedNames] = useState<string[]>([]);
  const [confirming, setConfirming] = useState(false);

  useEffect(() => {
    if (open) {
      setSelectedNames([]);
      setConfirming(false);
    }
  }, [open]);

  const handleOk = async () => {
    if (selectedNames.length === 0) return;
    setConfirming(true);
    try {
      await onConfirm(selectedNames);
    } finally {
      setConfirming(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("workspace.batchSyncToLibrary")}
      footer={
        <div className={styles.modalFooter}>
          <Btn onClick={onClose} className={styles.modalCancelButton}>
            {t("common.cancel")}
          </Btn>
          <Btn
            onClick={handleOk}
            disabled={selectedNames.length === 0}
            loading={confirming}
            className={styles.modalOkButton}
          >
            {t("common.confirm")}
          </Btn>
        </div>
      }
      width={600}
      className={styles.syncTransferModal}
    >
      <div className={styles.pickerSection}>
        <div className={styles.pickerHeader}>
          <div className={styles.pickerLabel}>
            {t("workspace.selectSyncFiles")}
          </div>
          <div className={styles.bulkActions}>
            <Btn
              size="sm"
              onClick={() => setSelectedNames([])}
              className={styles.bulkActionButton}
            >
              {t("skills.clearSelection")}
            </Btn>
            <Btn
              size="sm"
              onClick={() => setSelectedNames(files.map((f) => f.filename))}
              className={styles.bulkActionButton}
            >
              {t("skills.selectAll")}
            </Btn>
          </div>
        </div>

        <div className={styles.pickerGrid}>
          {files.map((file) => {
            const selected = selectedNames.includes(file.filename);
            return (
              <div
                key={file.filename}
                className={`${styles.pickerCard} ${
                  selected ? styles.pickerCardSelected : ""
                }`}
                onClick={() =>
                  setSelectedNames(
                    selected
                      ? selectedNames.filter((n) => n !== file.filename)
                      : [...selectedNames, file.filename],
                  )
                }
              >
                {selected && (
                  <span className={styles.pickerCheck}>
                    <Check />
                  </span>
                )}
                <div className={styles.pickerCardTitle}>{file.filename}</div>
              </div>
            );
          })}
        </div>
      </div>
    </Modal>
  );
}
