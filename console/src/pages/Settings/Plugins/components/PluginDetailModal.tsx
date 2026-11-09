import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { pluginApi } from "../../../../api/modules/plugin";
import type { PluginDetail } from "../../../../api/types";
import { EmptyState } from "@/components/EmptyState";
import { Trash2 } from "lucide-react";
import { Btn, Modal, Popconfirm, Tag, toast } from "@/ui";

interface PluginDetailModalProps {
  pluginId: string | null;
  open: boolean;
  onClose: () => void;
  onUninstall?: () => void;
}

export default function PluginDetailModal({
  pluginId,
  open,
  onClose,
  onUninstall,
}: PluginDetailModalProps) {
  const { t } = useTranslation();
  const [detail, setDetail] = useState<PluginDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [uninstalling, setUninstalling] = useState(false);

  const handleUninstall = async () => {
    if (!pluginId) return;
    setUninstalling(true);
    try {
      const result = await pluginApi.uninstallPlugin(pluginId);
      toast.success(t("plugins.uninstallSuccess", { id: pluginId }));
      if (result.requires_restart) {
        toast.info(t("plugins.requiresRestart"));
      }
      onClose();
      onUninstall?.();
    } catch (err) {
      const msg =
        err instanceof Error ? err.message : t("plugins.uninstallFailed");
      toast.error(msg);
    } finally {
      setUninstalling(false);
    }
  };

  useEffect(() => {
    if (!open || !pluginId) {
      setDetail(null);
      return;
    }
    setLoading(true);
    pluginApi
      .getPlugin(pluginId)
      .then((data) => setDetail(data))
      .catch((err) => console.error("Failed to load plugin detail:", err))
      .finally(() => setLoading(false));
  }, [open, pluginId]);

  return (
    <Modal
      className="modal-fullscreen-mobile"
      open={open}
      onClose={onClose}
      title={detail?.name ?? t("plugins.details")}
      footer={null}
      width={640}
      destroyOnClose
      loading={loading}
    >
      {detail && (
        <>
          <dl className="rua-descriptions">
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">ID</dt>
              <dd className="rua-descriptions__content">{detail.id}</dd>
            </div>
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">
                {t("plugins.version")}
              </dt>
              <dd className="rua-descriptions__content">v{detail.version}</dd>
            </div>
            {detail.author && (
              <div className="rua-descriptions__item">
                <dt className="rua-descriptions__label">
                  {t("plugins.author")}
                </dt>
                <dd className="rua-descriptions__content">{detail.author}</dd>
              </div>
            )}
            {detail.description && (
              <div className="rua-descriptions__item">
                <dt className="rua-descriptions__label">
                  {t("plugins.description")}
                </dt>
                <dd className="rua-descriptions__content">
                  {detail.description}
                </dd>
              </div>
            )}
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">
                {t("plugins.entryPoint")}
              </dt>
              <dd className="rua-descriptions__content">
                <code>{detail.entry_point}</code>
              </dd>
            </div>
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">
                {t("plugins.sourcePath")}
              </dt>
              <dd className="rua-descriptions__content">
                <code>{detail.source_path}</code>
              </dd>
            </div>
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">
                {t("plugins.minVersion")}
              </dt>
              <dd className="rua-descriptions__content">
                {detail.min_rua_version}
              </dd>
            </div>
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">
                {t("plugins.registeredProviders")}
              </dt>
              <dd className="rua-descriptions__content">
                {detail.providers.length > 0 ? (
                  detail.providers.map((p) => (
                    <Tag key={p} color="primary">
                      {p}
                    </Tag>
                  ))
                ) : (
                  <EmptyState
                    size="inline"
                    description={t("plugins.noProviders")}
                  />
                )}
              </dd>
            </div>
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">
                {t("plugins.startupHooks")}
              </dt>
              <dd className="rua-descriptions__content">
                {detail.startup_hooks.length > 0 ? (
                  detail.startup_hooks.map((h) => (
                    <Tag key={h} color="success">
                      {h}
                    </Tag>
                  ))
                ) : (
                  <EmptyState
                    size="inline"
                    description={t("plugins.noHooks")}
                  />
                )}
              </dd>
            </div>
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">
                {t("plugins.shutdownHooks")}
              </dt>
              <dd className="rua-descriptions__content">
                {detail.shutdown_hooks.length > 0 ? (
                  detail.shutdown_hooks.map((h) => (
                    <Tag key={h} color="warning">
                      {h}
                    </Tag>
                  ))
                ) : (
                  <EmptyState
                    size="inline"
                    description={t("plugins.noHooks")}
                  />
                )}
              </dd>
            </div>
            <div className="rua-descriptions__item">
              <dt className="rua-descriptions__label">
                {t("plugins.controlCommands")}
              </dt>
              <dd className="rua-descriptions__content">
                {detail.commands.length > 0 ? (
                  detail.commands.map((c) => (
                    <Tag key={c} color="primary">
                      /{c}
                    </Tag>
                  ))
                ) : (
                  <EmptyState
                    size="inline"
                    description={t("plugins.noCommands")}
                  />
                )}
              </dd>
            </div>
            {detail.diagnostics.length > 0 && (
              <div className="rua-descriptions__item">
                <dt className="rua-descriptions__label">
                  {t("plugins.diagnostics")}
                </dt>
                <dd className="rua-descriptions__content">
                  {detail.diagnostics.map((d, i) => (
                    <Tag key={i} color="danger">
                      {d}
                    </Tag>
                  ))}
                </dd>
              </div>
            )}
          </dl>

          <div style={{ marginTop: 24, textAlign: "right" }}>
            <Popconfirm
              title={t("plugins.uninstallConfirm")}
              onConfirm={handleUninstall}
              okText={t("common.confirm")}
              cancelText={t("common.cancel")}
            >
              <Btn variant="danger" icon={<Trash2 />} loading={uninstalling}>
                {t("plugins.uninstall")}
              </Btn>
            </Popconfirm>
          </div>
        </>
      )}
    </Modal>
  );
}
