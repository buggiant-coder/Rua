import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { useTranslation } from "react-i18next";
import { useAgentStore } from "../../../stores/agentStore";
import { skillApi } from "../../../api/modules/skill";
import { PageHeader } from "@/components/PageHeader";
import { useHubSearch, type SortBy } from "./useHubSearch";
import {
  useHubInstall,
  type InstallTarget,
  type InstallQueueItem,
} from "./useHubInstall";
import type { MarketResult } from "../../../api/modules/market";
import {
  ResultCard,
  DetailDrawer,
  QueueItem,
  EmptyState,
  AliyunConfigModal,
} from "./components";
import styles from "./index.module.less";
import { ArrowUpNarrowWide, MoreVertical, RefreshCw } from "lucide-react";
import { Btn, Dropdown, Pagination, Select, TextInput, Tooltip } from "@/ui";

function getCardKey(item: MarketResult) {
  return `${item.source}:${item.slug}`;
}

const InstallQueuePanel = memo(function InstallQueuePanel({
  queue,
  onClearCompleted,
  onCancel,
  onRetry,
}: {
  queue: InstallQueueItem[];
  onClearCompleted: () => void;
  onCancel: (id: string) => void;
  onRetry: (id: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className={styles.queueDrawer}>
      <div className={styles.queueHeader}>
        <span>{t("market.installQueue")}</span>
        <Btn size="sm" onClick={onClearCompleted}>
          {t("market.clearCompleted")}
        </Btn>
      </div>
      <div className={styles.queueList}>
        {queue.map((q) => (
          <QueueItem
            key={q.id}
            item={q}
            onCancel={onCancel}
            onRetry={onRetry}
          />
        ))}
      </div>
    </div>
  );
});

const ProviderChips = memo(function ProviderChips({
  providers,
  selectedKeys,
  onToggle,
  onDisabledClick,
}: {
  providers: {
    key: string;
    label: string;
    available: boolean;
    reason?: string | null;
  }[];
  selectedKeys: Set<string>;
  onToggle: (key: string) => void;
  onDisabledClick?: (key: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className={styles.providerChips}>
      {providers.map((p) => {
        const active = selectedKeys.has(p.key);
        const clickable = p.available || !!onDisabledClick;
        const klass = [
          styles.chip,
          active ? styles.chipActive : "",
          !p.available ? styles.chipDisabled : "",
        ]
          .filter(Boolean)
          .join(" ");
        return (
          <Tooltip
            key={p.key}
            title={
              p.available
                ? undefined
                : p.reason ?? t("market.providerUnavailable")
            }
          >
            <span
              className={klass}
              onClick={
                p.available
                  ? () => onToggle(p.key)
                  : onDisabledClick
                  ? () => onDisabledClick(p.key)
                  : undefined
              }
              role="button"
              tabIndex={clickable ? 0 : -1}
              onKeyDown={(e) => {
                if ((e.key === "Enter" || e.key === " ") && clickable) {
                  e.preventDefault();
                  if (p.available) onToggle(p.key);
                  else onDisabledClick?.(p.key);
                }
              }}
              aria-pressed={active}
              aria-disabled={!p.available}
            >
              {p.label}
            </span>
          </Tooltip>
        );
      })}
    </div>
  );
});

const SORT_OPTIONS: SortBy[] = ["relevance", "downloads", "installs", "stars"];

const SortFilterBar = memo(function SortFilterBar({
  sortBy,
  onSortChange,
  filterCategory,
  onFilterChange,
  categories,
}: {
  sortBy: SortBy;
  onSortChange: (v: SortBy) => void;
  filterCategory: string;
  onFilterChange: (v: string) => void;
  categories: string[];
}) {
  const { t } = useTranslation();
  const categoryOptions = useMemo(
    () => [
      { value: "", label: t("market.allCategories") },
      ...categories.map((c) => ({ value: c, label: c })),
    ],
    [categories, t],
  );
  return (
    <div className={styles.sortFilterRow}>
      <Dropdown
        menu={{
          items: SORT_OPTIONS.map((k) => ({
            key: k,
            label: t(`market.sort.${k}`),
          })),
          selectedKeys: [sortBy],
          onClick: ({ key }: { key: string }) => onSortChange(key as SortBy),
        }}
        trigger={["click"]}
      >
        <Btn variant="outline" icon={<ArrowUpNarrowWide />} />
      </Dropdown>
      {categories.length > 0 && (
        <Select
          size="sm"
          value={filterCategory}
          onChange={onFilterChange}
          className={styles.categorySelect}
          options={categoryOptions}
        />
      )}
    </div>
  );
});

interface MarketPageProps {
  embedded?: boolean;
  headerExtraContainer?: HTMLElement | null;
}

function MarketPage({
  embedded = false,
  headerExtraContainer,
}: MarketPageProps) {
  const { t } = useTranslation();
  const selectedAgent = useAgentStore((s) => s.selectedAgent);
  const market = useHubSearch();
  const [cardTargets, setCardTargets] = useState<Record<string, InstallTarget>>(
    {},
  );
  const [detailItem, setDetailItem] = useState<MarketResult | null>(null);
  const [aliyunModalOpen, setAliyunModalOpen] = useState(false);

  const [installedUrls, setInstalledUrls] = useState<Set<string>>(new Set());
  useEffect(() => {
    skillApi
      .listSkills(selectedAgent)
      .then((skills) => {
        const urls = new Set<string>();
        for (const s of skills) {
          if (s.install_meta?.source_url) urls.add(s.install_meta.source_url);
        }
        setInstalledUrls(urls);
      })
      .catch(() => undefined);
  }, [selectedAgent]);

  const handleDisabledChipClick = useCallback((key: string) => {
    if (key === "aliyun") setAliyunModalOpen(true);
  }, []);

  const cardTargetsRef = useRef(cardTargets);
  cardTargetsRef.current = cardTargets;

  const targetFor = useCallback(
    (item: MarketResult): InstallTarget =>
      cardTargetsRef.current[getCardKey(item)] ?? "workspace",
    [],
  );

  const setCardTarget = useCallback(
    (item: MarketResult, next: InstallTarget) => {
      setCardTargets((prev) => ({ ...prev, [getCardKey(item)]: next }));
    },
    [],
  );

  const install = useHubInstall({ selectedAgent });

  const onInstall = useCallback(
    (item: MarketResult) => {
      const target = cardTargetsRef.current[getCardKey(item)] ?? "workspace";
      install.enqueue([item], target);
    },
    [install],
  );

  const detailItemRef = useRef(detailItem);
  detailItemRef.current = detailItem;

  const handleDetailTargetChange = useCallback(
    (next: InstallTarget) => {
      const current = detailItemRef.current;
      if (current) setCardTarget(current, next);
    },
    [setCardTarget],
  );

  const handleDetailInstall = useCallback(() => {
    const current = detailItemRef.current;
    if (current) {
      onInstall(current);
      setDetailItem(null);
    }
  }, [onInstall]);

  const handleDetailClose = useCallback(() => {
    setDetailItem(null);
  }, []);

  const headerExtra: ReactNode = (
    <Dropdown
      menu={{
        items: [
          {
            key: "refresh",
            icon: (
              <RefreshCw
                size={14}
                className={market.loading ? "icon-spin" : ""}
              />
            ),
            label: t("common.refreshPage"),
            onClick: market.triggerSearch,
            disabled: market.loading,
          },
        ],
      }}
      trigger={["click"]}
    >
      <Btn
        variant="filled"
        icon={<MoreVertical />}
        loading={market.loading}
        aria-label={t("common.more")}
      />
    </Dropdown>
  );

  return (
    <div className={styles.marketPage}>
      {embedded && headerExtraContainer
        ? createPortal(headerExtra, headerExtraContainer)
        : null}
      {!embedded && (
        <PageHeader
          current={t("nav.market")}
          agentScoped
          onRefresh={market.triggerSearch}
          refreshing={market.loading}
        />
      )}
      <div className={styles.content}>
        <div className={styles.toolbar}>
          <div className={styles.searchContainer}>
            <TextInput
              className={styles.searchInput}
              placeholder={t("market.searchPlaceholder")}
              value={market.query}
              onChange={(e) => market.setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") market.triggerSearch();
              }}
              type="search"
              aria-label={t("market.searchPlaceholder")}
            />
            <Btn
              className={styles.searchButton}
              loading={market.loading && market.results.length === 0}
              onClick={market.triggerSearch}
            >
              {t("common.search")}
            </Btn>
          </div>
        </div>

        <ProviderChips
          providers={market.providers}
          selectedKeys={market.selectedProviderKeys}
          onToggle={market.toggleProvider}
          onDisabledClick={handleDisabledChipClick}
        />

        {market.results.length > 0 && (
          <SortFilterBar
            sortBy={market.sortBy}
            onSortChange={market.setSortBy}
            filterCategory={market.filterCategory}
            onFilterChange={market.setFilterCategory}
            categories={market.categories}
          />
        )}

        {market.globalError && (
          <div className={styles.errorRow}>{market.globalError}</div>
        )}
        {market.errors.map((err) => {
          const provider = market.providers.find((p) => p.key === err.provider);
          const label = provider?.label ?? err.provider;
          return (
            <div className={styles.errorRow} key={err.provider}>
              <strong>{label}</strong>: {err.message}
            </div>
          );
        })}

        {market.loading &&
        market.displayResults.length === 0 &&
        market.results.length === 0 ? (
          <EmptyState text={t("common.loading")} />
        ) : market.results.length === 0 &&
          (market.globalError || market.errors.length > 0) ? (
          <EmptyState variant="no-results" text={t("market.noResults")}>
            <Btn onClick={market.retry} loading={market.loading}>
              {t("market.retry")}
            </Btn>
          </EmptyState>
        ) : market.results.length === 0 && market.query.trim() ? (
          <EmptyState variant="no-results" text={t("market.noResults")} />
        ) : market.results.length === 0 ? (
          <EmptyState text={t("market.startTyping")} />
        ) : (
          <>
            <div className={styles.resultsGrid}>
              {market.displayResults.map((item) => (
                <ResultCard
                  key={getCardKey(item)}
                  item={item}
                  target={targetFor(item)}
                  isInstalled={installedUrls.has(item.source_url)}
                  onTargetChange={(next) => setCardTarget(item, next)}
                  onInstall={() => onInstall(item)}
                  onOpenDetail={() => setDetailItem(item)}
                />
              ))}
            </div>
            <div className={styles.paginationRow}>
              {market.totalResults > 0 && (
                <span className={styles.totalText}>
                  {t("market.totalResults", { count: market.totalResults })}
                </span>
              )}
              {market.totalPages > 1 && (
                <Pagination
                  current={market.currentPage}
                  total={market.totalPages}
                  pageSize={1}
                  onChange={market.goToPage}
                  showSizeChanger={false}
                  size="sm"
                />
              )}
            </div>
          </>
        )}
      </div>

      {install.queue.length > 0 && (
        <InstallQueuePanel
          queue={install.queue}
          onClearCompleted={install.clearCompleted}
          onCancel={install.cancel}
          onRetry={install.retry}
        />
      )}

      <DetailDrawer
        item={detailItem}
        target={detailItem ? targetFor(detailItem) : "workspace"}
        onTargetChange={handleDetailTargetChange}
        onInstall={handleDetailInstall}
        onClose={handleDetailClose}
      />

      <AliyunConfigModal
        open={aliyunModalOpen}
        onClose={() => setAliyunModalOpen(false)}
        onSuccess={market.refreshProviders}
      />
    </div>
  );
}

export default MarketPage;
