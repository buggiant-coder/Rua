import { useTranslation } from "react-i18next";
import { RuaCard } from "@/components/RuaCard";
import { getChannelIconUrl } from "./channelIcons";
import { getChannelLabel, type ChannelKey } from "./constants";
import styles from "../index.module.less";

interface ChannelCardProps {
  channelKey: ChannelKey;
  config: Record<string, unknown>;
  isHover: boolean;
  onClick: () => void;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
}

export function ChannelCard({
  channelKey,
  config,
  onClick,
  onMouseEnter,
  onMouseLeave,
}: ChannelCardProps) {
  const { t } = useTranslation();
  const enabled = Boolean(config.enabled);

  const label = getChannelLabel(channelKey, t);
  const getConfigString = (key: string) =>
    typeof config[key] === "string" ? config[key] : "";
  const botPrefix = getConfigString("bot_prefix");

  return (
    <RuaCard
      hoverable
      onClick={onClick}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={`${styles.channelCard} ${enabled ? styles.enabled : ""}`}
      icon={
        <div className={styles.channelIcon}>
          <img
            src={getChannelIconUrl(channelKey)}
            alt={channelKey}
            width={32}
            height={32}
          />
        </div>
      }
      title={<span className={styles.cardTitle}>{label}</span>}
      actions={[
        <span key="status" className={styles.statusContainer}>
          <span
            className={`${styles.statusDot} ${
              enabled ? styles.enabled : styles.disabled
            }`}
          />
          <span
            className={`${styles.statusText} ${
              enabled ? styles.enabled : styles.disabled
            }`}
          >
            {enabled ? t("common.enabled") : t("common.disabled")}
          </span>
        </span>,
      ]}
    >
      <div className={styles.cardBottomSection}>
        <div className={styles.cardDescription}>
          {t("channels.botPrefix")}: {botPrefix || t("channels.notSet")}
        </div>
      </div>
    </RuaCard>
  );
}
