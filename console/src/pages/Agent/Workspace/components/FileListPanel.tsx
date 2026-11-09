import React, { useRef, useState, useEffect, useCallback } from "react";
import {
  DndContext,
  closestCenter,
  PointerSensor,
  useSensor,
  useSensors,
  DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  verticalListSortingStrategy,
  arrayMove,
} from "@dnd-kit/sortable";
import type { MarkdownFile, SubdirFile } from "../../../../api/types";
import { FileItem } from "./FileItem";
import { EnabledFileItem } from "./EnabledFileItem";
import { useTranslation } from "react-i18next";
import styles from "../index.module.less";
import { Btn, Dropdown, Tooltip } from "@/ui";
import {
  ArrowUpNarrowWide,
  PanelLeftClose,
  PanelLeftOpen,
  RefreshCw,
} from "lucide-react";
interface FileListPanelProps {
  files: MarkdownFile[];
  selectedFile: MarkdownFile | null;
  subdirFiles: Record<string, SubdirFile[]>;
  expandedDirs: Set<string>;
  filesWithSubdirs: Set<string>;
  workspacePath: string | null;
  enabledFiles: string[];
  displayedLoadedFiles: string[];
  sortMode: string;
  onSortModeChange: (
    mode: "name-asc" | "name-desc" | "time-asc" | "time-desc",
  ) => void;
  onRefresh: () => void;
  onFileClick: (file: MarkdownFile) => void;
  onSubdirFileClick: (parentFilename: string, file: SubdirFile) => void;
  onCreateSubdirFile: (parentFilename: string) => void;
  onToggleEnabled: (filename: string) => void;
  onReorder: (newOrder: string[]) => void;
  onToggleExpandAll: () => void;
  batchDeleteMode: boolean;
  batchSelected: Set<string>;
  onToggleBatchSelect: (filename: string) => void;
  className?: string;
}

export const FileListPanel: React.FC<FileListPanelProps> = ({
  files,
  selectedFile,
  subdirFiles,
  expandedDirs,
  filesWithSubdirs,
  enabledFiles,
  displayedLoadedFiles,
  sortMode,
  onSortModeChange,
  onRefresh,
  onFileClick,
  onSubdirFileClick,
  onCreateSubdirFile,
  onToggleEnabled,
  onReorder,
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

  useEffect(() => {
    recalcHeight();
  }, [displayedLoadedFiles.length, recalcHeight]);

  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: {
        distance: 5,
      },
    }),
  );

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    const oldIndex = displayedLoadedFiles.indexOf(active.id as string);
    const newIndex = displayedLoadedFiles.indexOf(over.id as string);
    if (oldIndex === -1 || newIndex === -1) return;

    const newOrder = arrayMove(displayedLoadedFiles, oldIndex, newIndex);
    onReorder(newOrder);
  };

  const getFileSize = (entry: string): number | null => {
    if (entry.includes("/")) {
      const [dir, name] = entry.split("/", 2);
      for (const [parentFilename, subs] of Object.entries(subdirFiles)) {
        const parentSubdir = parentFilename.replace(/\.md$/i, "").toUpperCase();
        if (parentSubdir === dir) {
          const sub = subs.find((s) => s.filename === name);
          return sub ? sub.size : null;
        }
      }
      return null;
    }
    const file = files.find((f) => f.filename === entry);
    return file ? file.size : null;
  };

  const handleLoadedItemClick = (entry: string) => {
    if (entry.includes("/")) {
      const [dir, name] = entry.split("/", 2);

      for (const f of files) {
        if (f.filename.replace(/\.md$/i, "").toUpperCase() === dir) {
          const sub = subdirFiles[f.filename]?.find((s) => s.filename === name);
          if (sub) {
            onSubdirFileClick(f.filename, sub);
          }
          return;
        }
      }
    } else {
      const file = files.find((f) => f.filename === entry);
      if (file) {
        onFileClick(file);
      }
    }
  };

  const sortMenuItems = [
    { key: "name-asc", label: t("workspace.sortNameAsc") },
    { key: "name-desc", label: t("workspace.sortNameDesc") },
    { key: "time-desc", label: t("workspace.sortTimeDesc") },
    { key: "time-asc", label: t("workspace.sortTimeAsc") },
  ];

  return (
    <div className={`${styles.fileListPanel} ${className ?? ""}`.trim()}>
      <div className={styles.panelInner}>
        {}
        <div className={styles.headerRow}>
          <span className={styles.sectionTitle}>
            {t("workspace.loadedPrompts")}
          </span>
          <Btn
            variant="outline"
            size="sm"
            onClick={onRefresh}
            icon={<RefreshCw />}
          />
        </div>

        {}
        <div className={styles.loadedPromptList}>
          {displayedLoadedFiles.length > 0 ? (
            <DndContext
              sensors={sensors}
              collisionDetection={closestCenter}
              onDragEnd={handleDragEnd}
            >
              <SortableContext
                items={displayedLoadedFiles}
                strategy={verticalListSortingStrategy}
              >
                {displayedLoadedFiles.map((entry) => (
                  <EnabledFileItem
                    key={entry}
                    entry={entry}
                    fileSize={getFileSize(entry)}
                    enabled={enabledFiles.includes(entry)}
                    onToggle={onToggleEnabled}
                    onClick={handleLoadedItemClick}
                  />
                ))}
              </SortableContext>
            </DndContext>
          ) : (
            <div className={styles.emptyStateSmall}>
              {t("workspace.noEnabledFiles")}
            </div>
          )}
        </div>

        {}
        <div className={styles.headerRow} style={{ marginTop: 5 }}>
          <div className={styles.headerLine}>{t("workspace.promptDesc")}</div>
          <div style={{ display: "flex", gap: 4, flexShrink: 0 }}>
            <Dropdown
              menu={{
                items: sortMenuItems,
                selectedKeys: [sortMode],
                onClick: ({ key }: { key: string }) =>
                  onSortModeChange(
                    key as "name-asc" | "name-desc" | "time-asc" | "time-desc",
                  ),
              }}
              trigger={["click"]}
            >
              <Tooltip
                title={t("workspace.sort")}
                placement="top"
                mouseEnterDelay={0.8}
              >
                <Btn variant="outline" size="sm" icon={<ArrowUpNarrowWide />} />
              </Tooltip>
            </Dropdown>
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
            <Btn
              variant="outline"
              id="prompt_list_reload"
              size="sm"
              onClick={onRefresh}
              icon={<RefreshCw />}
            />
          </div>
        </div>

        {}
        <div
          ref={scrollRef}
          className={styles.scrollContainer}
          style={scrollMaxH ? { maxHeight: scrollMaxH } : undefined}
        >
          {files.length > 0 ? (
            files.map((file) => {
              const isEnabled = enabledFiles.includes(file.filename);
              return (
                <FileItem
                  key={file.filename}
                  file={file}
                  selectedFile={selectedFile}
                  isExpanded={expandedDirs.has(file.filename)}
                  subdirFiles={subdirFiles[file.filename] || []}
                  hasSubdir={filesWithSubdirs.has(file.filename)}
                  enabled={isEnabled}
                  enabledFiles={enabledFiles}
                  onFileClick={onFileClick}
                  onSubdirFileClick={onSubdirFileClick}
                  onCreateSubdirFile={onCreateSubdirFile}
                  onToggleEnabled={onToggleEnabled}
                  batchDeleteMode={batchDeleteMode}
                  batchSelected={batchSelected.has(file.filename)}
                  batchSelectedSubs={batchSelected}
                  onToggleBatchSelect={onToggleBatchSelect}
                />
              );
            })
          ) : (
            <div className={styles.emptyState}>{t("workspace.noFiles")}</div>
          )}
        </div>
      </div>
    </div>
  );
};
