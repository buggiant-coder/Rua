import type { ReactNode } from "react";
import { Check } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useAgentStore } from "../../stores/agentStore";
import { getAgentDisplayName } from "../../utils/agentDisplayName";
import styles from "./index.module.less";
import Popover from "@/ui/Popover";
import toast from "@/ui/Toast";
interface AgentSwitchPopoverProps {
  children: ReactNode;

  placement?: React.ComponentProps<typeof Popover>["placement"];
  onAgentChange?: (agentId: string) => void;
}

export default function AgentSwitchPopover({
  children,
  placement = "bottom-start",
  onAgentChange,
}: AgentSwitchPopoverProps) {
  const { t } = useTranslation();
  const {
    selectedAgent,
    agents = [],
    setSelectedAgent,
    ensureCatalogLoaded,
  } = useAgentStore();
  const handleOpenChange = (open: boolean) => {
    if (!open || !ensureCatalogLoaded) return;
    void ensureCatalogLoaded().catch((err) => {
      console.error("Failed to load agents:", err);
      toast.error(t("agent.loadFailed"));
    });
  };

  const handleChange = (agentId: string) => {
    const target = agents?.find((a) => a.id === agentId);
    if (target && !target.enabled) {
      toast.warning(t("agent.cannotSwitchToDisabled"));
      return;
    }
    setSelectedAgent?.(agentId);
    onAgentChange?.(agentId);
    toast.success(t("agent.switchSuccess"));
  };

  const content = (
    <div className={styles.menu} aria-label={t("agent.switchAgent")}>
      <div className={styles.title}>{t("agent.switchAgent")}</div>
      {[...agents]
        .sort((a, b) => (a.enabled === b.enabled ? 0 : a.enabled ? -1 : 1))
        .map((agent) => {
          const isActive = agent.id === selectedAgent;
          const isDisabled = !agent.enabled;
          const displayName = getAgentDisplayName(agent);
          return (
            <button
              type="button"
              key={agent.id}
              className={`${styles.item} ${isActive ? styles.itemActive : ""} ${
                isDisabled ? styles.itemDisabled : ""
              }`}
              disabled={isDisabled}
              onClick={() => handleChange(agent.id)}
            >
              <span className={styles.avatar} aria-hidden="true">
                {displayName.trim().charAt(0).toUpperCase() || "A"}
              </span>
              <span className={styles.itemCopy}>
                <span className={styles.itemName}>{displayName}</span>
                <span className={styles.itemMeta}>
                  {isActive
                    ? t("agent.currentWorkspace")
                    : isDisabled
                    ? t("common.disabled")
                    : t("agent.switchAgent")}
                </span>
              </span>
              {isActive && (
                <Check
                  size={14}
                  strokeWidth={2.2}
                  className={styles.itemActiveIndicator}
                />
              )}
            </button>
          );
        })}
    </div>
  );

  return (
    <Popover
      content={content}
      trigger="click"
      placement={placement}
      overlayClassName={styles.popover}
      onOpenChange={handleOpenChange}
    >
      {children}
    </Popover>
  );
}
