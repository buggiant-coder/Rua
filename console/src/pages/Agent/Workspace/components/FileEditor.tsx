import React, { useState, useMemo, useRef, useEffect } from "react";
import type { MarkdownFile } from "../../../../api/types";
import { useTranslation } from "react-i18next";
import { stripFrontmatter } from "../../../../utils/markdown";
import { copyText } from "../../../../utils/clipboard";
import { EmptyState } from "@/components/EmptyState";
import { RuaMarkdown } from "@/components/RuaMarkdown";
import styles from "../index.module.less";
import { CloudUpload, Copy, Save, Trash2, Undo2, X } from "lucide-react";
import { Btn, Modal, TextArea, TextInput, Toggle, Tooltip, toast } from "@/ui";

interface FileEditorProps {
  selectedFile: MarkdownFile | null;
  fileContent: string;
  loading: boolean;
  hasChanges: boolean;
  readOnly?: boolean;
  autoRename?: boolean;
  isCreatingNew?: boolean;
  onContentChange: (content: string) => void;
  onSave: () => void;
  onReset: () => void;
  onDelete?: () => void;
  onRename?: (newName: string) => void;
  onAutoRenameConsumed?: () => void;

  onCommitNewName?: (name: string) => void;
  onCancelNewFile?: () => void;
  onSyncToLibrary?: () => void;
  hasSubdir?: boolean;
  isFileEnabled?: boolean;

  onCloseDrawer?: () => void;
}

export const FileEditor: React.FC<FileEditorProps> = ({
  selectedFile,
  fileContent,
  loading,
  hasChanges,
  readOnly = false,
  onContentChange,
  onSave,
  onReset,
  onDelete,
  onRename,
  autoRename,
  isCreatingNew,
  onAutoRenameConsumed,
  onCommitNewName,
  onCancelNewFile,
  onSyncToLibrary,
  hasSubdir,
  isFileEnabled = false,
  onCloseDrawer,
}) => {
  const { t } = useTranslation();

  const [showMarkdown, setShowMarkdown] = useState(!isCreatingNew);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [isRenaming, setIsRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);

  const isMarkdownFile = selectedFile?.filename.endsWith(".md") || false;
  const markdownContent = useMemo(
    () => stripFrontmatter(fileContent || ""),
    [fileContent],
  );

  useEffect(() => {
    setIsRenaming(false);
  }, [selectedFile?.filename]);

  useEffect(() => {
    setShowMarkdown(!isCreatingNew);
  }, [isCreatingNew, selectedFile?.filename]);

  useEffect(() => {
    if (autoRename && selectedFile) {
      startRenaming();
      onAutoRenameConsumed?.();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRename, selectedFile?.filename]);

  useEffect(() => {
    if (isRenaming && renameInputRef.current) {
      renameInputRef.current.focus();
      renameInputRef.current.select();
    }
  }, [isRenaming]);

  const startRenaming = () => {
    if (!selectedFile) return;
    if (readOnly) return;
    if (isFileEnabled) {
      toast.warning(t("workspace.renameEnabledWarning"));
      return;
    }
    const nameWithoutExt = selectedFile.filename.replace(/\.md$/, "");
    setRenameValue(nameWithoutExt);
    setIsRenaming(true);
  };

  const confirmRename = async () => {
    const name = renameValue.trim();
    if (!name || !selectedFile) {
      if (isCreatingNew) {
        setIsRenaming(false);
        onCancelNewFile?.();
      } else {
        setIsRenaming(false);
      }
      return;
    }
    const newFilename = name.endsWith(".md") ? name : `${name}.md`;
    if (isCreatingNew) {
      onCommitNewName?.(newFilename);
      setIsRenaming(false);
    } else {
      setIsRenaming(false);
      if (newFilename === selectedFile.filename) return;
      onRename?.(newFilename);
    }
  };

  const cancelRename = () => {
    setIsRenaming(false);
    if (isCreatingNew) {
      onCancelNewFile?.();
    }
  };

  const copyToClipboard = async () => {
    try {
      const ok = await copyText(fileContent);
      if (ok) {
        toast.success(t("common.copied"));
      } else {
        toast.error(t("common.copyFailed"));
      }
    } catch (err) {
      console.error("Failed to copy text: ", err);
      toast.error(t("common.copyFailed"));
    }
  };

  const handleDeleteConfirm = () => {
    setDeleteConfirmOpen(false);
    onDelete?.();
  };

  return (
    <div className={styles.fileEditor}>
      {selectedFile ? (
        <>
          <div
            className={`${styles.editorHeader} ${
              onCloseDrawer ? styles.editorHeaderDrawer : ""
            }`}
          >
            <div className={styles.editorHeaderTitle}>
              {isRenaming ? (
                <span className={styles.renameInputWrapper}>
                  <TextInput
                    ref={renameInputRef}
                    value={renameValue}
                    onChange={(e) => setRenameValue(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") confirmRename();
                      if (e.key === "Escape") cancelRename();
                    }}
                    onBlur={confirmRename}
                    style={{ width: 200 }}
                  />
                  <span className={styles.renameSuffix}>.md</span>
                </span>
              ) : (
                <div
                  className={styles.fileName}
                  onClick={startRenaming}
                  style={{ cursor: readOnly ? "default" : "pointer" }}
                  title={readOnly ? undefined : t("common.rename")}
                >
                  {selectedFile.filename}
                  {readOnly && (
                    <span className={styles.readonlyPromptBadge}>
                      {t("workspace.readonlyPrompt")}
                    </span>
                  )}
                </div>
              )}
            </div>
            {onCloseDrawer && (
              <Btn
                variant="ghost"
                size="sm"
                icon={<X />}
                onClick={onCloseDrawer}
                className={styles.editorCloseBtn}
              />
            )}
            <div className={styles.buttonGroup}>
              {onSyncToLibrary && !readOnly && (
                <Tooltip
                  title={t("workspace.syncToLibrary")}
                  placement="top"
                  mouseEnterDelay={0.8}
                >
                  <Btn
                    variant="outline"
                    size="sm"
                    onClick={onSyncToLibrary}
                    aria-label={t("workspace.syncToLibrary")}
                    icon={<CloudUpload />}
                  />
                </Tooltip>
              )}
              {onDelete && !readOnly && (
                <Tooltip
                  title={t("common.delete")}
                  placement="top"
                  mouseEnterDelay={0.8}
                >
                  <Btn
                    size="sm"
                    variant="danger"
                    onClick={() => {
                      if (isFileEnabled) {
                        toast.warning(t("workspace.deleteEnabledWarning"));
                        return;
                      }
                      if (fileContent.trim() === "" && !hasSubdir) {
                        onDelete();
                      } else {
                        setDeleteConfirmOpen(true);
                      }
                    }}
                    aria-label={t("common.delete")}
                    icon={<Trash2 />}
                  />
                </Tooltip>
              )}
              {!readOnly && (
                <>
                  <Tooltip
                    title={t("common.reset")}
                    placement="top"
                    mouseEnterDelay={0.8}
                  >
                    <Btn
                      size="sm"
                      onClick={onReset}
                      disabled={!hasChanges}
                      aria-label={t("common.reset")}
                      icon={<Undo2 />}
                    />
                  </Tooltip>
                  <Tooltip
                    title={t("common.save")}
                    placement="top"
                    mouseEnterDelay={0.8}
                  >
                    <Btn
                      size="sm"
                      onClick={onSave}
                      disabled={!hasChanges && !isCreatingNew}
                      loading={loading}
                      aria-label={t("common.save")}
                      icon={<Save />}
                    />
                  </Tooltip>
                </>
              )}
            </div>
          </div>

          <div className={styles.editorContent}>
            {isMarkdownFile && (
              <div className={styles.contentLabel}>
                <div className={styles.markdownToggle}>
                  <span className={styles.toggleLabel}>
                    {t("common.preview")}
                  </span>
                  <Toggle
                    checked={showMarkdown}
                    onChange={(e) => setShowMarkdown(e.target.checked)}
                  />
                </div>
                <Btn
                  icon={<Copy />}
                  variant="outline"
                  onClick={copyToClipboard}
                  aria-label={t("common.copy")}
                  className={styles.copyButton}
                />
              </div>
            )}
            {showMarkdown && isMarkdownFile ? (
              markdownContent.trim() === "" ? (
                <EmptyState
                  variant="empty"
                  description={t("workspace.fileContentEmpty")}
                  className={styles.markdownViewer}
                />
              ) : (
                <RuaMarkdown
                  content={markdownContent}
                  className={styles.markdownViewer}
                />
              )
            ) : (
              <TextArea
                value={fileContent}
                readOnly={readOnly}
                onChange={(e) => {
                  if (!readOnly) onContentChange(e.target.value);
                }}
                className={styles.textarea}
                placeholder={t("workspace.fileContent")}
              />
            )}
          </div>
        </>
      ) : (
        <div className={styles.emptyState}>{t("workspace.selectFile")}</div>
      )}

      <Modal
        title={t("common.delete")}
        open={deleteConfirmOpen}
        onClose={() => setDeleteConfirmOpen(false)}
        footer={
          <>
            <Btn variant="outline" onClick={() => setDeleteConfirmOpen(false)}>
              {t("common.cancel")}
            </Btn>
            <Btn variant="danger" onClick={handleDeleteConfirm}>
              {t("common.delete")}
            </Btn>
          </>
        }
      >
        <p>{t("workspace.deleteConfirm", { name: selectedFile?.filename })}</p>
        {hasSubdir && (
          <p style={{ color: "var(--danger-fg)", marginTop: 8 }}>
            {t("workspace.deleteSubdirWarning")}
          </p>
        )}
      </Modal>
    </div>
  );
};
