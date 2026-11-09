import { useState, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import type { CSSProperties } from "react";
import { stripFrontmatter } from "../../utils/markdown";
import { copyText } from "../../utils/clipboard";
import { RuaMarkdown } from "../RuaMarkdown";
import styles from "./index.module.less";
import { Copy } from "lucide-react";
import { Btn, TextArea, Toggle, toast } from "@/ui";

interface MarkdownCopyProps {
  content: string;
  showMarkdown?: boolean;
  onShowMarkdownChange?: (show: boolean) => void;
  copyButtonProps?: {
    variant?: "filled" | "outline" | "danger" | "danger-filled";
    size?: "sm" | "md" | "lg";
    style?: CSSProperties;
  };
  markdownViewerProps?: {
    style?: CSSProperties;
    className?: string;
  };
  textareaProps?: {
    rows?: number;
    placeholder?: string;
    disabled?: boolean;
    style?: CSSProperties;
    className?: string;
  };
  showControls?: boolean;
  showCopyButton?: boolean;
  editable?: boolean;
  onContentChange?: (content: string) => void;
}

export function MarkdownCopy({
  content,
  showMarkdown = true,
  onShowMarkdownChange,
  copyButtonProps = {},
  markdownViewerProps = {},
  textareaProps = {},
  showControls = true,
  showCopyButton = true,
  editable = false,
  onContentChange,
}: MarkdownCopyProps) {
  const { t } = useTranslation();
  const [isCopying, setIsCopying] = useState(false);
  const [editContent, setEditContent] = useState(content);
  const [localShowMarkdown, setLocalShowMarkdown] = useState(showMarkdown);
  const markdownContent = useMemo(
    () => stripFrontmatter(content || ""),
    [content],
  );

  useEffect(() => {
    setEditContent(content);
  }, [content]);

  useEffect(() => {
    if (editable && !textareaProps.disabled) {
      setLocalShowMarkdown(false);
    } else {
      setLocalShowMarkdown(showMarkdown);
    }
  }, [editable, textareaProps.disabled, showMarkdown]);

  const copyToClipboard = async () => {
    const contentToCopy =
      localShowMarkdown && !(editable && !textareaProps.disabled)
        ? content
        : editable
        ? editContent
        : content;

    if (!contentToCopy) return;

    setIsCopying(true);
    try {
      const ok = await copyText(contentToCopy);
      if (ok) {
        toast.success(t("common.copied"));
      } else {
        toast.error(t("common.copyFailed"));
      }
    } catch (err) {
      console.error("Failed to copy text: ", err);
      toast.error(t("common.copyFailed"));
    } finally {
      setIsCopying(false);
    }
  };

  const handleContentChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const newContent = e.target.value;
    setEditContent(newContent);
    if (onContentChange) {
      onContentChange(newContent);
    }
  };

  const handleShowMarkdownChange = (show: boolean) => {
    setLocalShowMarkdown(show);
    if (onShowMarkdownChange) {
      onShowMarkdownChange(show);
    }
  };

  const defaultCopyButtonProps = {
    variant: "outline" as const,
    size: "sm" as const,
    ...copyButtonProps,
  };

  const defaultMarkdownViewerProps = {
    style: {
      padding: 16,
      height: "100%",
      overflow: "auto",
      backgroundColor: "var(--surface)",
      borderRadius: 6,
      ...markdownViewerProps.style,
    },
    ...markdownViewerProps,
  };

  const defaultTextareaProps = {
    placeholder: t("common.contentPlaceholder"),
    ...textareaProps,
    style: {
      height: "calc(100vh - 300px)",
      minHeight: 200,
      resize: "none" as const,
      ...textareaProps.style,
    },
  };

  return (
    <div className={styles.markdownCopy}>
      {showControls && (
        <div className={styles.controls}>
          <div>{t("common.content")}</div>
          <div className={styles.controlGroup}>
            <div className={styles.previewToggle}>
              <span className={styles.previewLabel}>{t("common.preview")}</span>
              <Toggle
                checked={localShowMarkdown}
                onChange={(e) => handleShowMarkdownChange(e.target.checked)}
              />
            </div>
            {showCopyButton && (
              <Btn
                icon={<Copy />}
                {...defaultCopyButtonProps}
                onClick={copyToClipboard}
                loading={isCopying}
              />
            )}
          </div>
        </div>
      )}

      {localShowMarkdown ? (
        <div
          className={styles.markdownViewer}
          style={markdownViewerProps.style}
        >
          <RuaMarkdown
            content={markdownContent}
            {...defaultMarkdownViewerProps}
          />
        </div>
      ) : (
        <div className={styles.textareaContainer}>
          <TextArea
            value={editable ? editContent : content}
            onChange={handleContentChange}
            {...defaultTextareaProps}
            className={styles.textarea}
            readOnly={!editable || textareaProps.disabled}
          />
        </div>
      )}
    </div>
  );
}
