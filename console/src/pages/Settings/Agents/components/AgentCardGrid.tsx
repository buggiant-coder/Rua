import { useTranslation } from "react-i18next";
import {
  DndContext,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  verticalListSortingStrategy,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { Bot, GripVertical } from "lucide-react";
import type { AgentSummary } from "../../../../api/types/agents";
import { getAgentDisplayName } from "../../../../utils/agentDisplayName";
import { EmptyState } from "@/components/EmptyState";
import { RuaCard } from "@/components/RuaCard";
import styles from "../index.module.less";
import { Btn, Check, Popconfirm, StackedList, Tag, Tooltip } from "@/ui";
interface AgentCardGridProps {
  agents: AgentSummary[];
  loading: boolean;
  reordering: boolean;
  onEdit: (agent: AgentSummary) => void;
  onDelete: (agentId: string) => void;
  onToggle: (agentId: string, currentEnabled: boolean) => void;
  onReorder: (activeId: string, overId: string) => void;

  selectMode?: boolean;
  selectedIds?: Set<string>;
  onSelectToggle?: (id: string) => void;
}

interface AgentCardProps {
  agent: AgentSummary;
  reordering: boolean;
  loading: boolean;
  onEdit: (agent: AgentSummary) => void;
  onDelete: (agentId: string) => void;
  onToggle: (agentId: string, currentEnabled: boolean) => void;
  selectMode?: boolean;
  selected?: boolean;
  onSelectToggle?: (id: string) => void;
}

function SortableAgentCard({
  agent,
  reordering,
  loading,
  onEdit,
  onDelete,
  onToggle,
  selectMode = false,
  selected = false,
  onSelectToggle,
}: AgentCardProps) {
  const { t } = useTranslation();
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: agent.id });

  const dragDisabled = reordering || loading;
  const dragBindings = dragDisabled ? {} : { ...attributes, ...listeners };

  return (
    <div
      ref={setNodeRef}
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: agent.enabled ? 1 : 0.65,
      }}
    >
      <RuaCard
        hoverable
        selected={isDragging || (selectMode && selected)}
        onClick={() => {
          if (selectMode) {
            if (agent.id === "default") return;
            onSelectToggle?.(agent.id);
          } else {
            onEdit(agent);
          }
        }}
        icon={
          <div className={styles.agentCardAvatar}>
            <Bot size={26} strokeWidth={2} />
          </div>
        }
        title={getAgentDisplayName(agent)}
        tags={
          !agent.enabled
            ? [
                <Tag key="d" color="danger" className={styles.agentCardTag}>
                  {t("agent.disabled")}
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
                    agent.id === "default" ? t("agent.defaultNotDeletable") : ""
                  }
                >
                  <Check
                    checked={selected}
                    disabled={agent.id === "default"}
                    onChange={() => {
                      if (agent.id === "default") return;
                      onSelectToggle?.(agent.id);
                    }}
                  />
                </Tooltip>,
              ]
            : [
                <Tooltip key="drag" title={t("agent.dragHandleTooltip")}>
                  <button
                    type="button"
                    className={styles.agentCardDragHandle}
                    tabIndex={dragDisabled ? -1 : 0}
                    aria-disabled={dragDisabled}
                    onClick={(e) => e.stopPropagation()}
                    {...dragBindings}
                  >
                    <GripVertical size={20} />
                  </button>
                </Tooltip>,
              ]
        }
        footer={
          !selectMode ? (
            <>
              <Btn
                variant="outline"
                onClick={(e) => {
                  e.stopPropagation();
                  onEdit(agent);
                }}
              >
                {t("agent.edit")}
              </Btn>
              <Popconfirm
                title={
                  agent.enabled
                    ? t("agent.disableConfirm")
                    : t("agent.enableConfirm")
                }
                description={
                  agent.enabled
                    ? t("agent.disableConfirmDesc")
                    : t("agent.enableConfirmDesc")
                }
                onConfirm={() => onToggle(agent.id, agent.enabled)}
                okText={t("common.confirm")}
                cancelText={t("common.cancel")}
              >
                <Btn variant="outline">
                  {agent.enabled ? t("agent.disable") : t("agent.enable")}
                </Btn>
              </Popconfirm>
              <Popconfirm
                title={t("agent.deleteConfirm")}
                description={t("agent.deleteConfirmDesc")}
                onConfirm={() => onDelete(agent.id)}
                disabled={agent.id === "default"}
                okText={t("common.confirm")}
                cancelText={t("common.cancel")}
              >
                <Btn variant="danger" disabled={agent.id === "default"}>
                  {t("agent.delete")}
                </Btn>
              </Popconfirm>
            </>
          ) : undefined
        }
      >
        {agent.description && (
          <div className={styles.agentCardDesc}>{agent.description}</div>
        )}
        {agent.workspace_dir && (
          <div className={styles.agentCardMeta} title={agent.workspace_dir}>
            {agent.workspace_dir}
          </div>
        )}
      </RuaCard>
    </div>
  );
}

export function AgentCardGrid({
  agents,
  loading,
  reordering,
  onEdit,
  onDelete,
  onToggle,
  onReorder,
  selectMode = false,
  selectedIds,
  onSelectToggle,
}: AgentCardGridProps) {
  const { t } = useTranslation();
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: 6 },
    }),
  );

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    onReorder(String(active.id), String(over.id));
  };

  if (!loading && agents.length === 0) {
    return (
      <div className={styles.cardGridEmpty}>
        <EmptyState description={t("agent.emptyText")} />
      </div>
    );
  }

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <SortableContext
        items={agents.map((a) => a.id)}
        strategy={verticalListSortingStrategy}
      >
        <StackedList className={styles.cardGrid}>
          {agents.map((agent) => (
            <SortableAgentCard
              key={agent.id}
              agent={agent}
              reordering={reordering}
              loading={loading}
              onEdit={onEdit}
              onDelete={onDelete}
              onToggle={onToggle}
              selectMode={selectMode}
              selected={selectedIds?.has(agent.id) ?? false}
              onSelectToggle={onSelectToggle}
            />
          ))}
        </StackedList>
      </SortableContext>
    </DndContext>
  );
}
