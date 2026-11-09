/* eslint-disable react-refresh/only-export-components */
import { useState } from "react";
import { useLoaderData } from "react-router-dom";

import { useTranslation } from "react-i18next";
import { usePlugins, type PluginsInitialData } from "./usePlugins";
import { pluginApi } from "../../../api/modules/plugin";
import {
  PluginCard,
  PluginDetailModal,
  InstallPluginModal,
} from "./components";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { EmptyState } from "@/components/EmptyState";
import Skeleton from "@/components/Skeleton";
import styles from "./index.module.less";
import { Btn, Tooltip } from "@/ui";
import { Plus } from "lucide-react";

function isFeatureUnavailableError(err: string | null): boolean {
  if (!err) return false;
  const lower = err.toLowerCase();
  return (
    lower.includes("not found") ||
    lower.includes("404") ||
    lower.includes("does not exist") ||
    lower.includes("unavailable")
  );
}

interface PluginsPageProps {
  initialData?: PluginsInitialData;
}

export async function loader(): Promise<PluginsInitialData> {
  try {
    const plugins = await pluginApi.listPlugins();
    return {
      plugins: Array.isArray(plugins) ? plugins : [],
      error: null,
    };
  } catch (err) {
    return {
      plugins: [],
      error: err instanceof Error ? err.message : "Failed to load plugins",
    };
  }
}

export function RouteComponent() {
  return <PluginsPage initialData={useLoaderData() as PluginsInitialData} />;
}

function PluginsPage({ initialData }: PluginsPageProps) {
  const { t } = useTranslation();
  const { plugins, loading, error, refresh } = usePlugins(initialData);
  const [selectedPluginId, setSelectedPluginId] = useState<string | null>(null);
  const [detailOpen, setDetailOpen] = useState(false);
  const [installOpen, setInstallOpen] = useState(false);

  const handleCardClick = (pluginId: string) => {
    setSelectedPluginId(pluginId);
    setDetailOpen(true);
  };

  return (
    <Page>
      <PageHeader
        current={t("plugins.title")}
        onRefresh={refresh}
        refreshing={loading}
        extra={
          <div style={{ display: "flex", gap: "var(--space-2)" }}>
            <Tooltip title={t("plugins.installPlugin")}>
              <Btn icon={<Plus />} onClick={() => setInstallOpen(true)} />
            </Tooltip>
          </div>
        }
      />

      <PageBody className={styles.content}>
        {loading ? (
          <Skeleton variant="card" count={6} />
        ) : error ? (
          isFeatureUnavailableError(error) ? (
            <EmptyState
              variant="not-available"
              description={t("plugins.notAvailable")}
            />
          ) : (
            <EmptyState variant="error" onRetry={() => refresh()} />
          )
        ) : plugins.length === 0 ? (
          <EmptyState
            description={
              <>
                <span className={styles.emptyTitle}>{t("plugins.empty")}</span>
                <span className={styles.emptyHint}>
                  {t("plugins.emptyHint")}
                </span>
              </>
            }
          />
        ) : (
          <div className={styles.pluginCards}>
            {plugins.map((plugin) => (
              <PluginCard
                key={plugin.id}
                plugin={plugin}
                onClick={() => handleCardClick(plugin.id)}
              />
            ))}
          </div>
        )}
      </PageBody>

      <PluginDetailModal
        pluginId={selectedPluginId}
        open={detailOpen}
        onClose={() => setDetailOpen(false)}
        onUninstall={refresh}
      />

      <InstallPluginModal
        open={installOpen}
        onClose={() => setInstallOpen(false)}
        onSuccess={refresh}
      />
    </Page>
  );
}

export default PluginsPage;
