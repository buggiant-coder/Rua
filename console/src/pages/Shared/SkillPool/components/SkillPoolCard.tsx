import { useTranslation } from "react-i18next";
import type { PoolSkillSpec } from "../../../../api/types";
import { RuaCard } from "@/components/RuaCard";
import { getSkillVisual } from "../../../Agent/Skills/components";
import styles from "../index.module.less";
import { Btn, Check, Tag } from "@/ui";

export const poolSkillIconUrl = (skill: PoolSkillSpec): string => {
  const raw = skill.install_meta?.market_meta?.icon_url;
  return typeof raw === "string" ? raw : "";
};
interface SkillPoolCardProps {
  skill: PoolSkillSpec;
  isSelected: boolean;
  batchMode: boolean;
  onToggleSelect: (name: string) => void;
  onEdit: (skill: PoolSkillSpec) => void;
  onBroadcast: (skill: PoolSkillSpec) => void;
  onDelete: (skill: PoolSkillSpec) => void;
}

export function SkillPoolCard({
  skill,
  isSelected,
  batchMode,
  onToggleSelect,
  onEdit,
  onBroadcast,
  onDelete,
}: SkillPoolCardProps) {
  const { t } = useTranslation();

  const handleClick = () => {
    if (batchMode) {
      onToggleSelect(skill.name);
    } else {
      onEdit(skill);
    }
  };

  const handleSelectClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onToggleSelect(skill.name);
  };

  const actions = batchMode
    ? [<Check key="cb" checked={isSelected} onClick={handleSelectClick} />]
    : undefined;

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

  return (
    <RuaCard
      hoverable
      selected={isSelected}
      onClick={handleClick}
      className={styles.skillCard}
      icon={
        <span className={styles.fileIcon}>
          {getSkillVisual(
            skill.name,
            skill.content,
            skill.emoji,
            poolSkillIconUrl(skill),
          )}
        </span>
      }
      title={titleNode}
      actions={actions}
      footer={
        <>
          <Btn
            variant="outline"
            disabled={batchMode}
            onClick={(e) => {
              e.stopPropagation();
              onBroadcast(skill);
            }}
          >
            {t("skillPool.broadcast")}
          </Btn>
          <Btn
            variant="danger"
            disabled={batchMode}
            onClick={(e) => {
              e.stopPropagation();
              onDelete(skill);
            }}
          >
            {t("skillPool.delete")}
          </Btn>
        </>
      }
    >
      {}
      <p className={styles.descriptionText}>{skill.description || "-"}</p>
    </RuaCard>
  );
}
