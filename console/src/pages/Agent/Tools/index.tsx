/* eslint-disable react-refresh/only-export-components */
import { useCallback, useMemo, useState } from "react";
import { useLoaderData } from "react-router-dom";
import { EmptyState } from "@/components/EmptyState";
import { useTools, type ToolsInitialData } from "./useTools";
import { useTranslation } from "react-i18next";
import type { ToolInfo } from "../../../api/modules/tools";
import { agentsApi } from "../../../api/modules/agents";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import Skeleton from "@/components/Skeleton";
import { RuaCard } from "@/components/RuaCard";
import styles from "./index.module.less";
import {
  ArrowUpNarrowWide,
  ChevronDown,
  ChevronRight,
  Clock,
  Eye,
  EyeOff,
  Power,
  PowerOff,
  SlidersHorizontal,
  Zap,
} from "lucide-react";
import { Btn, Dropdown, TextInput } from "@/ui";
import { WebSearchConfigModal } from "./WebSearchConfigModal";
import { routeAgentId } from "@/routes/routeAgent";
import { groupTools } from "./toolGroups";

const SORT_MODE_KEY = "rua.tools.sortField";
const SORT_ORDER_KEY = "rua.tools.sortOrder";

type SortMode = "default" | "name";
type SortOrder = "asc" | "desc";

const loadSortMode = (): SortMode => {
  try {
    const v = localStorage.getItem(SORT_MODE_KEY);
    if (v === "default" || v === "name") return v;
  } catch {}
  return "default";
};

const loadSortOrder = (): SortOrder => {
  try {
    const v = localStorage.getItem(SORT_ORDER_KEY);
    if (v === "asc" || v === "desc") return v;
  } catch {}
  return "desc";
};

function resolveToolDescription(
  toolName: string,
  fallback: string,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const key = `tools.builtinDescriptions.${toolName}`;
  const translated = t(key, { defaultValue: "" });
  return translated || fallback;
}

interface ToolsPageProps {
  initialData?: ToolsInitialData;
}

export async function loader(): Promise<ToolsInitialData> {
  const agentId = routeAgentId();
  return {
    agentId,
    tools: await agentsApi.listAgentTools(agentId),
  };
}

export function RouteComponent() {
  return <ToolsPage initialData={useLoaderData() as ToolsInitialData} />;
}

export default function ToolsPage({ initialData }: ToolsPageProps) {
  const { t } = useTranslation();
  const {
    tools,
    loading,
    batchLoading,
    loadTools,
    toggleEnabled,
    toggleAsyncExecution,
    enableAll,
    disableAll,
  } = useTools(initialData);
  const [hoverKey, setHoverKey] = useState<string | null>(null);

  const [searchTerm, setSearchTerm] = useState("");
  const [sortMode, setSortMode] = useState<SortMode>(loadSortMode);
  const [sortOrder, setSortOrder] = useState<SortOrder>(loadSortOrder);
  const [webSearchConfigOpen, setWebSearchConfigOpen] = useState(false);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(
    () => new Set(),
  );

  const toggleGroup = (id: string) => {
    setCollapsedGroups((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const updateSortMode = useCallback((mode: SortMode) => {
    setSortMode(mode);
    try {
      localStorage.setItem(SORT_MODE_KEY, mode);
    } catch {}
  }, []);

  const updateSortOrder = useCallback((order: SortOrder) => {
    setSortOrder(order);
    try {
      localStorage.setItem(SORT_ORDER_KEY, order);
    } catch {}
  }, []);

  const handleToggle = (tool: ToolInfo) => {
    toggleEnabled(tool);
  };

  const handleAsyncToggle = (tool: ToolInfo) => {
    toggleAsyncExecution(tool);
  };

  const collator = useMemo(() => new Intl.Collator(undefined), []);

  const visibleTools = useMemo<ToolInfo[]>(() => {
    const q = searchTerm.trim().toLowerCase();
    let list = tools;

    if (q) {
      list = list.filter((tool) => {
        const name = tool.name.toLowerCase();
        const desc = (tool.description || "").toLowerCase();
        return name.includes(q) || desc.includes(q);
      });
    }

    if (sortMode === "default") {
      return sortOrder === "asc" ? [...list].reverse() : list;
    }

    const sorted = [...list];
    const dir = sortOrder === "asc" ? 1 : -1;
    if (sortMode === "name") {
      sorted.sort((a, b) => dir * collator.compare(a.name, b.name));
    }
    return sorted;
  }, [tools, searchTerm, sortMode, sortOrder, collator]);

  const toolGroups = useMemo(() => groupTools(visibleTools), [visibleTools]);

  return (
    <Page>
      <PageHeader
        agentScoped
        items={[{ title: t("tools.title") }]}
        onRefresh={loadTools}
        refreshing={loading}
        moreButtonLoading={batchLoading}
        moreMenuItems={[
          {
            key: "enableAll",
            icon: <Power />,
            label: t("tools.enableAll"),
            onClick: enableAll,
            disabled: batchLoading || loading,
          },
          {
            key: "disableAll",
            icon: <PowerOff />,
            label: t("tools.disableAll"),
            onClick: disableAll,
            disabled: batchLoading || loading,
            danger: true,
          },
        ]}
      />
      <PageBody>
        {loading ? (
          <Skeleton variant="card" count={6} />
        ) : tools.length === 0 ? (
          <EmptyState description={t("tools.emptyState")} />
        ) : (
          <>
            <div className={styles.filterRow}>
              <div className={styles.filterBar}>
                <TextInput
                  placeholder={t("tools.searchPlaceholder")}
                  value={searchTerm}
                  onChange={(e) => setSearchTerm(e.target.value)}
                  className={styles.filterInput}
                />
                <Dropdown
                  menu={{
                    items: [
                      { key: "default-asc", label: t("tools.sortDefault") },
                      {
                        key: "name-asc",
                        label: t("tools.sortByName") + " A→Z",
                      },
                      {
                        key: "name-desc",
                        label: t("tools.sortByName") + " Z→A",
                      },
                    ],
                    selectedKeys: [`${sortMode}-${sortOrder}`],
                    onClick: ({ key }: { key: string }) => {
                      const idx = key.lastIndexOf("-");
                      updateSortMode(key.slice(0, idx) as SortMode);
                      updateSortOrder(key.slice(idx + 1) as SortOrder);
                    },
                  }}
                  trigger={["click"]}
                >
                  <Btn variant="outline" icon={<ArrowUpNarrowWide />} />
                </Dropdown>
              </div>
            </div>

            {visibleTools.length === 0 ? (
              <EmptyState description={t("tools.noMatch")} />
            ) : (
              <div className={styles.toolGroups}>
                {toolGroups.map((group) => (
                  <section
                    key={group.id}
                    aria-labelledby={`tool-group-${group.id}`}
                    className={styles.toolGroup}
                  >
                    <h2 className={styles.groupTitle}>
                      <button
                        type="button"
                        className={styles.groupHeader}
                        aria-expanded={!collapsedGroups.has(group.id)}
                        aria-labelledby={`tool-group-${group.id}`}
                        aria-controls={`tool-group-content-${group.id}`}
                        onClick={() => toggleGroup(group.id)}
                      >
                        {collapsedGroups.has(group.id) ? (
                          <ChevronRight size={18} aria-hidden="true" />
                        ) : (
                          <ChevronDown size={18} aria-hidden="true" />
                        )}
                        <span id={`tool-group-${group.id}`}>
                          {group.id === "other"
                            ? t("tools.groups.other", { defaultValue: "Other" })
                            : group.domain}
                        </span>
                        <span className={styles.groupCount}>
                          {group.tools.length}
                        </span>
                      </button>
                    </h2>
                    <div
                      id={`tool-group-content-${group.id}`}
                      hidden={collapsedGroups.has(group.id)}
                    >
                      <div className={styles.toolsGrid}>
                        {group.tools.map((tool) => {
                          const showAsync = tool.name === "bash";

                          const titleNode = (
                            <span className={styles.toolName}>{tool.name}</span>
                          );

                          const statusBadge = (
                            <span
                              key="status"
                              className={styles.statusContainer}
                            >
                              <span
                                className={`${styles.statusDot} ${
                                  tool.enabled
                                    ? styles.enabled
                                    : styles.disabled
                                }`}
                              />
                              <span
                                className={`${styles.statusText} ${
                                  tool.enabled
                                    ? styles.enabled
                                    : styles.disabled
                                }`}
                              >
                                {tool.enabled
                                  ? t("common.enabled")
                                  : t("common.disabled")}
                              </span>
                            </span>
                          );

                          return (
                            <RuaCard
                              key={tool.name}
                              hoverable
                              className={`${styles.toolCard} ${
                                tool.enabled ? styles.enabledCard : ""
                              } ${
                                hoverKey === tool.name
                                  ? styles.hoverCard
                                  : styles.normalCard
                              }`}
                              onMouseEnter={() => setHoverKey(tool.name)}
                              onMouseLeave={() => setHoverKey(null)}
                              title={titleNode}
                              actions={[statusBadge]}
                              footer={
                                <>
                                  {showAsync && (
                                    <Btn
                                      variant="outline"
                                      onClick={() => handleAsyncToggle(tool)}
                                      disabled={!tool.enabled}
                                      icon={
                                        tool.async_execution ? (
                                          <Zap />
                                        ) : (
                                          <Clock />
                                        )
                                      }
                                    >
                                      {tool.async_execution
                                        ? t("tools.asyncExecutionEnabled")
                                        : t("tools.asyncExecutionDisabled")}
                                    </Btn>
                                  )}
                                  {tool.name === "web_search" && (
                                    <Btn
                                      variant="outline"
                                      onClick={() =>
                                        setWebSearchConfigOpen(true)
                                      }
                                      icon={<SlidersHorizontal />}
                                    >
                                      {t("common.configure", {
                                        defaultValue: "Configure",
                                      })}
                                    </Btn>
                                  )}
                                  <Btn
                                    variant="outline"
                                    onClick={() => handleToggle(tool)}
                                    icon={tool.enabled ? <EyeOff /> : <Eye />}
                                  >
                                    {tool.enabled
                                      ? t("common.disable")
                                      : t("common.enable")}
                                  </Btn>
                                </>
                              }
                            >
                              <p className={styles.toolDescription}>
                                {resolveToolDescription(
                                  tool.name,
                                  tool.description,
                                  t,
                                )}
                              </p>
                            </RuaCard>
                          );
                        })}
                      </div>
                    </div>
                  </section>
                ))}
              </div>
            )}
            <WebSearchConfigModal
              open={webSearchConfigOpen}
              onClose={() => setWebSearchConfigOpen(false)}
            />
          </>
        )}
      </PageBody>
    </Page>
  );
}
