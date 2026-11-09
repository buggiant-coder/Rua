import React, { useRef, useState, useEffect, useCallback } from "react";
import type { MarkdownFile, SubdirFile } from "../../../../api/types";
import { LibraryFileItem } from "./LibraryFileItem";
import { useTranslation } from "react-i18next";
import styles from "../index.module.less";
import { Btn, Tooltip } from "@/ui";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
interface LibraryFileListPanelProps {
  files: MarkdownFile[];
  selectedFile: MarkdownFile | null;
  subdirFiles: Record<string, SubdirFile[]>;
  expandedDirs: Set<string>;
  filesWithSubdirs: Set<string>;
  sortMode?: string;
  onSortModeChange?: (
    mode: "name-asc" | "name-desc" | "time-asc" | "time-desc",
  ) => void;
  onRefresh?: () => void;
  onFileClick: (file: MarkdownFile) => void;
  onToggleExpand: (parentFilename: string) => void;
  onSubdirFileClick: (parentFilename: string, file: SubdirFile) => void;
  onCreateSubdirFile: (parentFilename: string) => void;
  onToggleExpandAll: () => void;
  batchDeleteMode: boolean;
  batchSelected: Set<string>;
  onToggleBatchSelect: (filename: string) => void;

  className?: string;
}

export const LibraryFileListPanel: React.FC<LibraryFileListPanelProps> = ({
  files,
  selectedFile,
  subdirFiles,
  expandedDirs,
  filesWithSubdirs,
  onFileClick,
  onToggleExpand,
  onSubdirFileClick,
  onCreateSubdirFile,
  onToggleExpandAll,
  batchDeleteMode,
  batchSelected,
  onToggleBatchSelect,
  className,
}) => {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollMaxH, setScrollMaxH] = useState<number | undefined>(undefined);

  const recalcHeight = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    setScrollMaxH(window.innerHeight - rect.top - 16);
  }, []);

  useEffect(() => {
    recalcHeight();
    window.addEventListener("resize", recalcHeight);
    const observer = new MutationObserver(recalcHeight);
    const resizeObs = new ResizeObserver(recalcHeight);
    const panel = scrollRef.current?.closest(`.${styles.fileListPanel}`);
    if (panel) {
      observer.observe(panel, {
        childList: true,
        subtree: true,
        attributes: true,
      });
      resizeObs.observe(panel);
    }
    return () => {
      window.removeEventListener("resize", recalcHeight);
      observer.disconnect();
      resizeObs.disconnect();
    };
  }, [recalcHeight]);

  return (
    <div className={`${styles.fileListPanel} ${className ?? ""}`.trim()}>
      <div className={styles.panelInner}>
        <div className={styles.headerRow}>
          <div className={styles.headerLine}>
            {t("promptLibrary.fileListDesc")}
          </div>
          <Tooltip
            title={
              expandedDirs.size > 0
                ? t("workspace.collapseAll")
                : t("workspace.expandAll")
            }
            placement="top"
            mouseEnterDelay={0.8}
          >
            <Btn
              variant="outline"
              size="sm"
              onClick={onToggleExpandAll}
              icon={
                expandedDirs.size > 0 ? <PanelLeftClose /> : <PanelLeftOpen />
              }
            />
          </Tooltip>
        </div>

        <div
          ref={scrollRef}
          className={styles.scrollContainer}
          style={scrollMaxH ? { maxHeight: scrollMaxH } : undefined}
        >
          {files.length > 0 ? (
            files.map((file) => (
              <LibraryFileItem
                key={file.filename}
                file={file}
                selectedFile={selectedFile}
                isExpanded={expandedDirs.has(file.filename)}
                subdirFiles={subdirFiles[file.filename] || []}
                hasSubdir={filesWithSubdirs.has(file.filename)}
                onFileClick={onFileClick}
                onToggleExpand={onToggleExpand}
                onSubdirFileClick={onSubdirFileClick}
                onCreateSubdirFile={onCreateSubdirFile}
                batchDeleteMode={batchDeleteMode}
                batchSelected={batchSelected.has(file.filename)}
                batchSelectedSubs={batchSelected}
                onToggleBatchSelect={onToggleBatchSelect}
              />
            ))
          ) : (
            <div className={styles.emptyState}>
              {t("promptLibrary.noFiles")}
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
