import { Bot, ChevronDown } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useAgentStore } from "../../stores/agentStore";
import { getAgentDisplayName } from "../../utils/agentDisplayName";
import AgentSwitchPopover from "../AgentSwitchPopover";
import styles from "./index.module.less";

export default function AgentBadge({
  onAgentChange,
  agentId,
}: {
  agentId?: string;
  onAgentChange?: (agentId: string) => void;
}) {
  const { t } = useTranslation();
  const { selectedAgent, agents } = useAgentStore();

  const displayedAgent = agentId ?? selectedAgent;
  const currentAgent = agents?.find((a) => a.id === displayedAgent);
  const label = currentAgent
    ? getAgentDisplayName(currentAgent)
    : displayedAgent;

  return (
    <AgentSwitchPopover placement="bottom-end" onAgentChange={onAgentChange}>
      <button
        type="button"
        className={styles.badge}
        aria-label={t("agent.switchAgent")}
      >
        <Bot size={16} strokeWidth={2} className={styles.icon} />
        <span className={styles.label}>{label}</span>
        <ChevronDown size={14} strokeWidth={2.5} className={styles.chevron} />
      </button>
    </AgentSwitchPopover>
  );
}
