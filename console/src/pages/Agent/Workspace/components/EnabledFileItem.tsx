import React from "react";

import { Toggle, Tooltip } from "@/ui";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useTranslation } from "react-i18next";
import { formatFileSize } from "./utils";
import styles from "../index.module.less";
import { GripVertical } from "lucide-react";

interface EnabledFileItemProps {
  entry: string;
  label?: string;
  fileSize: number | null;
  enabled: boolean;
  readOnly?: boolean;
  onToggle: (entry: string) => void;
  onClick: (entry: string) => void;
}

export const EnabledFileItem: React.FC<EnabledFileItemProps> = ({
  entry,
  label,
  fileSize,
  enabled,
  readOnly = false,
  onToggle,
  onClick,
}) => {
  const { t } = useTranslation();

  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: entry });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
    position: "relative",
    zIndex: isDragging ? 1 : undefined,
  };

  const handleToggle = (e: React.ChangeEvent<HTMLInputElement>) => {
    e.stopPropagation();
    onToggle(entry);
  };

  return (
    <div
      ref={setNodeRef}
      style={{ ...style, cursor: "pointer" }}
      className={`${styles.loadedPromptItem} ${
        readOnly ? styles.loadedPromptItemReadonly : ""
      }`}
      onClick={() => onClick(entry)}
    >
      <div className={styles.dragHandle} {...attributes} {...listeners}>
        <GripVertical />
      </div>
      <div className={styles.loadedPromptName}>
        {label ?? entry.replace(/\.md$/i, "")}
        {fileSize !== null && (
          <span className={styles.fileItemMetaInline}>
            {formatFileSize(fileSize)}
          </span>
        )}
        {readOnly && (
          <span className={styles.readonlyPromptBadge}>
            {t("workspace.readonlyPrompt")}
          </span>
        )}
      </div>
      {!readOnly && (
        <span onClick={(e) => e.stopPropagation()}>
          <Tooltip
            title={t("workspace.systemPromptToggleTooltip")}
            placement="right"
            mouseEnterDelay={0.8}
          >
            <Toggle checked={enabled} onChange={handleToggle} />
          </Tooltip>
        </span>
      )}
    </div>
  );
};
