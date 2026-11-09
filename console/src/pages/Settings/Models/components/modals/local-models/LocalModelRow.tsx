import { memo } from "react";

import { useTranslation } from "react-i18next";
import type { LocalModelInfo } from "../../../../../../api/types";
import styles from "../../../index.module.less";
import { formatFileSize } from "./shared";
import { Btn, Tooltip } from "@/ui";
import { Ban, Download, PlayCircle } from "lucide-react";
interface LocalModelRowProps {
  model: LocalModelInfo;
  currentRunningModelName: string | null;
  isModelDownloading: boolean;
  isServerBusy: boolean;
  startingModelName: string | null;
  stoppingServer: boolean;
  onStartDownload: (model: LocalModelInfo) => void;
  onStartServer: (model: LocalModelInfo) => void;
  onStopServer: () => void;
}

export const LocalModelRow = memo(function LocalModelRow({
  model,
  currentRunningModelName,
  isModelDownloading,
  isServerBusy,
  startingModelName,
  stoppingServer,
  onStartDownload,
  onStartServer,
  onStopServer,
}: LocalModelRowProps) {
  const { t } = useTranslation();
  const isRunning = currentRunningModelName === model.id;
  const isStarting = startingModelName === model.id;

  return (
    <div className={styles.modelListItem}>
      <div className={styles.modelListItemInfo}>
        <span className={styles.modelListItemName}>{model.name}</span>
        <span className={styles.modelListItemId}>
          {model.id} · {formatFileSize(model.size_bytes)}
        </span>
      </div>
      <div className={styles.modelListItemActions}>
        {model.downloaded ? (
          <span className={styles.modelListItemStatusButton}>
            {t("models.localDownloaded")}
          </span>
        ) : null}
        {!model.downloaded ? (
          <>
            <Btn
              size="sm"
              icon={<Download />}
              onClick={() => onStartDownload(model)}
              disabled={isModelDownloading || isServerBusy}
            >
              {t("common.download")}
            </Btn>
            <Tooltip title={t("models.localDownloadModelFirst")}>
              <Btn size="sm" icon={<PlayCircle />} disabled>
                {t("models.localStartServer")}
              </Btn>
            </Tooltip>
          </>
        ) : isRunning ? (
          <Btn
            variant="danger"
            size="sm"
            icon={<Ban />}
            loading={stoppingServer}
            onClick={onStopServer}
          >
            {t("models.localStopServer")}
          </Btn>
        ) : (
          <Btn
            size="sm"
            icon={<PlayCircle />}
            loading={isStarting}
            onClick={() => onStartServer(model)}
            disabled={isServerBusy}
          >
            {t("models.localStartServer")}
          </Btn>
        )}
      </div>
    </div>
  );
});
