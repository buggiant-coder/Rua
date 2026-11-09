import { useTranslation } from "react-i18next";
import { Puzzle } from "lucide-react";
import type { SubAgentTemplate } from "../../../../api/types/subagentTemplates";
import type { AgentSummary } from "../../../../api/types/agents";
import { EmptyState } from "@/components/EmptyState";
import { RuaCard } from "@/components/RuaCard";
import styles from "../index.module.less";
import { Btn, Check, Popconfirm, StackedList, Tag, Tooltip } from "@/ui";
interface SubAgentCardGridProps {
  templates: SubAgentTemplate[];
  agents: AgentSummary[];
  loading: boolean;
  onEdit: (template: SubAgentTemplate) => void;
  onDelete: (templateId: string) => void;

  selectMode?: boolean;
  selectedIds?: Set<string>;
  onSelectToggle?: (id: string) => void;
}

export function SubAgentCardGrid({
  templates,
  agents,
  loading,
  onEdit,
  onDelete,
  selectMode = false,
  selectedIds,
  onSelectToggle,
}: SubAgentCardGridProps) {
  const { t } = useTranslation();

  const agentNameMap: Record<string, string> = {};
  for (const agent of agents) {
    agentNameMap[agent.id] = agent.name || agent.id;
  }

  if (!loading && templates.length === 0) {
    return (
      <div className={styles.cardGridEmpty}>
        <EmptyState description={t("subAgent.emptyText")} />
      </div>
    );
  }

  return (
    <StackedList className={styles.cardGrid}>
      {templates.map((tpl) => {
        const visibleTo = tpl.visible_to || [];
        const visibleAll =
          visibleTo.length === 0 ||
          (agents.length > 0 &&
            visibleTo.length >= agents.length &&
            agents.every((a) => visibleTo.includes(a.id)));

        return (
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
                <Puzzle size={26} strokeWidth={2} />
              </div>
            }
            title={tpl.name}
            tags={
              tpl.builtin
                ? [
                    <Tag key="builtin" className={styles.agentCardTag}>
                      {t("subAgent.builtin")}
                    </Tag>,
                  ]
                : []
            }
            actions={
              selectMode
                ? [
                    <Tooltip
                      key="check"
                      title={
                        tpl.builtin ? t("subAgent.builtinNotDeletable") : ""
                      }
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
                    title={t("subAgent.deleteConfirm")}
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

            <div className={styles.subAgentVisibleRow}>
              <span className={styles.subAgentVisibleLabel}>
                {t("subAgent.visibleTo")}:
              </span>
              {visibleAll ? (
                <Tag color="success" className={styles.agentCardTag}>
                  {t("subAgent.visibleToAll")}
                </Tag>
              ) : (
                visibleTo.map((agentId) => (
                  <Tag key={agentId} className={styles.agentCardTag}>
                    {agentNameMap[agentId] || agentId}
                  </Tag>
                ))
              )}
            </div>
          </RuaCard>
        );
      })}
    </StackedList>
  );
}
