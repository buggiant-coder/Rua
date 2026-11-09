import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import styles from "./index.module.less";
import { Btn } from "@/ui";
import {
  AlertCircle,
  Inbox,
  Lock,
  Plug,
  RefreshCw,
  Search,
} from "lucide-react";
export type EmptyStateVariant =
  | "empty"
  | "error"
  | "not-available"
  | "no-results"
  | "forbidden";

export interface EmptyStateProps {
  variant?: EmptyStateVariant;

  size?: "default" | "inline";

  icon?: ReactNode;

  description?: ReactNode;

  action?: ReactNode;

  onRetry?: () => void;
  className?: string;
  style?: React.CSSProperties;
}

interface VariantConfig {
  icon: ReactNode;
  iconClassName?: string;

  defaultKey: string;

  defaultText: string;
}

function buildConfig(variant: EmptyStateVariant): VariantConfig {
  switch (variant) {
    case "error":
      return {
        icon: <AlertCircle />,
        iconClassName: styles.iconError,
        defaultKey: "emptyState.error",
        defaultText: "加载失败",
      };
    case "not-available":
      return {
        icon: <Plug />,
        iconClassName: styles.iconNotAvailable,
        defaultKey: "emptyState.notAvailable",
        defaultText: "此功能暂未开放",
      };
    case "no-results":
      return {
        icon: <Search />,
        iconClassName: styles.iconNoResults,
        defaultKey: "emptyState.noResults",
        defaultText: "未找到匹配结果，试试调整筛选条件",
      };
    case "forbidden":
      return {
        icon: <Lock />,
        iconClassName: styles.iconForbidden,
        defaultKey: "emptyState.forbidden",
        defaultText: "权限不足",
      };
    case "empty":
    default:
      return {
        icon: <Inbox strokeWidth={1} />,
        defaultKey: "emptyState.empty",
        defaultText: "暂无数据",
      };
  }
}

export function EmptyState({
  variant = "empty",
  size = "default",
  icon,
  description,
  action,
  onRetry,
  className,
  style,
}: EmptyStateProps) {
  const { t } = useTranslation();
  const config = buildConfig(variant);
  const isInline = size === "inline";

  const resolvedDescription =
    description ?? t(config.defaultKey, config.defaultText);

  const resolvedAction = isInline
    ? null
    : action ??
      (variant === "error" && onRetry ? (
        <Btn size="sm" icon={<RefreshCw />} onClick={onRetry}>
          {t("common.refresh", "刷新")}
        </Btn>
      ) : null);

  const rootClass = `${styles.root} ${isInline ? styles.rootInline : ""} ${
    className ?? ""
  }`
    .replace(/\s+/g, " ")
    .trim();
  const iconClass = `${styles.icon} ${isInline ? styles.iconInline : ""} ${
    config.iconClassName ?? ""
  }`
    .replace(/\s+/g, " ")
    .trim();
  const descriptionClass = `${styles.description} ${
    isInline ? styles.descriptionInline : ""
  }`
    .replace(/\s+/g, " ")
    .trim();

  return (
    <div className={rootClass} style={style}>
      <div className={iconClass}>{icon ?? config.icon}</div>
      <p className={descriptionClass}>{resolvedDescription}</p>
      {resolvedAction ? (
        <div className={styles.action}>{resolvedAction}</div>
      ) : null}
    </div>
  );
}

export default EmptyState;
