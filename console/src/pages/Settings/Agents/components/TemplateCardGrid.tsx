import { useTranslation } from "react-i18next";
import { LayoutTemplate } from "lucide-react";
import type { AgentTemplate } from "@/api/types/agentTemplates";
import { EmptyState } from "@/components/EmptyState";
import { RuaCard } from "@/components/RuaCard";
import styles from "../index.module.less";
import { Btn, Check, Popconfirm, StackedList, Tag, Tooltip } from "@/ui";
interface TemplateCardGridProps {
  templates: AgentTemplate[];
  loading: boolean;
  onEdit: (template: AgentTemplate) => void;
  onDelete: (templateId: string) => void;

  selectMode?: boolean;
  selectedIds?: Set<string>;
  onSelectToggle?: (id: string) => void;
}

export function TemplateCardGrid({
  templates,
  loading,
  onEdit,
  onDelete,
  selectMode = false,
  selectedIds,
  onSelectToggle,
}: TemplateCardGridProps) {
  const { t } = useTranslation();

  if (!loading && templates.length === 0) {
    return (
      <div className={styles.cardGridEmpty}>
        <EmptyState description={t("template.emptyText")} />
      </div>
    );
  }

  return (
    <StackedList className={styles.cardGrid}>
      {templates.map((tpl) => (
        <RuaCard
          key={tpl.id}
          hoverable
          selected={selectMode && (selectedIds?.has(tpl.id) ?? false)}
          onClick={() => {
            if (selectMode) {
              if (tpl.builtin) return;
              onSelectToggle?.(tpl.id);
            } else {
              onEdit(tpl);
            }
          }}
          icon={
            <div className={styles.agentCardAvatar}>
              <LayoutTemplate size={26} strokeWidth={2} />
            </div>
          }
          title={tpl.name}
          tags={
            tpl.builtin
              ? [
                  <Tag
                    key="builtin"
                    color="primary"
                    className={styles.agentCardTag}
                  >
                    {t("template.builtin")}
                  </Tag>,
                ]
              : []
          }
          actions={
            selectMode
              ? [
                  <Tooltip
                    key="check"
                    title={tpl.builtin ? t("template.builtinNotDeletable") : ""}
                  >
                    <Check
                      checked={selectedIds?.has(tpl.id) ?? false}
                      disabled={tpl.builtin}
                      onChange={() => {
                        if (tpl.builtin) return;
                        onSelectToggle?.(tpl.id);
                      }}
                    />
                  </Tooltip>,
                ]
              : []
          }
          footer={
            !selectMode ? (
              <>
                <Btn
                  variant="outline"
                  onClick={(e) => {
                    e.stopPropagation();
                    onEdit(tpl);
                  }}
                >
                  {t("agent.edit")}
                </Btn>
                <Popconfirm
                  title={t("template.deleteConfirm")}
                  onConfirm={() => onDelete(tpl.id)}
                  okText={t("common.confirm")}
                  cancelText={t("common.cancel")}
                >
                  <Btn variant="danger" disabled={tpl.builtin}>
                    {t("agent.delete")}
                  </Btn>
                </Popconfirm>
              </>
            ) : undefined
          }
        >
          {tpl.description && (
            <div className={styles.agentCardDesc}>{tpl.description}</div>
          )}

          <div className={styles.templatePromptRow}>
            <Tag className={styles.agentCardTag}>
              {tpl.prompt_file_count} {t("template.promptFiles")}
            </Tag>
          </div>
        </RuaCard>
      ))}
    </StackedList>
  );
}
