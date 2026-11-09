/* eslint-disable react-refresh/only-export-components */
import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { useLoaderData } from "react-router-dom";
import {
  ArrowUpNarrowWide,
  Check as CheckIcon,
  DatabaseZap,
  HelpCircle,
  Pencil,
  Share2,
  SlidersHorizontal,
  Trash2,
  X,
} from "lucide-react";
import { EmptyState } from "@/components/EmptyState";
import { InputWithExample } from "@/components/InputWithExample";
import { Page, PageBody } from "@/components/Page";
import { PageHeader } from "@/components/PageHeader";
import Skeleton from "@/components/Skeleton";
import { RuaCard } from "@/components/RuaCard";
import {
  Btn,
  Check,
  Dropdown,
  EnableDisableSwitch,
  FormField,
  Modal,
  NumberInput,
  Popconfirm,
  Radio,
  RadioGroup,
  Select,
  TextArea,
  TextInput,
  Tooltip,
  toast,
} from "@/ui";
import { agentsApi } from "@/api/modules/agents";
import {
  memoryApi,
  type MemoryKind,
  type MemoryRecord,
  type MemoryScope,
  type MemoryVisibility,
  type MemoryVisibilityAgentOption,
  type MemoryVisibilityMode,
} from "@/api/modules/memory";
import type { AgentsRunningConfig } from "@/api/types";
import { useAgentStore } from "@/stores/agentStore";
import { routeAgentId } from "@/routes/routeAgent";
import styles from "./index.module.less";

type SortMode = "updated" | "created" | "title";
type SortOrder = "asc" | "desc";
type MemorySearchMode = AgentsRunningConfig["memory_summary"]["search_mode"];

type EditForm = {
  id: string;
  title: string;
  content: string;
  searchTerms: string;
  kind: MemoryKind;
  scope: MemoryScope;
  confidence: number | null;
  importance: number | null;
};

type VisibilityForm = {
  memoryIds: string[];
  mode: MemoryVisibilityMode;
  granteeAgentIds: string[];
  batch: boolean;
};

const SORT_MODE_KEY = "rua.memory.sortField";
const SORT_ORDER_KEY = "rua.memory.sortOrder";

const MEMORY_KINDS: MemoryKind[] = [
  "fact",
  "preference",
  "convention",
  "project_fact",
  "tool_quirk",
  "personal_fact",
];
const MEMORY_SCOPES: MemoryScope[] = ["user", "agent", "workspace", "project"];

const loadSortMode = (): SortMode => {
  try {
    const value = localStorage.getItem(SORT_MODE_KEY);
    if (value === "updated" || value === "created" || value === "title") {
      return value;
    }
  } catch {}
  return "updated";
};

const loadSortOrder = (): SortOrder => {
  try {
    const value = localStorage.getItem(SORT_ORDER_KEY);
    if (value === "asc" || value === "desc") return value;
  } catch {}
  return "desc";
};

function normalize(value?: string | null): string {
  return (value ?? "").trim();
}

function recordTime(record: MemoryRecord, mode: SortMode): number {
  const raw =
    mode === "created"
      ? record.created_at
      : record.updated_at || record.created_at;
  const value = Date.parse(raw);
  return Number.isFinite(value) ? value : 0;
}

function deviceLabel(record: MemoryRecord): string {
  return (
    normalize(record.resource_device_name_snapshot) ||
    normalize(record.resource_device_id) ||
    normalize(record.written_on_device_name_snapshot) ||
    normalize(record.written_on_device_id)
  );
}

function editFormFromRecord(record: MemoryRecord): EditForm {
  return {
    id: record.id,
    title: record.title,
    content: record.content,
    searchTerms: (record.terms || [])
      .filter((term) => term.term_kind === "search_term")
      .map((term) => term.term)
      .join("\n"),
    kind: record.kind,
    scope: record.scope,
    confidence: roundScore(record.confidence),
    importance:
      record.importance == null ? null : roundScore(record.importance),
  };
}

function roundScore(value: number): number {
  return Math.round(value * 100) / 100;
}

function scorePercent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

function privateVisibility(record: MemoryRecord): MemoryVisibility {
  return {
    memory_id: record.id,
    owner_agent_id: record.agent_id,
    mode: "private",
    grantee_agent_ids: [],
    grants: [],
  };
}

function visibilityLabel(
  visibility: MemoryVisibility | undefined,
  t: (key: string, params?: Record<string, unknown>) => string,
): string {
  if (!visibility || visibility.mode === "private") return t("memory.private");
  if (visibility.mode === "all_agents") return t("memory.sharedAll");
  return t("memory.sharedCount", {
    count: visibility.grantee_agent_ids.filter((id) => id !== "*").length,
  });
}

function memorySearchMode(config: AgentsRunningConfig): MemorySearchMode {
  return config.memory_summary.search_mode ?? "precise";
}

function MemoryOptionRow({
  label,
  tooltip,
  children,
}: {
  label: ReactNode;
  tooltip?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className={styles.optionsRow}>
      <div className={styles.optionsLabel}>
        <span>{label}</span>
        {tooltip && (
          <Tooltip title={tooltip}>
            <button
              type="button"
              className={styles.optionsHelp}
              aria-label="配置说明"
            >
              <HelpCircle size={14} />
            </button>
          </Tooltip>
        )}
      </div>
      <div className={styles.optionsControl}>{children}</div>
    </div>
  );
}

function MemoryOptionsModal({
  open,
  selectedAgent,
  onClose,
}: {
  open: boolean;
  selectedAgent: string;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<AgentsRunningConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setLoadError(null);
    setDraft(null);
    agentsApi
      .getAgentRunningConfig(selectedAgent)
      .then((config) => {
        if (!cancelled) setDraft(config);
      })
      .catch((error) => {
        if (cancelled) return;
        const message =
          error instanceof Error
            ? error.message
            : t("memory.optionsLoadFailed");
        setLoadError(message);
        toast.error(message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, selectedAgent, t]);

  function updateMemorySummary(
    patch: Partial<AgentsRunningConfig["memory_summary"]>,
  ) {
    setDraft((current) =>
      current
        ? {
            ...current,
            memory_summary: {
              ...current.memory_summary,
              ...patch,
            },
          }
        : current,
    );
  }

  async function saveOptions() {
    if (!draft) return;
    setSaving(true);
    try {
      const updated = await agentsApi.updateAgentRunningConfig(
        selectedAgent,
        draft,
      );
      setDraft(updated);
      toast.success(t("memory.optionsSaveSuccess"));
      onClose();
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("memory.optionsSaveFailed"),
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("memory.optionsTitle")}
      width={560}
      footer={
        <div className={styles.modalFooter}>
          <Btn variant="outline" onClick={onClose} disabled={saving}>
            {t("common.cancel")}
          </Btn>
          <Btn
            onClick={() => void saveOptions()}
            loading={saving}
            disabled={!draft}
          >
            {t("common.save")}
          </Btn>
        </div>
      }
    >
      <div className={styles.optionsPanel}>
        {loading && (
          <div className={styles.optionsState}>
            {t("memory.optionsLoading")}
          </div>
        )}
        {!loading && loadError && (
          <div className={styles.optionsState}>{loadError}</div>
        )}
        {!loading && draft && (
          <>
            <MemoryOptionRow
              label={t("memory.searchMode")}
              tooltip={t("memory.searchModeTooltip")}
            >
              <RadioGroup
                className={styles.searchModeGroup}
                value={memorySearchMode(draft)}
                onChange={(mode) =>
                  updateMemorySummary({
                    search_mode: mode as MemorySearchMode,
                  })
                }
              >
                <Radio className={styles.searchModeOption} value="grep">
                  <span className={styles.searchModeLabel}>
                    {t("memory.searchModeGrep")}
                  </span>
                  <span className={styles.searchModeDescription}>
                    {t("memory.searchModeGrepDescription")}
                  </span>
                </Radio>
                <Radio className={styles.searchModeOption} value="precise">
                  <span className={styles.searchModeLabel}>
                    {t("memory.searchModePrecise")}
                  </span>
                  <span className={styles.searchModeDescription}>
                    {t("memory.searchModePreciseDescription")}
                  </span>
                </Radio>
              </RadioGroup>
            </MemoryOptionRow>
            <MemoryOptionRow
              label={t("agentConfig.memorySummaryEnabled")}
              tooltip={t("agentConfig.memorySummaryEnabledTooltip")}
            >
              <EnableDisableSwitch
                enabled={draft.memory_summary.memory_summary_enabled}
                onChange={(enabled) =>
                  updateMemorySummary({
                    memory_summary_enabled: enabled,
                  })
                }
              />
            </MemoryOptionRow>
            <MemoryOptionRow
              label={t("agentConfig.dreamCron")}
              tooltip={t("agentConfig.dreamCronTooltip")}
            >
              <InputWithExample
                value={draft.memory_summary.dream_cron}
                placeholder={t("agentConfig.dreamCronPlaceholder")}
                onChange={(event) =>
                  updateMemorySummary({ dream_cron: event.target.value })
                }
              />
            </MemoryOptionRow>
          </>
        )}
      </div>
    </Modal>
  );
}

interface MemoryInitialData {
  agentId: string;
  records: MemoryRecord[];
  visibility: Record<string, MemoryVisibility>;
  visibilityOptions: MemoryVisibilityAgentOption[];
}

interface MemoryPageProps {
  initialData?: MemoryInitialData;
}

export async function loader(): Promise<MemoryInitialData> {
  const agentId = routeAgentId();
  const [records, visibilityOptions] = await Promise.all([
    memoryApi.listMemoryRecords(agentId),
    memoryApi.getMemoryVisibilityOptions(agentId),
  ]);
  return {
    agentId,
    records: records.records || [],
    visibility: records.visibility || {},
    visibilityOptions: visibilityOptions.agents || [],
  };
}

export function RouteComponent() {
  return <MemoryPage initialData={useLoaderData() as MemoryInitialData} />;
}

export default function MemoryPage({ initialData }: MemoryPageProps) {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const [records, setRecords] = useState<MemoryRecord[]>(
    initialData?.records ?? [],
  );
  const [loading, setLoading] = useState(initialData === undefined);
  const [actionLoading, setActionLoading] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [searchTerm, setSearchTerm] = useState("");
  const [sortMode, setSortMode] = useState<SortMode>(loadSortMode);
  const [sortOrder, setSortOrder] = useState<SortOrder>(loadSortOrder);
  const [editing, setEditing] = useState<EditForm | null>(null);
  const [saving, setSaving] = useState(false);
  const [visibilityById, setVisibilityById] = useState<
    Record<string, MemoryVisibility>
  >(initialData?.visibility ?? {});
  const [visibilityOptions, setVisibilityOptions] = useState<
    MemoryVisibilityAgentOption[]
  >(initialData?.visibilityOptions ?? []);
  const [sharing, setSharing] = useState<VisibilityForm | null>(null);
  const [savingVisibility, setSavingVisibility] = useState(false);
  const [batchShareActive, setBatchShareActive] = useState(false);
  const [selectedMemoryIds, setSelectedMemoryIds] = useState<string[]>([]);
  const [memoryOptionsOpen, setMemoryOptionsOpen] = useState(false);

  const loadRecords = useCallback(async () => {
    setLoading(true);
    setLoadError(false);
    try {
      const response = await memoryApi.listMemoryRecords(selectedAgent);
      setRecords(response.records || []);
      setVisibilityById(response.visibility || {});
    } catch (error) {
      console.error("Failed to load Memory records:", error);
      setLoadError(true);
      setRecords([]);
      setVisibilityById({});
      toast.error(t("memory.loadError"));
    } finally {
      setLoading(false);
    }
  }, [selectedAgent, t]);

  useEffect(() => {
    if (initialData) {
      setRecords(initialData.records);
      setVisibilityById(initialData.visibility);
      setLoadError(false);
      setLoading(false);
      return;
    }
    void loadRecords();
  }, [initialData, loadRecords]);

  useEffect(() => {
    setSelectedMemoryIds((prev) =>
      prev.filter((id) => records.some((record) => record.id === id)),
    );
  }, [records]);

  useEffect(() => {
    if (initialData) {
      setVisibilityOptions(initialData.visibilityOptions);
      return;
    }
    let alive = true;
    memoryApi
      .getMemoryVisibilityOptions(selectedAgent)
      .then((response) => {
        if (alive) setVisibilityOptions(response.agents || []);
      })
      .catch((error) => {
        console.error("Failed to load Memory visibility options:", error);
        if (alive) setVisibilityOptions([]);
      });
    return () => {
      alive = false;
    };
  }, [initialData, selectedAgent]);

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

  const handleDelete = useCallback(
    async (memoryId: string) => {
      try {
        await memoryApi.deleteMemoryRecord(selectedAgent, memoryId);
        setRecords((prev) => prev.filter((record) => record.id !== memoryId));
        toast.success(t("memory.deleteSuccess"));
      } catch (error) {
        console.error("Failed to delete Memory record:", error);
        toast.error(t("memory.deleteFailed"));
      }
    },
    [selectedAgent, t],
  );

  const handleReindex = useCallback(async () => {
    setActionLoading(true);
    try {
      const response = await memoryApi.reindexMemory(selectedAgent);
      toast.success(
        t("memory.reindexSuccess", { count: response.documents_indexed }),
      );
      await loadRecords();
    } catch (error) {
      console.error("Failed to reindex Memory:", error);
      toast.error(t("memory.reindexFailed"));
    } finally {
      setActionLoading(false);
    }
  }, [loadRecords, selectedAgent, t]);

  const handleSaveEdit = useCallback(async () => {
    if (!editing) return;
    const title = editing.title.trim();
    if (!title) {
      toast.error(t("memory.titleRequired"));
      return;
    }
    const content = editing.content.trim();
    if (!content) {
      toast.error(t("memory.contentRequired"));
      return;
    }
    const searchTerms = Array.from(
      new Set(
        editing.searchTerms
          .split(/\r?\n|,/)
          .map((term) => term.trim())
          .filter(Boolean),
      ),
    );
    if (searchTerms.length === 0) {
      toast.error(t("memory.searchTermsRequired"));
      return;
    }

    const confidence =
      editing.confidence == null ? Number.NaN : roundScore(editing.confidence);
    const importance =
      editing.importance == null ? undefined : roundScore(editing.importance);
    if (!Number.isFinite(confidence) || confidence < 0 || confidence > 1) {
      toast.error(t("memory.confidenceInvalid"));
      return;
    }
    if (
      importance !== undefined &&
      (!Number.isFinite(importance) || importance < 0 || importance > 1)
    ) {
      toast.error(t("memory.importanceInvalid"));
      return;
    }

    setSaving(true);
    try {
      const updated = await memoryApi.updateMemoryRecord(
        selectedAgent,
        editing.id,
        {
          title,
          content,
          search_terms: searchTerms,
          kind: editing.kind,
          scope: editing.scope,
          confidence,
          importance,
        },
      );
      setRecords((prev) =>
        prev.map((record) => (record.id === updated.id ? updated : record)),
      );
      setEditing(null);
      toast.success(t("memory.updateSuccess"));
    } catch (error) {
      console.error("Failed to update Memory record:", error);
      toast.error(t("memory.updateFailed"));
    } finally {
      setSaving(false);
    }
  }, [editing, selectedAgent, t]);

  const openVisibility = useCallback(
    (record: MemoryRecord) => {
      const visibility = visibilityById[record.id] || privateVisibility(record);
      setSharing({
        memoryIds: [record.id],
        mode: visibility.mode,
        granteeAgentIds: visibility.grantee_agent_ids.filter(
          (id) => id !== "*",
        ),
        batch: false,
      });
    },
    [visibilityById],
  );

  const startBatchShare = useCallback(() => {
    setBatchShareActive(true);
    setSelectedMemoryIds([]);
  }, []);

  const cancelBatchShare = useCallback(() => {
    setBatchShareActive(false);
    setSelectedMemoryIds([]);
  }, []);

  const toggleSelectedMemory = useCallback((memoryId: string) => {
    setSelectedMemoryIds((prev) =>
      prev.includes(memoryId)
        ? prev.filter((id) => id !== memoryId)
        : [...prev, memoryId],
    );
  }, []);

  const confirmBatchShare = useCallback(() => {
    if (selectedMemoryIds.length === 0) {
      toast.error(t("memory.batchSelectRequired"));
      return;
    }
    setSharing({
      memoryIds: selectedMemoryIds,
      mode: "selected",
      granteeAgentIds: [],
      batch: true,
    });
  }, [selectedMemoryIds, t]);

  const toggleVisibilityAgent = useCallback((agentId: string) => {
    setSharing((prev) => {
      if (!prev) return prev;
      const hasAgent = prev.granteeAgentIds.includes(agentId);
      return {
        ...prev,
        granteeAgentIds: hasAgent
          ? prev.granteeAgentIds.filter((id) => id !== agentId)
          : [...prev.granteeAgentIds, agentId],
      };
    });
  }, []);

  const handleSaveVisibility = useCallback(async () => {
    if (!sharing) return;
    if (sharing.mode === "selected" && sharing.granteeAgentIds.length === 0) {
      toast.error(t("memory.shareSelectAgent"));
      return;
    }

    setSavingVisibility(true);
    try {
      const updates = await Promise.all(
        sharing.memoryIds.map((memoryId) =>
          memoryApi.updateMemoryVisibility(selectedAgent, memoryId, {
            mode: sharing.mode,
            grantee_agent_ids:
              sharing.mode === "selected" ? sharing.granteeAgentIds : [],
          }),
        ),
      );
      setVisibilityById((prev) => ({
        ...prev,
        ...Object.fromEntries(
          updates.map((updated, index) => {
            const memoryId = updated.memory_id || sharing.memoryIds[index];
            return [memoryId, { ...updated, memory_id: memoryId }];
          }),
        ),
      }));
      if (sharing.batch) {
        setBatchShareActive(false);
        setSelectedMemoryIds([]);
      }
      setSharing(null);
      toast.success(
        sharing.batch
          ? t("memory.batchShareSuccess", { count: updates.length })
          : t("memory.shareSuccess"),
      );
    } catch (error) {
      console.error("Failed to update Memory visibility:", error);
      toast.error(t("memory.shareFailed"));
    } finally {
      setSavingVisibility(false);
    }
  }, [selectedAgent, sharing, t]);

  const visibleRecords = useMemo(() => {
    const query = searchTerm.trim().toLowerCase();
    let list = records;
    if (query) {
      list = list.filter((record) => {
        const haystack = [
          record.id,
          record.kind,
          record.scope,
          record.title,
          record.content,
          ...(record.terms || []).map((term) => term.term),
          record.status,
          record.written_on_device_id,
          record.written_on_device_name_snapshot,
          record.resource_device_id,
          record.resource_device_name_snapshot,
          visibilityLabel(visibilityById[record.id], t),
        ]
          .filter(Boolean)
          .join("\n")
          .toLowerCase();
        return haystack.includes(query);
      });
    }

    const sorted = [...list];
    const dir = sortOrder === "asc" ? 1 : -1;
    if (sortMode === "title") {
      const collator = new Intl.Collator(undefined);
      sorted.sort(
        (a, b) =>
          dir * collator.compare(normalize(a.title), normalize(b.title)),
      );
    } else {
      sorted.sort(
        (a, b) => dir * (recordTime(a, sortMode) - recordTime(b, sortMode)),
      );
    }
    return sorted;
  }, [records, searchTerm, sortMode, sortOrder, t, visibilityById]);

  return (
    <Page>
      <PageHeader
        agentScoped
        items={[{ title: t("memory.title") }]}
        onRefresh={loadRecords}
        refreshing={loading}
        moreButtonLoading={actionLoading}
        showMoreMenu={!batchShareActive}
        moreMenuItems={[
          {
            key: "batch-share",
            icon: <Share2 />,
            label: t("memory.batchShare"),
            onClick: startBatchShare,
            disabled: loading || actionLoading,
          },
          {
            key: "reindex",
            icon: <DatabaseZap />,
            label: t("memory.reindex"),
            onClick: handleReindex,
            disabled: loading || actionLoading,
          },
        ]}
        moreOptionsMenuItems={[
          {
            key: "memory-options",
            icon: <SlidersHorizontal />,
            label: t("memory.optionsMenu"),
            onClick: () => setMemoryOptionsOpen(true),
            disabled: loading || actionLoading,
          },
        ]}
        extra={
          <div className={styles.headerActions}>
            {batchShareActive ? (
              <>
                <Btn
                  variant="filled"
                  icon={<CheckIcon size={14} />}
                  onClick={confirmBatchShare}
                  disabled={selectedMemoryIds.length === 0}
                >
                  {t("common.confirm")}
                </Btn>
                <Btn
                  variant="outline"
                  icon={<X size={14} />}
                  onClick={cancelBatchShare}
                >
                  {t("common.cancel")}
                </Btn>
              </>
            ) : null}
          </div>
        }
      />
      <PageBody>
        {loading ? (
          <Skeleton variant="card" count={6} />
        ) : loadError ? (
          <EmptyState description={t("memory.loadError")} />
        ) : records.length === 0 ? (
          <EmptyState description={t("memory.empty")} />
        ) : (
          <>
            <div className={styles.filterRow}>
              <div className={styles.filterBar}>
                <TextInput
                  className={styles.filterInput}
                  placeholder={t("memory.searchPlaceholder")}
                  value={searchTerm}
                  onChange={(event) => setSearchTerm(event.target.value)}
                />
                {batchShareActive && (
                  <span className={styles.selectionCount}>
                    {t("memory.selectedCount", {
                      count: selectedMemoryIds.length,
                    })}
                  </span>
                )}
                <Dropdown
                  menu={{
                    items: [
                      {
                        key: "updated-desc",
                        label: t("memory.sortUpdatedDesc"),
                      },
                      {
                        key: "created-desc",
                        label: t("memory.sortCreatedDesc"),
                      },
                      {
                        key: "title-asc",
                        label: t("memory.sortTitleAsc"),
                      },
                    ],
                    selectedKeys: [`${sortMode}-${sortOrder}`],
                    onClick: ({ key }: { key: string }) => {
                      const index = key.lastIndexOf("-");
                      updateSortMode(key.slice(0, index) as SortMode);
                      updateSortOrder(key.slice(index + 1) as SortOrder);
                    },
                  }}
                  trigger={["click"]}
                >
                  <Btn variant="outline" icon={<ArrowUpNarrowWide />} />
                </Dropdown>
              </div>
            </div>

            {visibleRecords.length === 0 ? (
              <EmptyState description={t("memory.noMatch")} />
            ) : (
              <div className={styles.memoryGrid}>
                {visibleRecords.map((record) => {
                  const device =
                    deviceLabel(record) || t("memory.unknownDevice");
                  const updatedAt = record.updated_at || record.created_at;
                  const visibility = visibilityById[record.id];
                  const selected = selectedMemoryIds.includes(record.id);
                  return (
                    <RuaCard
                      key={record.id}
                      className={`${styles.memoryCard} ${
                        selected ? styles.memoryCardSelected : ""
                      }`}
                    >
                      <div className={styles.factHeader}>
                        {batchShareActive && (
                          <Check
                            className={styles.cardSelect}
                            checked={selected}
                            aria-label={t("memory.selectRecord", {
                              title: record.title,
                            })}
                            onChange={() => toggleSelectedMemory(record.id)}
                          />
                        )}
                        <p className={styles.factTitle} title={record.title}>
                          {record.title}
                        </p>
                        {!batchShareActive && (
                          <div className={styles.cardActions}>
                            <Btn
                              variant="ghost"
                              icon={<Pencil size={14} />}
                              aria-label={t("common.edit")}
                              onClick={() =>
                                setEditing(editFormFromRecord(record))
                              }
                            />
                            <Btn
                              variant="ghost"
                              icon={<Share2 size={14} />}
                              aria-label={t("memory.shareAction")}
                              onClick={() => openVisibility(record)}
                            />
                            <Popconfirm
                              title={t("memory.deleteConfirm")}
                              okText={t("common.delete")}
                              cancelText={t("common.cancel")}
                              danger
                              onConfirm={() => handleDelete(record.id)}
                            >
                              <Btn
                                variant="ghost"
                                icon={<Trash2 size={14} />}
                                aria-label={t("common.delete")}
                              />
                            </Popconfirm>
                          </div>
                        )}
                      </div>
                      <p
                        className={styles.contentPreview}
                        title={record.content}
                      >
                        {record.content}
                      </p>
                      <div className={styles.tagRow}>
                        <span className={styles.tag}>
                          {t(`memory.kind.${record.kind}`)}
                        </span>
                        <span className={styles.tag}>
                          {t(`memory.scope.${record.scope}`)}
                        </span>
                        <span
                          className={`${styles.tag} ${
                            record.status === "active"
                              ? styles.statusActive
                              : styles.statusMuted
                          }`}
                        >
                          {t(`memory.status.${record.status}`)}
                        </span>
                        <span className={styles.tag}>
                          {visibilityLabel(visibility, t)}
                        </span>
                      </div>
                      <div className={styles.metaGrid}>
                        <span className={styles.metaLabel}>
                          {t("memory.device")}
                        </span>
                        <span className={styles.metaValue} title={device}>
                          {device}
                        </span>
                        <span className={styles.metaLabel}>
                          {t("memory.updatedAt")}
                        </span>
                        <span className={styles.metaValue}>
                          {new Date(updatedAt).toLocaleString()}
                        </span>
                        <span className={styles.metaLabel}>
                          {t("memory.confidence")}
                        </span>
                        <span className={styles.metaValue}>
                          {scorePercent(record.confidence)}
                        </span>
                      </div>
                    </RuaCard>
                  );
                })}
              </div>
            )}
          </>
        )}
      </PageBody>
      <MemoryOptionsModal
        open={memoryOptionsOpen}
        selectedAgent={selectedAgent}
        onClose={() => setMemoryOptionsOpen(false)}
      />
      <Modal
        open={!!editing}
        title={t("memory.editTitle")}
        onClose={() => setEditing(null)}
        footer={
          <div className={styles.modalFooter}>
            <Btn variant="outline" onClick={() => setEditing(null)}>
              {t("common.cancel")}
            </Btn>
            <Btn onClick={handleSaveEdit} loading={saving}>
              {t("common.save")}
            </Btn>
          </div>
        }
      >
        {editing && (
          <div className={styles.editForm}>
            <FormField label={t("memory.memoryTitle")} required>
              <TextInput
                maxLength={60}
                value={editing.title}
                onChange={(event) =>
                  setEditing((prev) =>
                    prev ? { ...prev, title: event.target.value } : prev,
                  )
                }
              />
              <span className={styles.characterCount}>
                {editing.title.length}/60
              </span>
            </FormField>
            <FormField label={t("memory.content")} required>
              <TextArea
                rows={5}
                value={editing.content}
                onChange={(event) =>
                  setEditing((prev) =>
                    prev ? { ...prev, content: event.target.value } : prev,
                  )
                }
              />
            </FormField>
            <FormField label={t("memory.searchTerms")} required>
              <TextArea
                rows={3}
                value={editing.searchTerms}
                onChange={(event) =>
                  setEditing((prev) =>
                    prev ? { ...prev, searchTerms: event.target.value } : prev,
                  )
                }
              />
            </FormField>
            <div className={styles.formGrid}>
              <FormField label={t("memory.kindLabel")}>
                <Select
                  value={editing.kind}
                  onChange={(value) =>
                    setEditing((prev) =>
                      prev ? { ...prev, kind: value as MemoryKind } : prev,
                    )
                  }
                  options={MEMORY_KINDS.map((kind) => ({
                    value: kind,
                    label: t(`memory.kind.${kind}`),
                  }))}
                />
              </FormField>
              <FormField label={t("memory.scopeLabel")}>
                <Select
                  value={editing.scope}
                  onChange={(value) =>
                    setEditing((prev) =>
                      prev ? { ...prev, scope: value as MemoryScope } : prev,
                    )
                  }
                  options={MEMORY_SCOPES.map((scope) => ({
                    value: scope,
                    label: t(`memory.scope.${scope}`),
                  }))}
                />
              </FormField>
              <FormField label={t("memory.confidence")}>
                <NumberInput
                  min={0}
                  max={1}
                  step={0.01}
                  inputStep="0.01"
                  value={editing.confidence}
                  onChange={(event) =>
                    setEditing((prev) =>
                      prev ? { ...prev, confidence: event } : prev,
                    )
                  }
                />
              </FormField>
              <FormField label={t("memory.importance")}>
                <NumberInput
                  min={0}
                  max={1}
                  step={0.01}
                  inputStep="0.01"
                  value={editing.importance}
                  onChange={(event) =>
                    setEditing((prev) =>
                      prev ? { ...prev, importance: event } : prev,
                    )
                  }
                />
              </FormField>
            </div>
          </div>
        )}
      </Modal>
      <Modal
        open={!!sharing}
        title={
          sharing?.batch
            ? t("memory.batchShareTitle", {
                count: sharing.memoryIds.length,
              })
            : t("memory.shareTitle")
        }
        onClose={() => setSharing(null)}
        footer={
          <div className={styles.modalFooter}>
            <Btn variant="outline" onClick={() => setSharing(null)}>
              {t("common.cancel")}
            </Btn>
            <Btn onClick={handleSaveVisibility} loading={savingVisibility}>
              {t("common.save")}
            </Btn>
          </div>
        }
      >
        {sharing && (
          <div className={styles.visibilityForm}>
            <RadioGroup
              value={sharing.mode}
              onChange={(mode) =>
                setSharing((prev) =>
                  prev
                    ? {
                        ...prev,
                        mode: mode as MemoryVisibilityMode,
                      }
                    : prev,
                )
              }
            >
              <Radio value="private">{t("memory.private")}</Radio>
              <Radio value="selected">{t("memory.sharedSelected")}</Radio>
              <Radio value="all_agents">{t("memory.sharedAll")}</Radio>
            </RadioGroup>

            {sharing.mode === "selected" && (
              <div className={styles.agentPicker}>
                {visibilityOptions.length === 0 ? (
                  <span className={styles.emptyAgents}>
                    {t("memory.noShareTargets")}
                  </span>
                ) : (
                  visibilityOptions.map((agent) => (
                    <Check
                      key={agent.id}
                      checked={sharing.granteeAgentIds.includes(agent.id)}
                      onChange={() => toggleVisibilityAgent(agent.id)}
                    >
                      <span className={styles.agentOption}>
                        <span>{agent.name || agent.id}</span>
                        <span className={styles.agentId}>{agent.id}</span>
                      </span>
                    </Check>
                  ))
                )}
              </div>
            )}
          </div>
        )}
      </Modal>
    </Page>
  );
}
