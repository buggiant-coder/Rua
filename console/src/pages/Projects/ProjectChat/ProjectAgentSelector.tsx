import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bot, Check, ChevronDown, EyeOff, LoaderCircle } from "lucide-react";

import type { AgentSummary } from "@/api/types/agents";
import { Modal, Popover } from "@/ui";
import { getAgentDisplayName } from "@/utils/agentDisplayName";

import styles from "./index.module.less";

type ProjectAgentSelectorProps = {
  agents: AgentSummary[];
  selectedAgentId: string;
  loading: boolean;
  switching: boolean;
  switchable: boolean;
  confirmSelection: boolean;
  switchBlockedReason?: string;
  onSelect: (agentId: string) => Promise<void>;
};

export default function ProjectAgentSelector({
  agents,
  selectedAgentId,
  loading,
  switching,
  switchable,
  confirmSelection,
  switchBlockedReason,
  onSelect,
}: ProjectAgentSelectorProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const selected = agents.find((agent) => agent.id === selectedAgentId);
  const selectedName = selected
    ? getAgentDisplayName(selected)
    : loading
    ? t("projects.chat.agentFallback")
    : t("projects.chat.unavailableAgent");
  const sortedAgents = useMemo(
    () =>
      [...agents].sort((a, b) => {
        if (a.id === selectedAgentId) return -1;
        if (b.id === selectedAgentId) return 1;
        if (a.enabled !== b.enabled) return a.enabled ? -1 : 1;
        return getAgentDisplayName(a).localeCompare(getAgentDisplayName(b));
      }),
    [agents, selectedAgentId],
  );

  const choose = async (agent: AgentSummary) => {
    if (!agent.enabled || agent.id === selectedAgentId || switching) return;
    if (!switchable) return;
    setOpen(false);
    if (!confirmSelection) {
      await onSelect(agent.id);
      return;
    }
    const confirmed = await Modal.confirm({
      title: t("projects.chat.switchAgent"),
      content: t("projects.chat.switchConfirm", {
        agent: getAgentDisplayName(agent),
      }),
      okText: t("projects.chat.switchAgent"),
      cancelText: t("common.cancel"),
    });
    if (confirmed) await onSelect(agent.id);
  };

  const content = (
    <div className={styles.agentPopover} aria-label={t("projects.chat.agent")}>
      <div className={styles.agentPopoverTitle}>{t("projects.chat.agent")}</div>
      {!loading && !selected && selectedAgentId ? (
        <div
          className={`${styles.agentOption} ${styles.agentOptionUnavailable}`}
        >
          <span className={styles.agentAvatar}>
            <EyeOff size={14} />
          </span>
          <span className={styles.agentOptionCopy}>
            <strong>{selectedAgentId}</strong>
            <small>{t("projects.chat.unavailableSelection")}</small>
          </span>
          <Check size={15} />
        </div>
      ) : null}
      {sortedAgents.map((agent) => {
        const active = agent.id === selectedAgentId;
        const disabled =
          !agent.enabled || switching || (!active && !switchable);
        const detail = active
          ? t("projects.chat.currentSelection")
          : !agent.enabled
          ? t("common.disabled")
          : !switchable
          ? switchBlockedReason || t("projects.chat.switchUnavailable")
          : t("projects.chat.switchStableBoundary");
        return (
          <button
            key={agent.id}
            type="button"
            className={`${styles.agentOption} ${
              active ? styles.agentOptionActive : ""
            }`}
            disabled={disabled}
            onClick={() => void choose(agent)}
          >
            <span className={styles.agentAvatar}>
              {agentInitials(getAgentDisplayName(agent))}
            </span>
            <span className={styles.agentOptionCopy}>
              <strong>{getAgentDisplayName(agent)}</strong>
              <small>{detail}</small>
            </span>
            {active ? <Check size={15} /> : null}
          </button>
        );
      })}
      {!loading && agents.length === 0 ? (
        <div className={styles.agentEmpty}>
          {t("projects.chat.noAvailableAgents")}
        </div>
      ) : null}
    </div>
  );

  return (
    <Popover
      content={content}
      trigger="click"
      placement="bottom-start"
      open={open}
      onOpenChange={setOpen}
      overlayClassName={styles.agentPopoverOverlay}
    >
      <button
        type="button"
        className={styles.agentControl}
        aria-label={t("projects.chat.agent")}
        title={selectedName}
        data-switch-blocked-reason={switchBlockedReason}
      >
        <span className={styles.agentControlIcon}>
          {loading || switching ? (
            <LoaderCircle className={styles.agentSpinner} size={16} />
          ) : (
            <Bot size={16} strokeWidth={2} />
          )}
        </span>
        <span
          className={`${styles.agentName} ${
            loading && !selected ? styles.agentNameLoading : ""
          }`}
        >
          {selectedName}
        </span>
        <ChevronDown
          size={14}
          strokeWidth={2.5}
          className={styles.agentControlChevron}
        />
      </button>
    </Popover>
  );
}

function agentInitials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "AG";
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return `${words[0][0]}${words[words.length - 1][0]}`.toUpperCase();
}
