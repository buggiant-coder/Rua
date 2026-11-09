import { useTranslation } from "react-i18next";
import type { PluginSummary } from "../../../../api/types";
import styles from "../index.module.less";

interface PluginCardProps {
  plugin: PluginSummary;
  onClick: () => void;
}

export default function PluginCard({ plugin, onClick }: PluginCardProps) {
  const { t } = useTranslation();

  const stats = [
    plugin.provider_count > 0 &&
      t("plugins.providers", { count: plugin.provider_count }),
    plugin.hook_count > 0 && t("plugins.hooks", { count: plugin.hook_count }),
    plugin.command_count > 0 &&
      t("plugins.commands", { count: plugin.command_count }),
  ].filter(Boolean);

  return (
    <div className={styles.pluginCard} onClick={onClick}>
      <div className={styles.cardStatusHeader}>
        <span
          className={styles.statusDot}
          style={{
            backgroundColor: plugin.enabled
              ? "var(--success-fg)"
              : "var(--text-3)",
          }}
        />
        <span
          className={`${styles.statusText} ${
            plugin.enabled ? styles.enabled : styles.disabled
          }`}
        >
          {plugin.enabled ? t("common.enabled") : t("common.disabled")}
        </span>
      </div>

      <div className={styles.cardTitleRow}>
        <span className={styles.cardName}>{plugin.name}</span>
      </div>

      <div className={styles.pluginMeta}>
        <span className={styles.pluginVersion}>v{plugin.version}</span>
        {plugin.author && (
          <>
            <span className={styles.pluginMetaSep}>&middot;</span>
            <span className={styles.pluginAuthor}>{plugin.author}</span>
          </>
        )}
      </div>

      {plugin.description && (
        <div className={styles.pluginDesc}>{plugin.description}</div>
      )}

      {stats.length > 0 && (
        <div className={styles.pluginStats}>{stats.join(" \u00B7 ")}</div>
      )}
    </div>
  );
}
