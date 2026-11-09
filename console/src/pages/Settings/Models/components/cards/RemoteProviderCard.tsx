import { useEffect, useRef, useState } from "react";
import type { ProviderInfo, ActiveModelsInfo } from "../../../../../api/types";
import { ProviderConfigModal } from "../modals/ProviderConfigModal";
import { ModelManageModal } from "../modals/ModelManageModal";
import api from "../../../../../api";
import { useTranslation } from "react-i18next";
import { RuaCard } from "@/components/RuaCard";
import {
  isProviderConfigured,
  providerHasModels,
} from "@/utils/providerReadiness";
import styles from "../../index.module.less";
import { providerIcon } from "../providerIcon";
import { Btn, Modal, toast } from "@/ui";
import { openOAuthAuthorization } from "@/desktop/oauth";
interface RemoteProviderCardProps {
  provider: ProviderInfo;
  activeModels: ActiveModelsInfo | null;
  onSaved: () => void;
  isHover: boolean;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
}

export function RemoteProviderCard({
  provider,
  activeModels,
  onSaved,
  onMouseEnter,
  onMouseLeave,
}: RemoteProviderCardProps) {
  const { t } = useTranslation();
  const [modalOpen, setModalOpen] = useState(false);
  const [modelManageOpen, setModelManageOpen] = useState(false);
  const [oauthBusy, setOauthBusy] = useState(false);
  const oauthPollRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (oauthPollRef.current !== null) {
        window.clearInterval(oauthPollRef.current);
      }
    };
  }, []);

  const handleDeleteProvider = (e: React.MouseEvent) => {
    e.stopPropagation();
    Modal.confirm({
      title: t("models.deleteProvider"),
      content: t("models.deleteProviderConfirm", { name: provider.name }),
      okText: t("common.delete"),
      danger: true,
      cancelText: t("models.cancel"),
      onOk: async () => {
        try {
          await api.deleteProvider(provider.id);
          toast.success(t("models.providerDeleted", { name: provider.name }));
          onSaved();
        } catch (error) {
          const errMsg =
            error instanceof Error
              ? error.message
              : t("models.providerDeleteFailed");
          toast.error(errMsg);
        }
      },
    });
  };

  const totalCount = provider.models.length + provider.extra_models.length;
  const needsOAuth =
    !!provider.supports_oauth && !provider.oauth_connected && !provider.api_key;

  const isConfigured = isProviderConfigured(provider);
  const hasModels = providerHasModels(provider);

  const configLabel = isConfigured
    ? t("models.available")
    : t("models.unavailable");
  const configDotColor = isConfigured
    ? "rgba(20, 184, 166, 1)"
    : "var(--border)";
  const configDotShadow = isConfigured
    ? "0 0 0 2px rgba(82, 196, 26, 0.2)"
    : "none";
  const configClass = isConfigured ? styles.enabled : styles.disabled;

  const modelLabel = hasModels
    ? t("models.hasModels")
    : t("models.noModelsStatus");
  const modelDotColor = hasModels ? "rgba(20, 184, 166, 1)" : "var(--border)";
  const modelDotShadow = hasModels
    ? "0 0 0 2px rgba(82, 196, 26, 0.2)"
    : "none";
  const modelClass = hasModels ? styles.enabled : styles.disabled;

  const handleAuthorizeOAuth = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (oauthBusy) return;
    setOauthBusy(true);
    try {
      const response = await api.startProviderOAuth(provider.id);
      const launch = await openOAuthAuthorization(
        response.authorize_url,
        "provider-oauth",
        "width=720,height=820",
      );
      const popup = launch.popup;
      const finish = async (success: boolean, message?: string) => {
        if (oauthPollRef.current !== null) {
          window.clearInterval(oauthPollRef.current);
          oauthPollRef.current = null;
        }
        window.removeEventListener("message", handleMessage);
        setOauthBusy(false);
        if (success) {
          toast.success(t("models.oauthConnected", { name: provider.name }));
          onSaved();
        } else if (message) {
          toast.error(message);
        }
      };
      const handleMessage = (event: MessageEvent) => {
        const data = event.data;
        if (data && data.type !== "provider-oauth") return;
        void finish(data?.status !== "error");
      };
      window.addEventListener("message", handleMessage);
      if (launch.mode === "popup" && !popup) {
        await finish(false, t("models.oauthPopupBlocked"));
        return;
      }
      oauthPollRef.current = window.setInterval(async () => {
        if (popup?.closed) {
          await finish(false);
          onSaved();
          return;
        }
        try {
          const status = await api.getProviderOAuthStatus(
            provider.id,
            response.state,
          );
          if (status.status === "completed") {
            await finish(true);
          } else if (status.status === "failed") {
            await finish(false, status.error || t("models.oauthFailed"));
          }
        } catch {}
      }, 1200);
    } catch (error) {
      setOauthBusy(false);
      toast.error(
        error instanceof Error ? error.message : t("models.oauthStartFailed"),
      );
    }
  };

  return (
    <RuaCard
      hoverable
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={styles.providerCard}
      title={<span className={styles.cardName}>{provider.name}</span>}
      icon={
        <img
          src={providerIcon(provider.id)}
          alt={provider.name}
          className={styles.providerIcon}
        />
      }
      footer={
        <>
          <Btn
            variant="outline"
            onClick={(e) => {
              e.stopPropagation();
              setModelManageOpen(true);
            }}
          >
            {t("models.models")}
          </Btn>
          <Btn
            variant="outline"
            onClick={(e) => {
              e.stopPropagation();
              setModalOpen(true);
            }}
          >
            {t("models.settings")}
          </Btn>
          <Btn variant="danger" onClick={handleDeleteProvider}>
            {t("common.delete")}
          </Btn>
        </>
      }
    >
      {}
      <div className={styles.cardStatusFloat}>
        <div className={styles.cardStatusHeader}>
          <span
            className={styles.statusDot}
            style={{
              backgroundColor: configDotColor,
              boxShadow: configDotShadow,
            }}
          />
          <span className={`${styles.statusText} ${configClass}`}>
            {configLabel}
          </span>
        </div>
        <div className={styles.cardStatusHeader}>
          <span
            className={styles.statusDot}
            style={{
              backgroundColor: modelDotColor,
              boxShadow: modelDotShadow,
            }}
          />
          <span className={`${styles.statusText} ${modelClass}`}>
            {modelLabel}
          </span>
        </div>
      </div>

      {}
      <div className={styles.cardInfo}>
        <div className={styles.infoRow}>
          <span className={styles.infoLabel}>{t("models.cardIdLabel")}</span>
          <span className={styles.infoValue} title={provider.id}>
            {provider.id}
          </span>
        </div>
        {(provider.provider_group_name || provider.provider_variant) && (
          <div className={styles.infoRow}>
            <span className={styles.infoLabel}>
              {t("models.providerFamily")}
            </span>
            <span
              className={styles.infoValue}
              title={[provider.provider_group_name, provider.provider_variant]
                .filter(Boolean)
                .join(" / ")}
            >
              {[provider.provider_group_name, provider.provider_variant]
                .filter(Boolean)
                .join(" / ")}
            </span>
          </div>
        )}
        <div className={styles.infoRow}>
          <span className={styles.infoLabel}>
            {t("models.cardBaseUrlLabel")}
          </span>
          {provider.base_url ? (
            <span className={styles.infoValue} title={provider.base_url}>
              {provider.base_url}
            </span>
          ) : (
            <span className={styles.infoEmpty}>{t("models.notSet")}</span>
          )}
        </div>
        <div className={styles.infoRow}>
          <span className={styles.infoLabel}>
            {t("models.cardApiKeyLabel")}
          </span>
          {provider.api_key ? (
            <span className={styles.infoValue}>{provider.api_key}</span>
          ) : needsOAuth ? (
            <span className={styles.infoValueInline}>
              <span className={styles.infoEmpty}>
                {t("models.oauthRequired")}
              </span>
              <Btn size="sm" onClick={handleAuthorizeOAuth} loading={oauthBusy}>
                {t("models.oauthAuthorize")}
              </Btn>
            </span>
          ) : (
            <span className={styles.infoEmpty}>{t("models.notSet")}</span>
          )}
        </div>
        <div className={styles.infoRow}>
          <span className={styles.infoLabel}>{t("models.cardModelLabel")}</span>
          <span className={styles.infoValue}>
            {totalCount > 0
              ? t("models.modelsCount", { count: totalCount })
              : t("models.noModels")}
          </span>
        </div>
      </div>

      <ProviderConfigModal
        provider={provider}
        activeModels={activeModels}
        open={modalOpen}
        onClose={() => setModalOpen(false)}
        onSaved={onSaved}
      />
      <ModelManageModal
        provider={provider}
        open={modelManageOpen}
        onClose={() => setModelManageOpen(false)}
        onSaved={onSaved}
      />
    </RuaCard>
  );
}
