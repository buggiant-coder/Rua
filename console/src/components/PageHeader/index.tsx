import { Fragment, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { MoreVertical, RefreshCw, SlidersHorizontal } from "lucide-react";
import AgentBadge from "../AgentBadge";
import { Btn, Dropdown, type MenuProps } from "@/ui";
import styles from "./index.module.less";

export type PageHeaderBreadcrumbItem = {
  title: ReactNode;
};

export interface PageHeaderProps {
  items?: PageHeaderBreadcrumbItem[];
  parent?: ReactNode;
  current?: ReactNode;
  center?: ReactNode;
  extra?: ReactNode;
  moreMenuItems?: MenuProps["items"];
  moreOptionsMenuItems?: MenuProps["items"];
  onRefresh?: () => void | Promise<void>;
  refreshing?: boolean;
  moreButtonLoading?: boolean;
  showMoreMenu?: boolean;

  afterBreadcrumb?: ReactNode;
  subRow?: ReactNode;
  className?: string;

  agentScoped?: boolean;
}

function buildItemsFromParentCurrent(
  parent: ReactNode | undefined,
  current: ReactNode | undefined,
): PageHeaderBreadcrumbItem[] {
  const out: PageHeaderBreadcrumbItem[] = [];
  if (parent != null && parent !== "") out.push({ title: parent });
  if (current != null && current !== "") out.push({ title: current });
  return out;
}

export function PageHeader({
  items: itemsProp,
  parent,
  current,
  center,
  extra,
  moreMenuItems = [],
  moreOptionsMenuItems = [],
  onRefresh,
  refreshing = false,
  moreButtonLoading = false,
  showMoreMenu,
  afterBreadcrumb,
  subRow,
  className,
  agentScoped = false,
}: PageHeaderProps) {
  const { t } = useTranslation();
  const items =
    itemsProp !== undefined
      ? itemsProp
      : buildItemsFromParentCurrent(parent, current);
  const moreLabel = t("common.more", "More");
  const shouldShowMoreMenu =
    showMoreMenu ??
    (!extra ||
      moreMenuItems.length > 0 ||
      moreOptionsMenuItems.length > 0 ||
      Boolean(onRefresh));
  const optionItems = moreOptionsMenuItems.map((item) => {
    if (item.type === "divider" || item.icon) return item;
    return {
      ...item,
      icon: <SlidersHorizontal size={14} aria-hidden="true" />,
    };
  });
  const menuItems: MenuProps["items"] = [
    ...moreMenuItems,
    ...(moreMenuItems.length > 0 ? [{ type: "divider" } as const] : []),
    {
      key: "refresh",
      icon: (
        <RefreshCw
          size={14}
          aria-hidden="true"
          className={refreshing ? "icon-spin" : undefined}
        />
      ),
      label: t("common.refreshPage", "Refresh page"),
      disabled: refreshing,
      onClick: () => {
        if (onRefresh) {
          void onRefresh();
          return;
        }
        window.location.reload();
      },
    },
    ...optionItems,
  ];

  return (
    <>
      <div className={`${styles.pageHeader} ${className ?? ""}`.trim()}>
        <div className={styles.leading}>
          <div className={styles.leadingTop}>
            <div className={styles.breadcrumbHeader}>
              {items.map((item, index) => (
                <Fragment key={index}>
                  {index > 0 ? (
                    <span className={styles.breadcrumbSeparator}>/</span>
                  ) : null}
                  <span
                    className={
                      index === items.length - 1
                        ? styles.breadcrumbCurrent
                        : styles.breadcrumbParent
                    }
                  >
                    {item.title}
                  </span>
                </Fragment>
              ))}
              {agentScoped && <AgentBadge />}
              {afterBreadcrumb}
            </div>
          </div>
          {subRow}
        </div>
        {center ? <div className={styles.center}>{center}</div> : null}
        <div className={styles.extra}>
          {extra}
          {shouldShowMoreMenu ? (
            <Dropdown menu={{ items: menuItems }} trigger={["click"]}>
              <Btn
                variant="filled"
                icon={<MoreVertical size={14} aria-hidden="true" />}
                loading={moreButtonLoading}
                aria-label={moreLabel}
                title={moreLabel}
              />
            </Dropdown>
          ) : null}
        </div>
      </div>
      <hr className="rua-divider" style={{ margin: 0 }} />
    </>
  );
}
