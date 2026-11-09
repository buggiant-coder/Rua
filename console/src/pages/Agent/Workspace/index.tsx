import { useAgentsData, FileListPanel, FileEditor } from "./components";
import { SyncToLibraryModal } from "./components/SyncToLibraryModal";
import { LoadFromLibraryModal } from "./components/LoadFromLibraryModal";
import styles from "./index.module.less";
import { agentsApi } from "../../../api/modules/agents";
import { promptLibraryApi } from "../../../api/modules/promptLibrary";
import type { MarkdownFile } from "../../../api/types";
import { useAgentStore } from "../../../stores/agentStore";
import { useRef, useState, useCallback, useMemo, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page } from "@/components/Page";
import { isMobile } from "@/utils/isMobile";
import { saveFile, getDownloadFolderHint } from "@/utils/saveFile";
import { ZIP_FILE_ACCEPT } from "@/utils/fileInputAccept";
import { useHistoryDismiss } from "../../../hooks/useHistoryDismiss";
import {
  Check,
  CloudDownload,
  CloudUpload,
  Download,
  MoreVertical,
  Plus,
  RefreshCw,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import {
  Btn,
  Drawer,
  Dropdown,
  Modal,
  Tooltip,
  toast,
  type MenuProps,
} from "@/ui";

interface WorkspacePageProps {
  embedded?: boolean;
  headerExtraContainer?: HTMLElement | null;
}

type SelectedWorkspaceFile = MarkdownFile & {
  _subdir?: string;
  _parentFilename?: string;
};

export default function WorkspacePage({
  embedded = false,
  headerExtraContainer,
}: WorkspacePageProps) {
  const { t } = useTranslation();
  const {
    files,
    selectedFile,
    subdirFiles,
    expandedDirs,
    filesWithSubdirs,
    fileContent,
    loading,
    workspacePath,
    hasChanges,
    enabledFiles,
    displayedLoadedFiles,
    autoRename,
    isCreatingNew,
    sortMode,
    setFileContent,
    setSortMode,
    setAutoRename,
    startCreateFile,
    commitNewFileName,
    cancelCreateFile,
    clearSelection,
    fetchFiles,
    fetchSubdirFiles,
    handleFileClick,
    handleSubdirFileClick,
    handleCreateSubdirFile,
    handleSave,
    handleReset,
    handleToggleFileEnabled,
    handleReorderFiles,
    handleToggleExpandAll,
  } = useAgentsData();

  const { selectedAgent } = useAgentStore();
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [batchDeleteMode, setBatchDeleteMode] = useState(false);
  const [batchSelected, setBatchSelected] = useState<Set<string>>(new Set());
  const [syncModalOpen, setSyncModalOpen] = useState(false);
  const [loadModalOpen, setLoadModalOpen] = useState(false);
  const [conflictInfo, setConflictInfo] = useState<{
    file: string;
    suggestedName: string;
    resolve: (v: "overwrite" | "rename" | "cancel") => void;
  } | null>(null);
  const isMobileMode = useMemo(() => isMobile(), []);
  const selectedFileReadOnly = Boolean(
    selectedFile && (selectedFile as { _readonly?: boolean })._readonly,
  );

  const editorDrawerOpen = isMobileMode && (!!selectedFile || !!isCreatingNew);
  const handleEditorDrawerClose = useCallback(() => {
    if (isCreatingNew) {
      cancelCreateFile();
    } else {
      clearSelection();
    }
  }, [isCreatingNew, cancelCreateFile, clearSelection]);
  useHistoryDismiss(editorDrawerOpen, handleEditorDrawerClose);

  const enterBatchDelete = () => {
    setBatchDeleteMode(true);
    setBatchSelected(new Set());
  };

  const cancelBatchDelete = () => {
    setBatchDeleteMode(false);
    setBatchSelected(new Set());
  };

  const confirmBatchDelete = async () => {
    if (batchSelected.size === 0) {
      cancelBatchDelete();
      return;
    }
    try {
      for (const entry of batchSelected) {
        if (entry.includes("/")) {
          const [subdir, name] = entry.split("/", 2);
          await agentsApi.deleteAgentSubdirFile(selectedAgent, subdir, name);
        } else {
          const hasSubdir = filesWithSubdirs.has(entry);
          await agentsApi.deleteAgentFile(selectedAgent, entry, hasSubdir);
        }
      }
      toast.success(t("workspace.deleteSuccess"));
      if (
        selectedFile &&
        (batchSelected.has(selectedFile.filename) ||
          batchSelected.has(
            `${(selectedFile as SelectedWorkspaceFile)._subdir}/${
              selectedFile.filename
            }`,
          ))
      ) {
        clearSelection();
      }
      await fetchFiles();
    } catch (error) {
      console.error("Batch delete failed:", error);
      toast.error(t("workspace.deleteFailed"));
    }
    setBatchDeleteMode(false);
    setBatchSelected(new Set());
  };

  const toggleBatchSelect = (filename: string) => {
    setBatchSelected((prev) => {
      const next = new Set(prev);
      if (next.has(filename)) {
        next.delete(filename);
      } else {
        next.add(filename);
      }
      return next;
    });
  };

  const handleDownload = async () => {
    try {
      const { blob, filename } = await agentsApi.downloadAgentWorkspace(
        selectedAgent,
      );
      const result = await saveFile(blob, filename);
      if (result.ok) {
        const hint = result.usedPicker ? "" : getDownloadFolderHint();
        toast.success(
          hint
            ? `${t("workspace.downloadSuccess")} (${hint}${filename})`
            : t("workspace.downloadSuccess"),
        );
      } else {
        toast.error(t("workspace.downloadFailed"));
      }
    } catch (error) {
      console.error("Download failed:", error);
      toast.error(
        t("workspace.downloadFailed") + ": " + (error as Error).message,
      );
    }
  };

  const handleFileUpload = async (
    event: React.ChangeEvent<HTMLInputElement>,
  ) => {
    const file = event.target.files?.[0];
    if (!file) return;

    if (!file.name.toLowerCase().endsWith(".zip")) {
      toast.error(t("workspace.zipOnly"));
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
      return;
    }

    const maxSizeMb = 100;
    const maxSize = maxSizeMb * 1024 * 1024;
    if (file.size > maxSize) {
      toast.error(
        t("workspace.fileSizeExceeded", {
          limit: maxSizeMb,
          size: (file.size / (1024 * 1024)).toFixed(2),
        }),
      );
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
      return;
    }

    try {
      const result = await agentsApi.uploadAgentWorkspace(selectedAgent, file);
      if (result.success) {
        toast.success(t("workspace.uploadSuccess"));
      } else {
        toast.error(t("workspace.uploadFailed") + ": " + result.message);
      }
    } catch (error) {
      console.error("Upload failed:", error);
      toast.error(
        t("workspace.uploadFailed") + ": " + (error as Error).message,
      );
    } finally {
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
    }
  };

  const handleRename = async (newName: string) => {
    if (!selectedFile) return;
    const sel = selectedFile as SelectedWorkspaceFile;
    try {
      if (sel._subdir) {
        const parentFilename = sel._parentFilename ?? sel.filename;

        const stem = newName.replace(/\.md$/i, "").toUpperCase();
        newName = `${stem}.md`;
        await agentsApi.renameAgentSubdirFile(
          selectedAgent,
          sel._subdir,
          sel.filename,
          newName,
        );
        toast.success(t("workspace.renameSuccess"));
        await fetchSubdirFiles(parentFilename);

        handleSubdirFileClick(parentFilename, {
          filename: newName,
          path: "",
          size: 0,
          created_time: "",
          modified_time: "",
          builtin: false,
          subdir: sel._subdir,
          updated_at: 0,
        });
      } else {
        await agentsApi.renameAgentFile(selectedAgent, sel.filename, newName);
        toast.success(t("workspace.renameSuccess"));
        await fetchFiles();
        handleFileClick({
          filename: newName,
          path: "",
          size: 0,
          created_time: "",
          modified_time: "",
          builtin: false,
          updated_at: 0,
        });
      }
    } catch (error) {
      console.error("Rename failed:", error);
      toast.error(t("workspace.renameFailed"));
    }
  };

  const handleDelete = async () => {
    if (!selectedFile) return;
    const sel = selectedFile as SelectedWorkspaceFile;

    if (sel._subdir) {
      const subdir =
        sel._subdir || sel._parentFilename?.replace(/\.md$/i, "").toUpperCase();
      const enabledKey = `${subdir}/${sel.filename}`;
      if (enabledFiles.includes(enabledKey)) {
        toast.warning(t("workspace.deleteEnabledWarning"));
        return;
      }
    } else if (enabledFiles.includes(sel.filename)) {
      toast.warning(t("workspace.deleteEnabledWarning"));
      return;
    }

    try {
      if (sel._subdir) {
        const parentFilename = sel._parentFilename ?? sel.filename;
        await agentsApi.deleteAgentSubdirFile(
          selectedAgent,
          sel._subdir,
          sel.filename,
        );
        toast.success(t("workspace.deleteSuccess"));
        clearSelection();
        await fetchSubdirFiles(parentFilename);

        await fetchFiles();
      } else {
        const hasSubdir = filesWithSubdirs.has(sel.filename);
        await agentsApi.deleteAgentFile(selectedAgent, sel.filename, hasSubdir);
        toast.success(t("workspace.deleteSuccess"));
        clearSelection();
        fetchFiles();
      }
    } catch (error) {
      console.error("Delete failed:", error);
      toast.error(t("workspace.deleteFailed"));
    }
  };

  const showConflictModal = useCallback(
    (
      filename: string,
      suggestedName: string,
    ): Promise<"overwrite" | "rename" | "cancel"> => {
      return new Promise((resolve) => {
        setConflictInfo({ file: filename, suggestedName, resolve });
      });
    },
    [],
  );

  const syncFilesWithConflictResolution = useCallback(
    async (fileNames: string[]) => {
      const result = await promptLibraryApi.sync(
        selectedAgent,
        fileNames,
        "ask",
      );
      const conflicts = result.results.filter((r) => r.conflict);
      const errors = result.results.filter((r) => r.error);
      const successes = result.results.filter(
        (r) => r.final_name && !r.conflict && !r.error,
      );

      for (const conflict of conflicts) {
        const choice = await showConflictModal(
          conflict.file,
          conflict.suggested_name || "",
        );
        if (choice === "cancel") continue;
        try {
          const retry = await promptLibraryApi.sync(
            selectedAgent,
            [conflict.file],
            choice,
          );
          const retryErrors = retry.results.filter((r) => r.error);
          if (retryErrors.length > 0) {
            errors.push(...retryErrors);
          } else {
            successes.push(...retry.results.filter((r) => r.final_name));
          }
        } catch (err) {
          errors.push({
            file: conflict.file,
            final_name: null,
            error: String(err),
          });
        }
      }

      if (errors.length > 0) {
        toast.error(t("workspace.syncFailed") + ": " + errors[0].error);
      } else if (successes.length > 0) {
        toast.success(t("workspace.syncSuccess"));
      }
    },
    [selectedAgent, showConflictModal, t],
  );

  const handleSyncToLibrary = async () => {
    if (!selectedFile) return;
    try {
      await syncFilesWithConflictResolution([selectedFile.filename]);
    } catch (error) {
      console.error("Sync to library failed:", error);
      toast.error(t("workspace.syncFailed"));
    }
  };

  const handleBatchSync = async (fileNames: string[]) => {
    try {
      await syncFilesWithConflictResolution(fileNames);
      setSyncModalOpen(false);
    } catch (error) {
      console.error("Batch sync failed:", error);
      toast.error(t("workspace.syncFailed"));
    }
  };

  const handleLoadFromLibrary = async (fileNames: string[]) => {
    try {
      const result = await promptLibraryApi.broadcast(
        fileNames,
        [selectedAgent],
        "ask",
      );
      const conflicts = result.results.filter((r) => r.conflict);
      const errors = result.results.filter((r) => r.error);
      const successes = result.results.filter(
        (r) => r.final_name && !r.conflict && !r.error,
      );

      for (const conflict of conflicts) {
        const choice = await showConflictModal(
          conflict.file,
          conflict.suggested_name || "",
        );
        if (choice === "cancel") continue;
        try {
          const retry = await promptLibraryApi.broadcast(
            [conflict.file],
            [selectedAgent],
            choice,
          );
          const retryErrors = retry.results.filter((r) => r.error);
          if (retryErrors.length > 0) {
            errors.push(...retryErrors);
          } else {
            successes.push(...retry.results.filter((r) => r.final_name));
          }
        } catch (err) {
          errors.push({
            agent_id: selectedAgent,
            file: conflict.file,
            final_name: null,
            error: String(err),
          });
        }
      }

      if (errors.length > 0) {
        toast.error(
          t("workspace.loadFromLibraryFailed") + ": " + errors[0].error,
        );
      } else if (successes.length > 0) {
        toast.success(t("workspace.loadFromLibrarySuccess"));
        await fetchFiles();
      }
      setLoadModalOpen(false);
    } catch (error) {
      console.error("Load from library failed:", error);
      toast.error(t("workspace.loadFromLibraryFailed"));
    }
  };

  const handleUploadClick = () => {
    fileInputRef.current?.click();
  };

  const workspaceMoreMenuItems: MenuProps["items"] = [
    {
      key: "loadFromLibrary",
      icon: <CloudDownload />,
      label: t("workspace.loadFromLibrary"),
      onClick: () => setLoadModalOpen(true),
    },
    {
      key: "syncToLibrary",
      icon: <CloudUpload />,
      label: t("workspace.syncToLibrary"),
      onClick: () => setSyncModalOpen(true),
    },
    {
      key: "upload",
      icon: <Download />,
      label: t("common.import"),
      onClick: handleUploadClick,
    },
    {
      key: "export",
      icon: <Upload />,
      label: t("common.export"),
      onClick: handleDownload,
    },
    {
      key: "batchDelete",
      icon: <Trash2 />,
      label: t("workspace.batchDelete"),
      onClick: enterBatchDelete,
      danger: true,
    },
  ];
  const headerMoreMenuItems: MenuProps["items"] = [
    ...workspaceMoreMenuItems,
    { type: "divider" },
    {
      key: "refresh",
      icon: <RefreshCw size={14} />,
      label: t("common.refreshPage"),
      onClick: fetchFiles,
      disabled: loading,
    },
  ];

  const headerExtra: ReactNode = (
    <div className={styles.workspaceInfo}>
      <div className={styles.actionButtons}>
        <input
          type="file"
          ref={fileInputRef}
          onChange={handleFileUpload}
          style={{ display: "none" }}
          accept={ZIP_FILE_ACCEPT}
          title={t("workspace.zipUploadHint")}
        />
        {batchDeleteMode ? (
          <>
            <span style={{ color: "var(--text-2)", fontSize: 13 }}>
              {t("common.selectedN", { count: batchSelected.size })}
            </span>
            <Btn variant="filled" icon={<X />} onClick={cancelBatchDelete} />
            <Btn
              variant="danger-filled"
              onClick={confirmBatchDelete}
              icon={<Check />}
            />
          </>
        ) : (
          <>
            <Tooltip title={t("common.create")}>
              <Btn variant="filled" onClick={startCreateFile} icon={<Plus />} />
            </Tooltip>
            <Dropdown menu={{ items: headerMoreMenuItems }} trigger={["click"]}>
              <Btn variant="filled" icon={<MoreVertical />} />
            </Dropdown>
          </>
        )}
      </div>
    </div>
  );

  return (
    <Page className={styles.workspacePage}>
      {embedded && headerExtraContainer
        ? createPortal(headerExtra, headerExtraContainer)
        : null}
      {!embedded && (
        <PageHeader
          agentScoped
          items={[{ title: t("workspace.title") }]}
          onRefresh={fetchFiles}
          refreshing={loading}
          showMoreMenu={!batchDeleteMode}
          moreMenuItems={workspaceMoreMenuItems}
          extra={
            <div className={styles.workspaceInfo}>
              <div className={styles.actionButtons}>
                <input
                  type="file"
                  ref={fileInputRef}
                  onChange={handleFileUpload}
                  style={{ display: "none" }}
                  accept={ZIP_FILE_ACCEPT}
                  title={t("workspace.zipUploadHint")}
                />
                {batchDeleteMode ? (
                  <>
                    <span style={{ color: "var(--text-2)", fontSize: 13 }}>
                      {t("common.selectedN", { count: batchSelected.size })}
                    </span>
                    <Btn
                      variant="filled"
                      icon={<X />}
                      onClick={cancelBatchDelete}
                    />
                    <Btn
                      variant="danger-filled"
                      onClick={confirmBatchDelete}
                      icon={<Check />}
                    />
                  </>
                ) : (
                  <Tooltip title={t("common.create")}>
                    <Btn
                      variant="filled"
                      onClick={startCreateFile}
                      icon={<Plus />}
                    />
                  </Tooltip>
                )}
              </div>
            </div>
          }
        />
      )}

      <div className={styles.content}>
        <FileListPanel
          className={isMobileMode ? styles.mobileFullList : undefined}
          files={files}
          selectedFile={selectedFile}
          subdirFiles={subdirFiles}
          expandedDirs={expandedDirs}
          filesWithSubdirs={filesWithSubdirs}
          workspacePath={workspacePath}
          enabledFiles={enabledFiles}
          displayedLoadedFiles={displayedLoadedFiles}
          sortMode={sortMode}
          onSortModeChange={setSortMode}
          onRefresh={fetchFiles}
          onFileClick={handleFileClick}
          onSubdirFileClick={handleSubdirFileClick}
          onCreateSubdirFile={handleCreateSubdirFile}
          onToggleEnabled={handleToggleFileEnabled}
          onReorder={handleReorderFiles}
          onToggleExpandAll={handleToggleExpandAll}
          batchDeleteMode={batchDeleteMode}
          batchSelected={batchSelected}
          onToggleBatchSelect={toggleBatchSelect}
        />

        {!isMobileMode && (
          <FileEditor
            selectedFile={selectedFile}
            fileContent={fileContent}
            loading={loading}
            hasChanges={hasChanges}
            readOnly={selectedFileReadOnly}
            autoRename={autoRename}
            isCreatingNew={isCreatingNew}
            onContentChange={setFileContent}
            onSave={handleSave}
            onReset={handleReset}
            onDelete={selectedFileReadOnly ? undefined : handleDelete}
            onRename={selectedFileReadOnly ? undefined : handleRename}
            onAutoRenameConsumed={() => setAutoRename(false)}
            onCommitNewName={commitNewFileName}
            onCancelNewFile={cancelCreateFile}
            onSyncToLibrary={
              selectedFile &&
              !selectedFileReadOnly &&
              !(selectedFile as SelectedWorkspaceFile)._subdir
                ? handleSyncToLibrary
                : undefined
            }
            hasSubdir={
              selectedFile ? filesWithSubdirs.has(selectedFile.filename) : false
            }
            isFileEnabled={
              selectedFile
                ? enabledFiles.includes(selectedFile.filename) ||
                  enabledFiles.includes(
                    `${(selectedFile as SelectedWorkspaceFile)._subdir}/${
                      selectedFile.filename
                    }`,
                  )
                : false
            }
          />
        )}
      </div>

      {isMobileMode && (
        <Drawer
          className={styles.mobileEditorDrawer}
          placement="right"
          width="100%"
          open={!!selectedFile || !!isCreatingNew}
          onClose={() => {
            if (isCreatingNew) {
              cancelCreateFile();
            } else {
              clearSelection();
            }
          }}
          title={null}
          closable={false}
          destroyOnClose
        >
          <div className={styles.mobileEditorDrawerBody}>
            <FileEditor
              selectedFile={selectedFile}
              fileContent={fileContent}
              loading={loading}
              hasChanges={hasChanges}
              readOnly={selectedFileReadOnly}
              autoRename={autoRename}
              isCreatingNew={isCreatingNew}
              onContentChange={setFileContent}
              onSave={handleSave}
              onReset={handleReset}
              onDelete={
                selectedFileReadOnly
                  ? undefined
                  : async () => {
                      await handleDelete();
                      clearSelection();
                    }
              }
              onRename={selectedFileReadOnly ? undefined : handleRename}
              onAutoRenameConsumed={() => setAutoRename(false)}
              onCommitNewName={commitNewFileName}
              onCancelNewFile={cancelCreateFile}
              onSyncToLibrary={
                selectedFile &&
                !selectedFileReadOnly &&
                !(selectedFile as SelectedWorkspaceFile)._subdir
                  ? handleSyncToLibrary
                  : undefined
              }
              hasSubdir={
                selectedFile
                  ? filesWithSubdirs.has(selectedFile.filename)
                  : false
              }
              isFileEnabled={
                selectedFile
                  ? enabledFiles.includes(selectedFile.filename) ||
                    enabledFiles.includes(
                      `${(selectedFile as SelectedWorkspaceFile)._subdir}/${
                        selectedFile.filename
                      }`,
                    )
                  : false
              }
              onCloseDrawer={() => {
                if (isCreatingNew) {
                  cancelCreateFile();
                } else {
                  clearSelection();
                }
              }}
            />
          </div>
        </Drawer>
      )}

      <SyncToLibraryModal
        open={syncModalOpen}
        files={files}
        onClose={() => setSyncModalOpen(false)}
        onConfirm={handleBatchSync}
      />

      <LoadFromLibraryModal
        open={loadModalOpen}
        onClose={() => setLoadModalOpen(false)}
        onConfirm={handleLoadFromLibrary}
      />

      <Modal
        open={!!conflictInfo}
        title={t("promptLibrary.conflictTitle")}
        onClose={() => {
          conflictInfo?.resolve("cancel");
          setConflictInfo(null);
        }}
        footer={
          <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
            <Btn
              onClick={() => {
                conflictInfo?.resolve("cancel");
                setConflictInfo(null);
              }}
            >
              {t("common.cancel")}
            </Btn>
            <Btn
              onClick={() => {
                conflictInfo?.resolve("rename");
                setConflictInfo(null);
              }}
            >
              {t("promptLibrary.renameCreate")} ({conflictInfo?.suggestedName})
            </Btn>
            <Btn
              variant="danger"
              onClick={() => {
                conflictInfo?.resolve("overwrite");
                setConflictInfo(null);
              }}
            >
              {t("promptLibrary.overwrite")}
            </Btn>
          </div>
        }
      >
        <p>
          {t("promptLibrary.conflictMessage", {
            name: conflictInfo?.file || "",
          })}
        </p>
      </Modal>
    </Page>
  );
}
