import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { promptLibraryApi } from "../../../../api/modules/promptLibrary";
import type { MarkdownFile } from "../../../../api/types";
import styles from "../index.module.less";
import { Check } from "lucide-react";
import { Btn, Modal } from "@/ui";

interface LoadFromLibraryModalProps {
  open: boolean;
  onClose: () => void;
  onConfirm: (fileNames: string[]) => Promise<void>;
}

export function LoadFromLibraryModal({
  open,
  onClose,
  onConfirm,
}: LoadFromLibraryModalProps) {
  const { t } = useTranslation();
  const [libraryFiles, setLibraryFiles] = useState<MarkdownFile[]>([]);
  const [selectedNames, setSelectedNames] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [confirming, setConfirming] = useState(false);

  useEffect(() => {
    if (open) {
      setSelectedNames([]);
      setConfirming(false);
      setLoading(true);
      promptLibraryApi
        .listFiles()
        .then((files) => setLibraryFiles(files as MarkdownFile[]))
        .catch(() => setLibraryFiles([]))
        .finally(() => setLoading(false));
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
      title={t("workspace.loadFromLibrary")}
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
            {t("promptLibrary.selectFiles")}
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
              onClick={() =>
                setSelectedNames(libraryFiles.map((f) => f.filename))
              }
              className={styles.bulkActionButton}
            >
              {t("skills.selectAll")}
            </Btn>
          </div>
        </div>

        {loading ? (
          <div
            style={{ textAlign: "center", padding: 24, color: "var(--text-3)" }}
          >
            {t("common.loading")}
          </div>
        ) : libraryFiles.length === 0 ? (
          <div
            style={{ textAlign: "center", padding: 24, color: "var(--text-3)" }}
          >
            {t("promptLibrary.noFiles")}
          </div>
        ) : (
          <div className={styles.pickerGrid}>
            {libraryFiles.map((file) => {
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
        )}
      </div>
    </Modal>
  );
}
