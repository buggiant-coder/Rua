/* eslint-disable react-refresh/only-export-components */
import { useState, useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useLoaderData } from "react-router-dom";
import api from "../../../api";
import { PageHeader } from "../../../components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import Skeleton from "@/components/Skeleton";
import { useAgentStore } from "../../../stores/agentStore";
import { ChannelCard } from "./components/ChannelCard";
import { ChannelDrawer } from "./components/ChannelDrawer";
import { AccessControlDrawer } from "./components/AccessControlDrawer";
import { getChannelLabel, type ChannelKey } from "./components/constants";
import { useChannels, type ChannelsInitialData } from "./useChannels";
import styles from "./index.module.less";
import { ArrowUpNarrowWide, PowerOff, ShieldCheck } from "lucide-react";
import { Btn, Dropdown, TextInput, Tooltip, toast } from "@/ui";
import { routeAgentId } from "@/routes/routeAgent";

const SORT_MODE_KEY = "rua.channels.sortMode";
const SORT_ORDER_KEY = "rua.channels.sortOrder";
const UPDATED_MAP_KEY = "rua.channels.updatedAt";

type SortMode = "default" | "name" | "updated";
type SortOrder = "asc" | "desc";

const loadSortMode = (): SortMode => {
  try {
    const v = localStorage.getItem(SORT_MODE_KEY);
    if (v === "default" || v === "name" || v === "updated") return v;
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

const loadUpdatedMap = (): Record<string, number> => {
  try {
    const raw = localStorage.getItem(UPDATED_MAP_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return typeof parsed === "object" && parsed !== null ? parsed : {};
  } catch {
    return {};
  }
};

const persistUpdatedAt = (key: string): Record<string, number> => {
  const map = loadUpdatedMap();
  map[key] = Date.now();
  try {
    localStorage.setItem(UPDATED_MAP_KEY, JSON.stringify(map));
  } catch {}
  return map;
};

interface ChannelsPageProps {
  initialData?: ChannelsInitialData;
}

export async function loader(): Promise<ChannelsInitialData> {
  const agentId = routeAgentId();
  const [channels, channelTypes, schemas] = await Promise.all([
    api.listChannels(),
    api.listChannelTypes(),
    api.listChannelSchemas(),
  ]);
  return {
    agentId,
    channels:
      (channels as unknown as Record<string, Record<string, unknown>>) ?? {},
    channelTypes: channelTypes ?? [],
    schemas,
  };
}

export function RouteComponent() {
  return <ChannelsPage initialData={useLoaderData() as ChannelsInitialData} />;
}

export default function ChannelsPage({ initialData }: ChannelsPageProps) {
  const { t } = useTranslation();
  const { channels, schemas, orderedKeys, isBuiltin, loading, fetchChannels } =
    useChannels(initialData);
  const { selectedAgent } = useAgentStore();

  const [searchTerm, setSearchTerm] = useState("");
  const [sortMode, setSortMode] = useState<SortMode>(loadSortMode);
  const [sortOrder, setSortOrder] = useState<SortOrder>(loadSortOrder);
  const [updatedMap, setUpdatedMap] =
    useState<Record<string, number>>(loadUpdatedMap);

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

  const touchUpdatedAt = useCallback((key: string) => {
    setUpdatedMap(persistUpdatedAt(key));
  }, []);

  const [channelDrawerOpen, setChannelDrawerOpen] = useState(false);
  const [accessDrawerOpen, setAccessDrawerOpen] = useState(false);
  const [selectedChannel, setSelectedChannel] = useState<ChannelKey | null>(
    null,
  );
  const [saving, setSaving] = useState(false);

  const openChannelDrawer = (key: ChannelKey) => {
    setSelectedChannel(key);
    setChannelDrawerOpen(true);
  };

  const handleChannelSave = useCallback(async () => {
    if (!selectedChannel) return;
  }, [selectedChannel]);

  const handleDisableAllChannels = useCallback(async () => {
    const keys = orderedKeys.filter((key) => key !== "console");
    if (keys.length === 0) return;
    setSaving(true);
    try {
      await Promise.all([
        channels.console
          ? api.updateChannelConfig("console", {
              ...(channels.console as Record<string, unknown>),
              enabled: true,
            })
          : Promise.resolve(),
        ...keys.map((key) => {
          const current = (channels[key] || {}) as Record<string, unknown>;
          if (current.enabled === false) return Promise.resolve();
          return api.updateChannelConfig(key, { ...current, enabled: false });
        }),
      ]);
      keys.forEach(touchUpdatedAt);
      touchUpdatedAt("console");
      toast.success(t("channels.disableAllSuccess"));
      fetchChannels();
    } catch {
      toast.error(t("channels.configFailed"));
    } finally {
      setSaving(false);
    }
  }, [channels, fetchChannels, orderedKeys, t, touchUpdatedAt]);

  const [hoveredCard, setHoveredCard] = useState<string | null>(null);

  const visibleKeys = useMemo<string[]>(() => {
    const q = searchTerm.trim().toLowerCase();
    let keys = orderedKeys;

    if (q) {
      keys = keys.filter((key) => {
        const label = getChannelLabel(key, t).toLowerCase();
        return label.includes(q) || key.toLowerCase().includes(q);
      });
    }

    if (sortMode === "default") {
      return sortOrder === "asc" ? [...keys].reverse() : keys;
    }

    const sorted = [...keys];
    const dir = sortOrder === "asc" ? 1 : -1;
    if (sortMode === "name") {
      sorted.sort((a, b) => {
        const la = getChannelLabel(a, t);
        const lb = getChannelLabel(b, t);
        return la.localeCompare(lb) * dir;
      });
    } else if (sortMode === "updated") {
      sorted.sort((a, b) => {
        const ua = updatedMap[a] ?? 0;
        const ub = updatedMap[b] ?? 0;
        if (ua === ub) {
          return getChannelLabel(a, t).localeCompare(getChannelLabel(b, t));
        }
        return sortOrder === "desc" ? ub - ua : ua - ub;
      });
    }
    return sorted;
  }, [orderedKeys, searchTerm, sortMode, sortOrder, updatedMap, t]);

  const showEmpty = !loading && visibleKeys.length === 0;

  return (
    <Page>
      <PageHeader
        agentScoped
        current={t("channels.title")}
        onRefresh={fetchChannels}
        refreshing={loading}
        moreButtonLoading={saving}
        moreMenuItems={[
          {
            key: "disableAll",
            icon: <PowerOff />,
            label: t("channels.disableAllExceptConsole"),
            onClick: handleDisableAllChannels,
            disabled: saving,
            danger: true,
          },
        ]}
        extra={
          <div className={styles.headerControls}>
            <Tooltip
              title={t("channels.aclTitle", {
                defaultValue: "Access control",
              })}
            >
              <Btn
                variant="filled"
                icon={<ShieldCheck />}
                onClick={() => setAccessDrawerOpen(true)}
              />
            </Tooltip>
          </div>
        }
      />
      <PageBody>
        {loading ? (
          <Skeleton variant="card" count={6} />
        ) : (
          <>
            <div className={styles.filterRow}>
              <TextInput
                placeholder={t("channels.searchPlaceholder")}
                value={searchTerm}
                onChange={(e) => setSearchTerm(e.target.value)}
                className={styles.filterInput}
              />
              <Dropdown
                menu={{
                  items: [
                    { key: "default-desc", label: t("channels.sortDefault") },
                    {
                      key: "name-asc",
                      label: t("channels.sortByName") + " A→Z",
                    },
                    {
                      key: "name-desc",
                      label: t("channels.sortByName") + " Z→A",
                    },
                    {
                      key: "updated-desc",
                      label: t("channels.sortByLastUpdated") + " ↓",
                    },
                    {
                      key: "updated-asc",
                      label: t("channels.sortByLastUpdated") + " ↑",
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

            {showEmpty ? (
              <div className={styles.emptyState}>
                {t("common.noData", { defaultValue: "No matching channels" })}
              </div>
            ) : (
              <div className={styles.channelsGrid}>
                {visibleKeys.map((key: string) => (
                  <ChannelCard
                    key={key}
                    channelKey={key}
                    config={
                      key === "console"
                        ? { ...(channels[key] || {}), enabled: true }
                        : channels[key] || {}
                    }
                    isHover={hoveredCard === key}
                    onClick={() => openChannelDrawer(key)}
                    onMouseEnter={() => setHoveredCard(key)}
                    onMouseLeave={() => setHoveredCard(null)}
                  />
                ))}
              </div>
            )}
          </>
        )}
      </PageBody>

      {}
      <ChannelDrawer
        open={channelDrawerOpen}
        activeKey={selectedChannel}
        activeLabel={selectedChannel ? getChannelLabel(selectedChannel, t) : ""}
        saving={saving}
        initialValues={
          selectedChannel
            ? (channels[selectedChannel] as Record<string, unknown>)
            : undefined
        }
        isBuiltin={selectedChannel ? isBuiltin(selectedChannel) : true}
        schema={
          selectedChannel
            ? schemas?.channels[
                selectedChannel === "weixin" ? "wechat" : selectedChannel
              ]
            : undefined
        }
        onClose={() => setChannelDrawerOpen(false)}
        onSubmit={handleChannelSave}
        onSaved={() => {
          if (selectedChannel) touchUpdatedAt(selectedChannel);
          fetchChannels();
          setSaving(false);
          setChannelDrawerOpen(false);
        }}
      />
      <AccessControlDrawer
        open={accessDrawerOpen}
        agentId={selectedAgent}
        onClose={() => setAccessDrawerOpen(false)}
      />
    </Page>
  );
}
