import { usePromptLibraryData } from "./components/usePromptLibraryData";
import { LibraryFileListPanel } from "./components/LibraryFileListPanel";
import { FileEditor } from "../../Agent/Workspace/components/FileEditor";
import { BroadcastModal } from "./components/BroadcastModal";
import styles from "./index.module.less";
import { promptLibraryApi } from "../../../api/modules/promptLibrary";
import type { MarkdownFile } from "../../../api/types";
import {
  useEffect,
  useRef,
  useState,
  useCallback,
  useMemo,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { PageHeader } from "@/components/PageHeader";
import { Page } from "@/components/Page";
import { isMobile } from "@/utils/isMobile";
import { saveFile, getDownloadFolderHint } from "@/utils/saveFile";
import { MARKDOWN_OR_ZIP_FILE_ACCEPT } from "@/utils/fileInputAccept";
import { useHistoryDismiss } from "../../../hooks/useHistoryDismiss";
import {
  ArrowUpNarrowWide,
  Check,
  Download,
  MoreVertical,
  Plus,
  RefreshCw,
  Send,
  Trash2,
  Undo2,
  Upload,
  X,
} from "lucide-react";
import {
  Btn,
  Drawer,
  Dropdown,
  Modal,
  TextInput,
  Tooltip,
  toast,
  type MenuProps,
} from "@/ui";

interface PromptLibraryPageProps {
  embedded?: boolean;
  headerExtraContainer?: HTMLElement | null;
}

type SelectedPromptLibraryFile = MarkdownFile & {
  _subdir?: string;
  _parentFilename?: string;
};

export default function PromptLibraryPage({
  embedded = false,
  headerExtraContainer,
}: PromptLibraryPageProps) {
  const { t } = useTranslation();
  const {
    files,
    selectedFile,
    subdirFiles,
    expandedDirs,
    filesWithSubdirs,
    fileContent,
    loading,
    hasChanges,
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
    toggleExpand,
    handleSubdirFileClick,
    handleCreateSubdirFile,
    handleSave,
    handleReset,
    handleToggleExpandAll,
    restoreBuiltin,
  } = usePromptLibraryData();

  const fileInputRef = useRef<HTMLInputElement>(null);
  const [batchDeleteMode, setBatchDeleteMode] = useState(false);
  const [batchSelected, setBatchSelected] = useState<Set<string>>(new Set());
  const [broadcastOpen, setBroadcastOpen] = useState(false);
  const [conflictInfo, setConflictInfo] = useState<{
    file: string;
    agentId: string;
    suggestedName: string;
    resolve: (v: "overwrite" | "rename" | "cancel") => void;
  } | null>(null);
  const isMobileMode = useMemo(() => isMobile(), []);

  const SORT_FIELD_KEY = "rua.promptLibrary.sortField";
  const SORT_ORDER_KEY = "rua.promptLibrary.sortOrder";
  type PromptSortField = "name" | "lastUpdated";
  const [searchQuery, setSearchQuery] = useState("");
  const [sortField, setSortField] = useState<PromptSortField>(() => {
    try {
      const v = localStorage.getItem(SORT_FIELD_KEY);
      if (v === "name" || v === "lastUpdated") return v;
    } catch {}
    return "name";
  });
  const [sortOrder, setSortOrder] = useState<"asc" | "desc">(() => {
    try {
      const v = localStorage.getItem(SORT_ORDER_KEY);
      if (v === "asc" || v === "desc") return v;
    } catch {}
    return "asc";
  });

  useEffect(() => {
    try {
      localStorage.setItem(SORT_FIELD_KEY, sortField);
    } catch {}
  }, [sortField]);
  useEffect(() => {
    try {
      localStorage.setItem(SORT_ORDER_KEY, sortOrder);
    } catch {}
  }, [sortOrder]);

  useEffect(() => {
    const mode =
      sortField === "lastUpdated"
        ? sortOrder === "asc"
          ? "time-asc"
          : "time-desc"
        : sortOrder === "asc"
        ? "name-asc"
        : "name-desc";
    if (mode !== sortMode) setSortMode(mode);
  }, [sortField, sortOrder, sortMode, setSortMode]);

  const displayFiles = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    const filtered = q
      ? files.filter((f) => f.filename.toLowerCase().includes(q))
      : files;
    return [...filtered].sort((a, b) => {
      let cmp: number;
      if (sortField === "lastUpdated") {
        cmp = (a.updated_at || 0) - (b.updated_at || 0);
      } else {
        cmp = a.filename.localeCompare(b.filename);
      }
      return sortOrder === "asc" ? cmp : -cmp;
    });
  }, [files, searchQuery, sortField, sortOrder]);

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
          await promptLibraryApi.deleteSubdirFile(subdir, name);
        } else {
          const hasSubdir = filesWithSubdirs.has(entry);
          await promptLibraryApi.deleteFile(entry, hasSubdir);
        }
      }
      toast.success(t("workspace.deleteSuccess"));
      if (
        selectedFile &&
        (batchSelected.has(selectedFile.filename) ||
          batchSelected.has(
            `${(selectedFile as SelectedPromptLibraryFile)._subdir}/${
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
      if (next.has(filename)) next.delete(filename);
      else next.add(filename);
      return next;
    });
  };

  const handleDownload = async () => {
    try {
      const res = await fetch("/api/prompt-library/download");
      if (!res.ok) throw new Error("Download failed");
      const blob = await res.blob();
      const disposition = res.headers.get("content-disposition") || "";
      const match = disposition.match(/filename="?(.+?)"?$/);
      const filename = match ? match[1] : "prompt_library.zip";
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
    const lower = file.name.toLowerCase();
    if (!lower.endsWith(".md") && !lower.endsWith(".zip")) {
      toast.error(t("promptLibrary.mdOrZipOnly"));
      if (fileInputRef.current) fileInputRef.current.value = "";
      return;
    }
    const maxSize = 100 * 1024 * 1024;
    if (file.size > maxSize) {
      toast.error(
        t("workspace.fileSizeExceeded", {
          limit: 100,
          size: (file.size / (1024 * 1024)).toFixed(2),
        }),
      );
      if (fileInputRef.current) fileInputRef.current.value = "";
      return;
    }
    try {
      const formData = new FormData();
      formData.append("file", file);
      const res = await fetch("/api/prompt-library/upload", {
        method: "POST",
        body: formData,
      });
      const result = await res.json();
      if (result.success) {
        toast.success(t("workspace.uploadSuccess"));
        fetchFiles();
      } else {
        toast.error(t("workspace.uploadFailed") + ": " + result.message);
      }
    } catch (error) {
      console.error("Upload failed:", error);
      toast.error(
        t("workspace.uploadFailed") + ": " + (error as Error).message,
      );
    } finally {
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  };

  const handleRename = async (newName: string) => {
    if (!selectedFile) return;
    const sel = selectedFile as SelectedPromptLibraryFile;
    try {
      if (sel._subdir) {
        const parentFilename = sel._parentFilename ?? sel.filename;
        const stem = newName.replace(/\.md$/i, "").toUpperCase();
        newName = `${stem}.md`;
        await promptLibraryApi.renameSubdirFile(
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
        await promptLibraryApi.renameFile(sel.filename, newName);
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
    const sel = selectedFile as SelectedPromptLibraryFile;
    try {
      if (sel._subdir) {
        const parentFilename = sel._parentFilename ?? sel.filename;
        await promptLibraryApi.deleteSubdirFile(sel._subdir, sel.filename);
        toast.success(t("workspace.deleteSuccess"));
        clearSelection();
        await fetchSubdirFiles(parentFilename);
        await fetchFiles();
      } else {
        const hasSubdir = filesWithSubdirs.has(sel.filename);
        await promptLibraryApi.deleteFile(sel.filename, hasSubdir);
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
      agentId: string,
      suggestedName: string,
    ): Promise<"overwrite" | "rename" | "cancel"> => {
      return new Promise((resolve) => {
        setConflictInfo({ file: filename, agentId, suggestedName, resolve });
      });
    },
    [],
  );

  const handleBroadcast = async (fileNames: string[], agentIds: string[]) => {
    try {
      const result = await promptLibraryApi.broadcast(
        fileNames,
        agentIds,
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
          conflict.agent_id,
          conflict.suggested_name || "",
        );
        if (choice === "cancel") continue;
        try {
          const retry = await promptLibraryApi.broadcast(
            [conflict.file],
            [conflict.agent_id],
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
            agent_id: conflict.agent_id,
            file: conflict.file,
            final_name: null,
            error: String(err),
          });
        }
      }

      if (errors.length > 0) {
        toast.warning(
          `${t("promptLibrary.broadcastPartial")}: ${errors
            .map((e) => e.error)
            .join(", ")}`,
        );
      } else if (successes.length > 0) {
        toast.success(t("promptLibrary.broadcastSuccess"));
      }
      setBroadcastOpen(false);
    } catch (error) {
      console.error("Broadcast failed:", error);
      toast.error(t("promptLibrary.broadcastFailed"));
    }
  };

  const promptMoreMenuItems: MenuProps["items"] = [
    {
      key: "restoreBuiltin",
      icon: <Undo2 />,
      label: t("promptLibrary.restoreBuiltin"),
      onClick: () => {
        Modal.confirm({
          title: t("promptLibrary.restoreBuiltinConfirmTitle"),
          content: t("promptLibrary.restoreBuiltinConfirmContent"),
          okText: t("common.confirm"),
          cancelText: t("common.cancel"),
          onOk: restoreBuiltin,
        });
      },
    },
    {
      key: "broadcast",
      icon: <Send />,
      label: t("promptLibrary.broadcast"),
      onClick: () => setBroadcastOpen(true),
    },
    {
      key: "upload",
      icon: <Download />,
      label: t("common.import"),
      onClick: () => fileInputRef.current?.click(),
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
    },
  ];
  const headerMoreMenuItems: MenuProps["items"] = [
    ...promptMoreMenuItems,
    { type: "divider" },
    {
      key: "refresh",
      icon: <RefreshCw />,
      label: t("common.refreshPage"),
      onClick: fetchFiles,
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
          accept={MARKDOWN_OR_ZIP_FILE_ACCEPT}
        />
        <Tooltip title={t("common.create")}>
          <Btn
            variant="filled"
            onClick={startCreateFile}
            aria-label={t("common.create")}
            icon={<Plus />}
          />
        </Tooltip>
        {batchDeleteMode ? (
          <>
            <Tooltip title={t("common.cancel")}>
              <Btn
                variant="outline"
                onClick={cancelBatchDelete}
                aria-label={t("common.cancel")}
                icon={<X />}
              />
            </Tooltip>
            <Tooltip title={t("common.confirm")}>
              <Btn
                variant="danger"
                onClick={confirmBatchDelete}
                aria-label={t("common.confirm")}
                icon={<Check />}
              />
            </Tooltip>
          </>
        ) : (
          <Dropdown menu={{ items: headerMoreMenuItems }} trigger={["click"]}>
            <Btn
              variant="filled"
              aria-label={t("common.more")}
              icon={<MoreVertical />}
            />
          </Dropdown>
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
          items={[{ title: t("nav.promptLibrary") }]}
          onRefresh={fetchFiles}
          showMoreMenu={!batchDeleteMode}
          moreMenuItems={promptMoreMenuItems}
          extra={
            <div className={styles.workspaceInfo}>
              <div className={styles.actionButtons}>
                <input
                  type="file"
                  ref={fileInputRef}
                  onChange={handleFileUpload}
                  style={{ display: "none" }}
                  accept={MARKDOWN_OR_ZIP_FILE_ACCEPT}
                />
                <Tooltip title={t("common.create")}>
                  <Btn
                    variant="filled"
                    onClick={startCreateFile}
                    aria-label={t("common.create")}
                    icon={<Plus />}
                  />
                </Tooltip>
                {batchDeleteMode ? (
                  <>
                    <Tooltip title={t("common.cancel")}>
                      <Btn
                        variant="outline"
                        onClick={cancelBatchDelete}
                        aria-label={t("common.cancel")}
                        icon={<X />}
                      />
                    </Tooltip>
                    <Tooltip title={t("common.confirm")}>
                      <Btn
                        variant="danger"
                        onClick={confirmBatchDelete}
                        aria-label={t("common.confirm")}
                        icon={<Check />}
                      />
                    </Tooltip>
                  </>
                ) : null}
              </div>
            </div>
          }
        />
      )}

      <div className={styles.filterRow}>
        <div className={styles.filterBar}>
          <TextInput
            className={styles.searchInput}
            placeholder={t("promptLibrary.searchPlaceholder")}
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
          <Dropdown
            menu={{
              items: [
                {
                  key: "name-asc",
                  label: t("promptLibrary.sortByName") + " A→Z",
                },
                {
                  key: "name-desc",
                  label: t("promptLibrary.sortByName") + " Z→A",
                },
                {
                  key: "lastUpdated-desc",
                  label: t("promptLibrary.sortByLastUpdated") + " ↓",
                },
                {
                  key: "lastUpdated-asc",
                  label: t("promptLibrary.sortByLastUpdated") + " ↑",
                },
              ],
              selectedKeys: [`${sortField}-${sortOrder}`],
              onClick: ({ key }: { key: string }) => {
                const [field, order] = key.split("-") as [string, string];
                setSortField(field as "name" | "lastUpdated");
                setSortOrder(order as "asc" | "desc");
              },
            }}
            trigger={["click"]}
          >
            <Btn variant="outline" icon={<ArrowUpNarrowWide />} />
          </Dropdown>
        </div>
      </div>

      <div className={styles.content}>
        <LibraryFileListPanel
          className={isMobileMode ? styles.mobileFullList : undefined}
          files={displayFiles}
          selectedFile={selectedFile}
          subdirFiles={subdirFiles}
          expandedDirs={expandedDirs}
          filesWithSubdirs={filesWithSubdirs}
          sortMode={sortMode}
          onSortModeChange={setSortMode}
          onRefresh={fetchFiles}
          onFileClick={handleFileClick}
          onToggleExpand={toggleExpand}
          onSubdirFileClick={handleSubdirFileClick}
          onCreateSubdirFile={handleCreateSubdirFile}
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
            autoRename={autoRename}
            isCreatingNew={isCreatingNew}
            onContentChange={setFileContent}
            onSave={handleSave}
            onReset={handleReset}
            onDelete={handleDelete}
            onRename={handleRename}
            onAutoRenameConsumed={() => setAutoRename(false)}
            onCommitNewName={commitNewFileName}
            onCancelNewFile={cancelCreateFile}
            hasSubdir={
              selectedFile ? filesWithSubdirs.has(selectedFile.filename) : false
            }
            isFileEnabled={false}
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
              autoRename={autoRename}
              isCreatingNew={isCreatingNew}
              onContentChange={setFileContent}
              onSave={handleSave}
              onReset={handleReset}
              onDelete={() => {
                handleDelete();
                clearSelection();
              }}
              onRename={handleRename}
              onAutoRenameConsumed={() => setAutoRename(false)}
              onCommitNewName={commitNewFileName}
              onCancelNewFile={cancelCreateFile}
              hasSubdir={
                selectedFile
                  ? filesWithSubdirs.has(selectedFile.filename)
                  : false
              }
              isFileEnabled={false}
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

      <BroadcastModal
        open={broadcastOpen}
        files={files}
        initialFiles={selectedFile ? [selectedFile.filename] : []}
        onClose={() => setBroadcastOpen(false)}
        onConfirm={handleBroadcast}
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
