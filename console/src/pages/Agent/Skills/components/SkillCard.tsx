import React from "react";
import type { SkillSpec } from "../../../../api/types";
import { useTranslation } from "react-i18next";
import { RuaCard } from "@/components/RuaCard";
import styles from "../index.module.less";
import {
  Calendar,
  Code,
  Eye,
  EyeOff,
  FileArchive,
  FileImage,
  FileSpreadsheet,
  FileText,
  Presentation,
} from "lucide-react";
import { Btn, Check, Tag } from "@/ui";
interface SkillCardProps {
  skill: SkillSpec;
  isHover: boolean;
  selected?: boolean;
  onSelect?: (e: React.MouseEvent) => void;
  onClick: () => void;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  onToggleEnabled: (e: React.MouseEvent) => void;
  onDelete?: (e?: React.MouseEvent) => void;
}

const extractSkillEmoji = (content?: string) => {
  if (!content) return "";
  const match = content.match(/"emoji"\s*:\s*"([^"]+)"/);
  return match?.[1] || "";
};

export const skillIconUrl = (
  skill: Pick<SkillSpec, "install_meta">,
): string => {
  const raw = skill.install_meta?.market_meta?.icon_url;
  return typeof raw === "string" ? raw : "";
};

const normalizeSkillIconKey = (value: string) =>
  value
    .trim()
    .toLowerCase()
    .split(/\s+/)[0]
    ?.replace(/[^a-z0-9_-]/g, "") || "";

export const getFileIcon = (filePath: string) => {
  const skillKey = normalizeSkillIconKey(filePath);
  const textSkillIcons = new Set([
    "news",
    "file_reader",
    "browser_visible",
    "guidance",
    "himalaya",
    "dingtalk_channel",
  ]);

  if (textSkillIcons.has(skillKey)) {
    return <FileText style={{ color: "#1890ff" }} />;
  }

  switch (skillKey) {
    case "docx":
      return <FileText style={{ color: "#2B8DFF" }} />;
    case "xlsx":
      return <FileSpreadsheet style={{ color: "#44C161" }} />;
    case "pptx":
      return <Presentation style={{ color: "#FF5B3B" }} />;
    case "pdf":
      return <FileText style={{ color: "#F04B57" }} />;
    case "cron":
      return <Calendar style={{ color: "#13c2c2" }} />;
    default:
      break;
  }

  const extension = filePath.split(".").pop()?.toLowerCase() || "";

  switch (extension) {
    case "txt":
    case "md":
    case "markdown":
      return <FileText style={{ color: "#1890ff" }} />;
    case "zip":
    case "rar":
    case "7z":
    case "tar":
    case "gz":
      return <FileArchive style={{ color: "#fa8c16" }} />;
    case "pdf":
      return <FileText style={{ color: "#F04B57" }} />;
    case "doc":
    case "docx":
      return <FileText style={{ color: "#2B8DFF" }} />;
    case "xls":
    case "xlsx":
      return <FileSpreadsheet style={{ color: "#44C161" }} />;
    case "ppt":
    case "pptx":
      return <Presentation style={{ color: "#FF5B3B" }} />;
    case "jpg":
    case "jpeg":
    case "png":
    case "gif":
    case "svg":
    case "webp":
      return <FileImage style={{ color: "#eb2f96" }} />;
    case "py":
    case "js":
    case "ts":
    case "jsx":
    case "tsx":
    case "java":
    case "cpp":
    case "c":
    case "go":
    case "rs":
    case "rb":
    case "php":
      return <Code style={{ color: "#52c41a" }} />;
    default:
      return <FileText style={{ color: "#1890ff" }} />;
  }
};

export const getSkillVisual = (
  name: string,
  content?: string,
  emoji?: string,
  iconUrl?: string,
) => {
  const resolved = emoji || extractSkillEmoji(content);
  if (resolved) {
    return <span className={styles.skillEmoji}>{resolved}</span>;
  }
  if (iconUrl) {
    return <RemoteSkillIcon url={iconUrl} name={name} />;
  }
  return getFileIcon(name);
};

function RemoteSkillIcon({ url, name }: { url: string; name: string }) {
  const [failed, setFailed] = React.useState(false);
  if (failed) return getFileIcon(name);
  return (
    <img
      className={styles.remoteSkillIcon}
      src={url}
      alt=""
      loading="lazy"
      referrerPolicy="no-referrer"
      onError={() => setFailed(true)}
    />
  );
}

export const SkillCard = React.memo(function SkillCard({
  skill,
  selected,
  onSelect,
  onClick,
  onMouseEnter,
  onMouseLeave,
  onToggleEnabled,
  onDelete,
}: SkillCardProps) {
  const { t } = useTranslation();
  const batchMode = selected !== undefined;

  const handleToggleClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onToggleEnabled(e);
  };

  const handleDeleteClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onDelete?.(e);
  };

  const handleSelectClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onSelect?.(e);
  };

  const handleCardClick = (e: React.MouseEvent) => {
    if (batchMode && onSelect) {
      onSelect(e);
    } else {
      onClick();
    }
  };

  const titleNode = (
    <div className={styles.headInfo}>
      <div className={styles.titleRow}>
        <span className={styles.skillTitle}>{skill.name}</span>
      </div>
      {skill.install_meta?.installed_from && (
        <div className={styles.metaRow}>
          <Tag>{skill.install_meta.installed_from}</Tag>
        </div>
      )}
    </div>
  );

  const actions = [
    <span key="status" className={styles.statusContainer}>
      <span
        className={`${styles.statusDot} ${
          skill.enabled ? styles.enabled : styles.disabled
        }`}
      />
      <span
        className={`${styles.statusText} ${
          skill.enabled ? styles.enabled : styles.disabled
        }`}
      >
        {skill.enabled ? t("common.enabled") : t("common.disabled")}
      </span>
    </span>,
  ];
  if (batchMode) {
    actions.push(
      <Check key="cb" checked={selected} onClick={handleSelectClick} />,
    );
  }

  return (
    <RuaCard
      hoverable
      selected={!!selected}
      onClick={handleCardClick}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={`${styles.skillCard} ${
        skill.enabled ? styles.enabledCard : ""
      }`}
      icon={
        <span className={styles.fileIcon}>
          {getSkillVisual(
            skill.name,
            skill.content,
            skill.emoji,
            skillIconUrl(skill),
          )}
        </span>
      }
      title={titleNode}
      actions={actions}
      footer={
        <>
          <Btn
            variant="outline"
            onClick={handleToggleClick}
            icon={skill.enabled ? <EyeOff /> : <Eye />}
            disabled={batchMode}
          >
            {skill.enabled ? t("common.disable") : t("common.enable")}
          </Btn>
          {onDelete && (
            <Btn
              variant="danger"
              onClick={handleDeleteClick}
              disabled={batchMode}
            >
              {t("common.delete")}
            </Btn>
          )}
        </>
      }
    >
      {}
      <p className={styles.descriptionText}>{skill.description || "-"}</p>
    </RuaCard>
  );
});
