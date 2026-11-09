import { useState } from "react";
import type { ProviderInfo } from "../../../../../api/types";
import { ModelManageModal } from "../modals/ModelManageModal";
import { useTranslation } from "react-i18next";
import { RuaCard } from "@/components/RuaCard";
import styles from "../../index.module.less";
import { providerIcon } from "../providerIcon";
import { Btn } from "@/ui";

interface LocalProviderCardProps {
  provider: ProviderInfo;
  onSaved: () => void;
  isHover: boolean;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
}

export function LocalProviderCard({
  provider,
  onSaved,
  onMouseEnter,
  onMouseLeave,
}: LocalProviderCardProps) {
  const { t } = useTranslation();
  const [modelManageOpen, setModelManageOpen] = useState(false);

  const totalCount = provider.models.length + provider.extra_models.length;
  const statusReady = totalCount > 0;
  const statusLabel = statusReady
    ? t("models.available")
    : t("models.unavailable");

  return (
    <RuaCard
      hoverable
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={styles.providerCard}
      icon={
        <img
          src={providerIcon(provider.id)}
          alt={provider.name}
          className={styles.providerIcon}
        />
      }
      title={<span className={styles.cardName}>{provider.name}</span>}
      tags={[
        <span key="local" className={styles.localTag}>
          {t("models.local")}
        </span>,
      ]}
      actions={[
        <span key="status" className={styles.cardStatusHeader}>
          <span
            className={styles.statusDot}
            style={{
              backgroundColor: statusReady
                ? "var(--success-fg)"
                : "var(--border)",
              boxShadow: statusReady
                ? "0 0 0 2px rgba(82, 196, 26, 0.2)"
                : "none",
            }}
          />
          <span
            className={`${styles.statusText} ${
              statusReady ? styles.enabled : styles.disabled
            }`}
          >
            {statusLabel}
          </span>
        </span>,
      ]}
      footer={
        <Btn
          variant="outline"
          onClick={(e) => {
            e.stopPropagation();
            setModelManageOpen(true);
          }}
        >
          {t("models.models")}
        </Btn>
      }
    >
      {}
      <div className={styles.cardInfo}>
        <div className={styles.infoRow}>
          <span className={styles.infoLabel}>{t("models.localType")}:</span>
          <span className={styles.infoValue}>{t("models.localEmbedded")}</span>
        </div>
        <div className={styles.infoRow}>
          <span className={styles.infoLabel}>{t("models.model")}:</span>
          <span className={styles.infoValue}>
            {totalCount > 0
              ? t("models.modelsCount", { count: totalCount })
              : t("models.localDownloadFirst")}
          </span>
        </div>
      </div>

      <ModelManageModal
        provider={provider}
        open={modelManageOpen}
        onClose={() => setModelManageOpen(false)}
        onSaved={onSaved}
      />
    </RuaCard>
  );
}
