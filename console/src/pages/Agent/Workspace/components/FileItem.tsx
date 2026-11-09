import React from "react";

import { Toggle, Tooltip } from "@/ui";
import type { MarkdownFile, SubdirFile } from "../../../../api/types";
import { formatFileSize } from "./utils";
import { useTranslation } from "react-i18next";
import styles from "../index.module.less";
import { ChevronDown, ChevronRight, Plus } from "lucide-react";

interface FileItemProps {
  file: MarkdownFile;
  selectedFile: MarkdownFile | null;
  isExpanded: boolean;
  subdirFiles: SubdirFile[];
  hasSubdir: boolean;
  enabled?: boolean;
  enabledFiles: string[];
  onFileClick: (file: MarkdownFile) => void;
  onSubdirFileClick: (parentFilename: string, file: SubdirFile) => void;
  onCreateSubdirFile: (parentFilename: string) => void;
  onToggleEnabled: (filename: string) => void;
  batchDeleteMode?: boolean;
  batchSelected?: boolean;
  batchSelectedSubs?: Set<string>;
  onToggleBatchSelect?: (filename: string) => void;
}

export const FileItem: React.FC<FileItemProps> = ({
  file,
  selectedFile,
  isExpanded,
  subdirFiles,
  hasSubdir,
  enabled = false,
  enabledFiles,
  onFileClick,
  onSubdirFileClick,
  onCreateSubdirFile,
  onToggleEnabled,
  batchDeleteMode = false,
  batchSelected = false,
  batchSelectedSubs = new Set(),
  onToggleBatchSelect,
}) => {
  const { t } = useTranslation();
  const isSelected = selectedFile?.filename === file.filename;

  const handleToggleClick = (e: React.ChangeEvent<HTMLInputElement>) => {
    e.stopPropagation();
    onToggleEnabled(file.filename);
  };

  const handleCreateClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onCreateSubdirFile(file.filename);
  };

  const subdirEnabledKey = (sub: SubdirFile) => {
    const subdir =
      sub.subdir || file.filename.replace(/\.md$/i, "").toUpperCase();
    return `${subdir}/${sub.filename}`;
  };

  const handleSubToggleClick = (
    sub: SubdirFile,
    e: React.ChangeEvent<HTMLInputElement>,
  ) => {
    e.stopPropagation();
    onToggleEnabled(subdirEnabledKey(sub));
  };

  return (
    <div>
      <div
        onClick={() => onFileClick(file)}
        className={`${styles.fileItem} ${isSelected ? styles.selected : ""}`}
      >
        <div className={styles.fileItemHeader}>
          <div className={styles.fileInfo}>
            <div className={styles.fileItemName}>
              {batchDeleteMode && !enabled && (
                <span
                  className={`${styles.batchCheckbox} ${
                    batchSelected ? styles.batchChecked : ""
                  }`}
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleBatchSelect?.(file.filename);
                  }}
                />
              )}
              {enabled && <span className={styles.enabledBadge}>●</span>}
              {file.filename.replace(/\.md$/i, "")}
              <span className={styles.fileItemMetaInline}>
                {formatFileSize(file.size)}
              </span>
            </div>
          </div>
          <div className={styles.fileItemActions}>
            <span onClick={(e) => e.stopPropagation()}>
              <Tooltip
                title={t("workspace.systemPromptToggleTooltip")}
                placement="right"
                mouseEnterDelay={0.8}
              >
                <Toggle checked={enabled} onChange={handleToggleClick} />
              </Tooltip>
            </span>
            <Tooltip
              title={t("workspace.createSubFile") || "New sub-file"}
              placement="top"
              mouseEnterDelay={0.8}
            >
              <span
                className={styles.createSubdirBtn}
                onClick={handleCreateClick}
              >
                <Plus />
              </span>
            </Tooltip>
            {hasSubdir && (
              <span className={styles.expandIcon}>
                {isExpanded ? <ChevronDown /> : <ChevronRight />}
              </span>
            )}
          </div>
        </div>
      </div>

      {isExpanded && subdirFiles.length > 0 && (
        <div className={styles.subdirFileList}>
          {subdirFiles.map((sub) => {
            const isSubSelected = selectedFile?.filename === sub.filename;
            const subEnabled = enabledFiles.includes(subdirEnabledKey(sub));
            return (
              <div
                key={sub.filename}
                onClick={() => onSubdirFileClick(file.filename, sub)}
                className={`${styles.subdirFileItem} ${
                  isSubSelected ? styles.selected : ""
                }`}
              >
                <div className={styles.subdirFileHeader}>
                  <div className={styles.subdirFileName}>
                    {batchDeleteMode && !subEnabled && (
                      <span
                        className={`${styles.batchCheckbox} ${
                          batchSelectedSubs.has(subdirEnabledKey(sub))
                            ? styles.batchChecked
                            : ""
                        }`}
                        onClick={(e) => {
                          e.stopPropagation();
                          onToggleBatchSelect?.(subdirEnabledKey(sub));
                        }}
                      />
                    )}
                    {sub.filename.replace(/\.md$/i, "")}
                    <span className={styles.fileItemMetaInline}>
                      {formatFileSize(sub.size)}
                    </span>
                  </div>
                  <span onClick={(e) => e.stopPropagation()}>
                    <Tooltip
                      title={t("workspace.systemPromptToggleTooltip")}
                      placement="right"
                      mouseEnterDelay={0.8}
                    >
                      <Toggle
                        checked={subEnabled}
                        onChange={(e) => handleSubToggleClick(sub, e)}
                      />
                    </Tooltip>
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
