import { memo, useCallback, useState } from "react";

import { RuaCard } from "@/components/RuaCard";
import { useTranslation } from "react-i18next";
import type { MarketResult } from "../../../../api/modules/market";
import type { InstallTarget } from "../useHubInstall";
import { SkillIcon, sourceLabel } from "./SkillIcon";
import { TargetToggle } from "./TargetToggle";
import styles from "./ResultCard.module.less";
import { Btn, Tag, Tooltip } from "@/ui";

interface ResultCardProps {
  item: MarketResult;
  target: InstallTarget;
  isInstalled?: boolean;
  onTargetChange: (next: InstallTarget) => void;
  onInstall: () => void;
  onOpenDetail: () => void;
}

const CURSOR_STYLE = { cursor: "pointer" } as const;

export const ResultCard = memo(function ResultCard({
  item,
  target,
  isInstalled,
  onTargetChange,
  onInstall,
  onOpenDetail,
}: ResultCardProps) {
  const { t } = useTranslation();
  const [hover, setHover] = useState(false);

  const showFooter = useCallback(() => setHover(true), []);
  const hideFooter = useCallback(() => setHover(false), []);
  const stopPropagation = useCallback(
    (e: React.MouseEvent | React.KeyboardEvent) => e.stopPropagation(),
    [],
  );

  return (
    <RuaCard
      hoverable
      className={styles.skillCard}
      onClick={onOpenDetail}
      onMouseEnter={showFooter}
      onMouseLeave={hideFooter}
      style={CURSOR_STYLE}
    >
      <div className={styles.cardTopRow}>
        <SkillIcon url={item.icon_url} alt={item.name} source={item.source} />
        <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
          {isInstalled && <Tag color="success">{t("market.installed")}</Tag>}
          <Tag>{sourceLabel(item.source)}</Tag>
        </div>
      </div>

      <div className={styles.titleRow}>
        <Tooltip title={item.name}>
          <h3 className={styles.skillTitle}>{item.name}</h3>
        </Tooltip>
      </div>

      <p className={styles.descriptionText}>
        {item.description || t("market.noDescription")}
      </p>

      {hover && (
        <div
          className={styles.cardFooter}
          onClick={stopPropagation}
          onKeyDown={stopPropagation}
        >
          <TargetToggle target={target} onChange={onTargetChange} size="sm" />
          <Btn
            variant={isInstalled ? "outline" : "filled"}
            size="sm"
            onClick={onInstall}
            className={styles.installButton}
          >
            {isInstalled ? t("market.installed") : t("market.install")}
          </Btn>
        </div>
      )}
    </RuaCard>
  );
});
