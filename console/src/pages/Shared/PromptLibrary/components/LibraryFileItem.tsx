import React from "react";

import type { MarkdownFile, SubdirFile } from "../../../../api/types";
import { formatFileSize } from "../../../Agent/Workspace/components/utils";
import { useTranslation } from "react-i18next";
import styles from "../index.module.less";
import { Tooltip } from "@/ui";
import { ChevronDown, ChevronRight, Plus } from "lucide-react";

interface LibraryFileItemProps {
  file: MarkdownFile;
  selectedFile: MarkdownFile | null;
  isExpanded: boolean;
  subdirFiles: SubdirFile[];
  hasSubdir: boolean;
  onFileClick: (file: MarkdownFile) => void;

  onToggleExpand: (parentFilename: string) => void;
  onSubdirFileClick: (parentFilename: string, file: SubdirFile) => void;
  onCreateSubdirFile: (parentFilename: string) => void;
  batchDeleteMode?: boolean;
  batchSelected?: boolean;
  batchSelectedSubs?: Set<string>;
  onToggleBatchSelect?: (filename: string) => void;
}

export const LibraryFileItem: React.FC<LibraryFileItemProps> = ({
  file,
  selectedFile,
  isExpanded,
  subdirFiles,
  hasSubdir,
  onFileClick,
  onToggleExpand,
  onSubdirFileClick,
  onCreateSubdirFile,
  batchDeleteMode = false,
  batchSelected = false,
  batchSelectedSubs = new Set(),
  onToggleBatchSelect,
}) => {
  const { t } = useTranslation();
  const isSelected = selectedFile?.filename === file.filename;

  const handleCreateClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onCreateSubdirFile(file.filename);
  };

  const handleExpandClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onToggleExpand(file.filename);
  };

  const subdirKey = (sub: SubdirFile) => {
    const subdir =
      sub.subdir || file.filename.replace(/\.md$/i, "").toUpperCase();
    return `${subdir}/${sub.filename}`;
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
              {batchDeleteMode && (
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
              {file.filename.replace(/\.md$/i, "")}
              {file.builtin && <span className={styles.builtinTag}>内置</span>}
              <span className={styles.fileItemMetaInline}>
                {formatFileSize(file.size)}
              </span>
            </div>
          </div>
          <div className={styles.fileItemActions}>
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
              <span
                className={styles.expandIcon}
                onClick={handleExpandClick}
                role="button"
                aria-label={isExpanded ? "collapse" : "expand"}
              >
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
                    {batchDeleteMode && (
                      <span
                        className={`${styles.batchCheckbox} ${
                          batchSelectedSubs.has(subdirKey(sub))
                            ? styles.batchChecked
                            : ""
                        }`}
                        onClick={(e) => {
                          e.stopPropagation();
                          onToggleBatchSelect?.(subdirKey(sub));
                        }}
                      />
                    )}
                    {sub.filename.replace(/\.md$/i, "")}
                    <span className={styles.fileItemMetaInline}>
                      {formatFileSize(sub.size)}
                    </span>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
