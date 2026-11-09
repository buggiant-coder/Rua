import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useForm, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { useTranslation } from "react-i18next";
import api from "../../../api";
import type {
  BuiltinImportSpec,
  PoolSkillSpec,
  WorkspaceSkillSummary,
} from "../../../api/types";
import { parseErrorDetail } from "../../../utils/error";
import { handleScanError, checkScanWarnings } from "../../../utils/scanError";
import { ZIP_FILE_ACCEPT } from "@/utils/fileInputAccept";
import {
  getSkillDisplaySource,
  getSkillVisual,
  parseFrontmatter,
  SkillInstallInfo,
  useConflictRenameModal,
} from "../../Agent/Skills/components";
import { SkillUpdateButton } from "../../Agent/Skills/components/SkillUpdateButton";
import { SkillPoolCard, poolSkillIconUrl } from "./components";
import { MarkdownCopy } from "../../../components/MarkdownCopy/MarkdownCopy";
import { BroadcastModal } from "./components/BroadcastModal";
import { ImportBuiltinModal } from "./components/ImportBuiltinModal";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { isMobile } from "@/utils/isMobile";
import styles from "./index.module.less";
import {
  ArrowUpNarrowWide,
  Download,
  LayoutGrid,
  List,
  MoreVertical,
  Plus,
  RefreshCcw,
  RefreshCw,
  Send,
  Trash2,
  X,
} from "lucide-react";
import {
  Btn,
  Check,
  Drawer,
  Dropdown,
  FormField,
  Modal,
  StackedList,
  Tag,
  TextInput,
  Tooltip,
  toast,
  type MenuProps,
} from "@/ui";
type PoolMode = "broadcast" | "create" | "edit";

const SKILL_POOL_ZIP_MAX_MB = 100;

interface SkillPoolPageProps {
  embedded?: boolean;
  headerExtraContainer?: HTMLElement | null;
}

let skillPoolPageCache: {
  skills: PoolSkillSpec[];
  workspaces: WorkspaceSkillSummary[];
} | null = null;

export function __resetSkillPoolPageCacheForTests() {
  skillPoolPageCache = null;
}

function SkillPoolPage({
  embedded = false,
  headerExtraContainer,
}: SkillPoolPageProps) {
  const { t } = useTranslation();
  const [skills, setSkills] = useState<PoolSkillSpec[]>(
    () => skillPoolPageCache?.skills ?? [],
  );
  const [workspaces, setWorkspaces] = useState<WorkspaceSkillSummary[]>(
    () => skillPoolPageCache?.workspaces ?? [],
  );
  const [loading, setLoading] = useState(false);
  const [mode, setMode] = useState<PoolMode | null>(null);
  const [activeSkill, setActiveSkill] = useState<PoolSkillSpec | null>(null);
  const [broadcastInitialNames, setBroadcastInitialNames] = useState<string[]>(
    [],
  );
  const [configText, setConfigText] = useState("{}");
  const zipInputRef = useRef<HTMLInputElement>(null);
  const [importBuiltinModalOpen, setImportBuiltinModalOpen] = useState(false);
  const [builtinSources, setBuiltinSources] = useState<BuiltinImportSpec[]>([]);
  const [importBuiltinLoading, setImportBuiltinLoading] = useState(false);
  const { showConflictRenameModal, conflictRenameModal } =
    useConflictRenameModal();
  const [selectedPoolSkills, setSelectedPoolSkills] = useState<Set<string>>(
    new Set(),
  );
  const [batchModeEnabled, setBatchModeEnabled] = useState(false);
  const poolBatchMode = batchModeEnabled;
  const isMobileMode = useMemo(() => isMobile(), []);
  const SKILLPOOL_VIEW_MODE_KEY = "agent.skillPool.viewMode";
  const [viewMode, setViewMode] = useState<"card" | "list">(() => {
    try {
      const v = localStorage.getItem(SKILLPOOL_VIEW_MODE_KEY);
      if (v === "card" || v === "list") return v;
    } catch {}
    return "card";
  });

  const effectiveViewMode: "card" | "list" = isMobileMode ? "card" : viewMode;
  const [searchQuery, setSearchQuery] = useState("");
  const SKILLPOOL_SORT_FIELD_KEY = "agent.skillPool.sortField";
  const SKILLPOOL_SORT_ORDER_KEY = "agent.skillPool.sortOrder";
  const [sortField, setSortField] = useState<"name" | "lastUpdated">(() => {
    try {
      const v = localStorage.getItem(SKILLPOOL_SORT_FIELD_KEY);
      if (v === "name" || v === "lastUpdated") return v;
    } catch {}
    return "name";
  });
  const [sortOrder, setSortOrder] = useState<"asc" | "desc">(() => {
    try {
      const v = localStorage.getItem(SKILLPOOL_SORT_ORDER_KEY);
      if (v === "asc" || v === "desc") return v;
    } catch {}
    return "asc";
  });

  useEffect(() => {
    try {
      localStorage.setItem(SKILLPOOL_SORT_FIELD_KEY, sortField);
    } catch {}
  }, [sortField]);
  useEffect(() => {
    try {
      localStorage.setItem(SKILLPOOL_SORT_ORDER_KEY, sortOrder);
    } catch {}
  }, [sortOrder]);
  useEffect(() => {
    try {
      localStorage.setItem(SKILLPOOL_VIEW_MODE_KEY, viewMode);
    } catch {}
  }, [viewMode]);

  const togglePoolSelect = (name: string) => {
    setSelectedPoolSkills((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };

  const clearPoolSelection = () => {
    setSelectedPoolSkills(new Set());
    setBatchModeEnabled(false);
  };

  const toggleBatchMode = () => {
    if (batchModeEnabled) {
      clearPoolSelection();
    } else {
      setBatchModeEnabled(true);
    }
  };

  const selectAllPool = () =>
    setSelectedPoolSkills(new Set(skills.map((s) => s.name)));

  const filteredSkills = useMemo(() => {
    const query = searchQuery.toLowerCase();
    const filtered = skills.filter(
      (skill) =>
        skill.name.toLowerCase().includes(query) ||
        (skill.description || "").toLowerCase().includes(query),
    );
    return filtered.slice().sort((a, b) => {
      let cmp: number;
      if (sortField === "lastUpdated") {
        const ta = a.last_updated ? new Date(a.last_updated).getTime() : 0;
        const tb = b.last_updated ? new Date(b.last_updated).getTime() : 0;
        cmp = ta - tb;
      } else {
        cmp = a.name.localeCompare(b.name);
      }
      return sortOrder === "asc" ? cmp : -cmp;
    });
  }, [skills, searchQuery, sortField, sortOrder]);

  const poolFormSchema = z.object({
    name: z.string().min(1, t("skills.pleaseInputName")),
    content: z.string().min(1),
  });
  const {
    control: poolFormControl,
    handleSubmit: poolHandleSubmit,
    reset: poolFormReset,
    setValue: poolSetValue,
    formState: { errors: poolFormErrors },
  } = useForm<{ name: string; content: string }>({
    resolver: zodResolver(poolFormSchema),
    defaultValues: { name: "", content: "" },
  });
  const [drawerContent, setDrawerContent] = useState("");
  const [showMarkdown, setShowMarkdown] = useState(true);

  const dataLoadedRef = useRef(Boolean(skillPoolPageCache));

  const applyLoadedData = useCallback(
    (
      poolSkills: PoolSkillSpec[],
      workspaceSummaries: WorkspaceSkillSummary[],
    ) => {
      skillPoolPageCache = {
        skills: poolSkills,
        workspaces: workspaceSummaries,
      };
      setSkills(poolSkills);
      setWorkspaces(workspaceSummaries);
      dataLoadedRef.current = true;
    },
    [],
  );

  const loadData = useCallback(
    async (forceReload = false) => {
      if (dataLoadedRef.current && !forceReload) return;

      setLoading(skills.length === 0);
      try {
        const [poolSkills, workspaceSummaries] = await Promise.all([
          api.listSkillPoolSkills(),
          api.listSkillWorkspaces(),
        ]);
        applyLoadedData(poolSkills, workspaceSummaries);
      } catch (error) {
        toast.error(
          error instanceof Error ? error.message : "Failed to load skill pool",
        );
      } finally {
        setLoading(false);
      }
    },
    [applyLoadedData, skills.length],
  );

  const handleRefresh = useCallback(async () => {
    setLoading(skills.length === 0);
    try {
      const [poolSkills, workspaceSummaries] = await Promise.all([
        api.refreshSkillPool(),
        api.listSkillWorkspaces(),
      ]);
      applyLoadedData(poolSkills, workspaceSummaries);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Failed to refresh");
    } finally {
      setLoading(false);
    }
  }, [applyLoadedData, skills.length]);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  const closeModal = () => {
    setMode(null);
    setBroadcastInitialNames([]);
    setConfigText("{}");
  };

  const openCreate = () => {
    setMode("create");
    setDrawerContent("");
    setConfigText("{}");
    poolFormReset({ name: "", content: "" });
  };

  const openBroadcast = (skill?: PoolSkillSpec) => {
    setMode("broadcast");
    setBroadcastInitialNames(skill ? [skill.name] : []);
  };

  const openImportBuiltin = async () => {
    try {
      setImportBuiltinLoading(true);
      const sources = await api.listPoolBuiltinSources();
      setBuiltinSources(sources);
      setImportBuiltinModalOpen(true);
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("skillPool.importBuiltinFailed"),
      );
    } finally {
      setImportBuiltinLoading(false);
    }
  };

  const closeImportBuiltin = () => {
    if (importBuiltinLoading) return;
    setImportBuiltinModalOpen(false);
  };

  const openEdit = (skill: PoolSkillSpec) => {
    setMode("edit");
    setActiveSkill(skill);
    setDrawerContent(skill.content);
    setConfigText(JSON.stringify(skill.config || {}, null, 2));
    poolFormReset({ name: skill.name, content: skill.content });
  };

  const closeDrawer = () => {
    setMode(null);
    setActiveSkill(null);
  };

  const handleDrawerContentChange = (content: string) => {
    setDrawerContent(content);
    poolSetValue("content", content);
  };

  const validateFrontmatter = useCallback(
    (_: unknown, value: string) => {
      const content = drawerContent || value;
      if (!content || !content.trim()) {
        return Promise.reject(new Error(t("skills.pleaseInputContent")));
      }
      const fm = parseFrontmatter(content);
      if (!fm) {
        return Promise.reject(new Error(t("skills.frontmatterRequired")));
      }
      if (!fm.name) {
        return Promise.reject(new Error(t("skills.frontmatterNameRequired")));
      }
      if (!fm.description) {
        return Promise.reject(
          new Error(t("skills.frontmatterDescriptionRequired")),
        );
      }
      return Promise.resolve();
    },
    [drawerContent, t],
  );

  const handleBroadcast = async (
    broadcastSkillNames: string[],
    targetWorkspaceIds: string[],
  ) => {
    try {
      for (const skillName of broadcastSkillNames) {
        const renameMap: Record<string, string> = {};

        while (true) {
          try {
            await api.downloadSkillPoolSkill({
              skill_name: skillName,
              targets: targetWorkspaceIds.map((workspace_id) => ({
                workspace_id,
                target_name: renameMap[workspace_id] || undefined,
              })),
            });
            break;
          } catch (error) {
            if (handleScanError(error, t)) return;
            const detail = parseErrorDetail(error);
            const detailStr =
              typeof detail === "string"
                ? detail
                : detail?.message || detail?.detail || "";
            const isConflict =
              typeof detailStr === "string" &&
              (detailStr.includes("already exists") ||
                detailStr.includes("conflict"));

            if (isConflict) {
              const confirmed = await new Promise<boolean>((resolve) => {
                Modal.confirm({
                  title: t("skills.builtinUpgradeTitle"),
                  content: t("skills.builtinUpgradeContent", {
                    name: skillName,
                  }),
                  okText: t("common.confirm"),
                  cancelText: t("common.cancel"),
                  onOk: () => resolve(true),
                  onCancel: () => resolve(false),
                });
              });
              if (!confirmed) return;
              await api.downloadSkillPoolSkill({
                skill_name: skillName,
                targets: targetWorkspaceIds.map((workspace_id) => ({
                  workspace_id,
                  target_name: renameMap[workspace_id] || undefined,
                })),
                overwrite: true,
              });
              break;
            }

            throw error;
          }
        }
      }
      toast.success(t("skillPool.broadcastSuccess"));
      closeModal();

      await loadData(true);
      for (const skillName of broadcastSkillNames) {
        await checkScanWarnings(
          skillName,
          api.getBlockedHistory,
          api.getSkillScanner,
          t,
        );
      }
    } catch (error) {
      if (!handleScanError(error, t)) {
        toast.error(
          error instanceof Error
            ? error.message
            : t("skillPool.broadcastFailed"),
        );
      }
    }
  };

  const handleImportBuiltins = async (
    selectedNames: string[],
    overwriteConflicts: boolean = false,
  ) => {
    if (selectedNames.length === 0) return;
    try {
      setImportBuiltinLoading(true);
      const result = await api.importSelectedPoolBuiltins({
        skill_names: selectedNames,
        overwrite_conflicts: overwriteConflicts,
      });
      const imported = Array.isArray(result.imported) ? result.imported : [];
      const updated = Array.isArray(result.updated) ? result.updated : [];
      const unchanged = Array.isArray(result.unchanged) ? result.unchanged : [];

      if (!imported.length && !updated.length && unchanged.length) {
        toast.info(t("skillPool.importBuiltinNoChanges"));
        closeImportBuiltin();
        return;
      }

      if (imported.length || updated.length) {
        toast.success(
          t("skillPool.importBuiltinSuccess", {
            names: [...imported, ...updated].join(", "),
          }),
        );
      }
      closeImportBuiltin();

      await loadData(true);
    } catch (error) {
      const detail = parseErrorDetail(error);
      const conflicts = Array.isArray(detail?.conflicts)
        ? detail.conflicts
        : [];
      if (conflicts.length && !overwriteConflicts) {
        Modal.confirm({
          title: t("skillPool.importBuiltinConflictTitle"),
          content: (
            <div style={{ display: "grid", gap: 8 }}>
              <div>{t("skillPool.importBuiltinConflictContent")}</div>
              {conflicts.map((item) => (
                <div key={item.skill_name}>
                  <strong>{item.skill_name}</strong>
                  {"  "}
                  {t("skillPool.currentVersion")}:{" "}
                  {item.current_version_text || "-"}
                  {"  ->  "}
                  {t("skillPool.sourceVersion")}:{" "}
                  {item.source_version_text || "-"}
                </div>
              ))}
            </div>
          ),
          okText: t("common.confirm"),
          cancelText: t("common.cancel"),
          onOk: async () => {
            await handleImportBuiltins(selectedNames, true);
          },
        });
        return;
      }
      toast.error(
        error instanceof Error
          ? error.message
          : t("skillPool.importBuiltinFailed"),
      );
    } finally {
      setImportBuiltinLoading(false);
    }
  };

  const handleSavePoolSkill = poolHandleSubmit(async (values) => {
    const trimmedConfig = configText.trim();
    let parsedConfig: Record<string, unknown> = {};
    if (trimmedConfig && trimmedConfig !== "{}") {
      try {
        parsedConfig = JSON.parse(trimmedConfig);
      } catch {
        toast.error(t("skills.configInvalidJson"));
        return;
      }
    }

    const skillName = (values.name || "").trim();
    const skillContent = drawerContent || values.content;

    if (!skillName || !skillContent.trim()) return;

    try {
      const result =
        mode === "edit"
          ? await api.saveSkillPoolSkill({
              name: skillName,
              content: skillContent,
              source_name: activeSkill?.name,
              config: parsedConfig,
            })
          : await api
              .createSkillPoolSkill({
                name: skillName,
                content: skillContent,
                config: parsedConfig,
              })
              .then((created) => ({
                success: true,
                mode: "edit" as const,
                name: created.name,
              }));
      if (result.mode === "noop") {
        closeDrawer();
        return;
      }
      const savedAsNew =
        mode === "edit" && activeSkill && result.name !== activeSkill.name;
      toast.success(
        savedAsNew
          ? `${t("common.create")}: ${result.name}`
          : mode === "edit"
          ? t("common.save")
          : t("common.create"),
      );
      closeDrawer();

      await loadData(true);
      await checkScanWarnings(
        result.name || skillName,
        api.getBlockedHistory,
        api.getSkillScanner,
        t,
      );
    } catch (error) {
      if (handleScanError(error, t)) return;
      const detail = parseErrorDetail(error);
      if (detail?.suggested_name) {
        const renameMap = await showConflictRenameModal([
          {
            key: skillName,
            label: skillName,
            suggested_name: detail.suggested_name,
          },
        ]);
        if (renameMap) {
          const newName = Object.values(renameMap)[0];
          if (newName) {
            poolSetValue("name", newName);
            await handleSavePoolSkill();
          }
        }
        return;
      }
      toast.error(
        error instanceof Error ? error.message : t("common.save") + " failed",
      );
    }
  });

  const handleDelete = async (skill: PoolSkillSpec) => {
    Modal.confirm({
      title: t("skillPool.deleteTitle", { name: skill.name }),
      content:
        getSkillDisplaySource(skill.source) === "builtin"
          ? t("skillPool.deleteBuiltinConfirm")
          : t("skillPool.deleteConfirm"),
      okText: t("common.delete"),
      danger: true,
      onOk: async () => {
        await api.deleteSkillPoolSkill(skill.name);
        toast.success(t("skillPool.deletedFromPool"));

        await loadData(true);
      },
    });
  };

  const handleZipImport = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";

    if (!file.name.toLowerCase().endsWith(".zip")) {
      toast.warning(t("skills.zipOnly"));
      return;
    }

    const sizeMB = file.size / (1024 * 1024);
    if (sizeMB > SKILL_POOL_ZIP_MAX_MB) {
      toast.warning(
        t("skills.fileSizeExceeded", {
          limit: SKILL_POOL_ZIP_MAX_MB,
          size: sizeMB.toFixed(1),
        }),
      );
      return;
    }

    let renameMap: Record<string, string> | undefined;
    while (true) {
      try {
        const result = await api.uploadSkillPoolZip(file, {
          overwrite: false,
          rename_map: renameMap,
        });
        if (result.count > 0) {
          toast.success(
            t("skillPool.imported", { names: result.imported.join(", ") }),
          );
        } else {
          toast.info(t("skillPool.noNewImports"));
        }

        await loadData(true);
        if (result.count > 0 && Array.isArray(result.imported)) {
          for (const name of result.imported) {
            await checkScanWarnings(
              name,
              api.getBlockedHistory,
              api.getSkillScanner,
              t,
            );
          }
        }
        break;
      } catch (error) {
        const detail = parseErrorDetail(error);
        const conflicts = Array.isArray(detail?.conflicts)
          ? detail.conflicts
          : [];
        if (conflicts.length === 0) {
          if (handleScanError(error, t)) break;
          toast.error(
            error instanceof Error
              ? error.message
              : t("skillPool.zipImportFailed"),
          );
          break;
        }
        const newRenames = await showConflictRenameModal(
          conflicts.map(
            (c: { skill_name?: string; suggested_name?: string }) => ({
              key: c.skill_name || "",
              label: c.skill_name || "",
              suggested_name: c.suggested_name || "",
            }),
          ),
        );
        if (!newRenames) break;
        renameMap = { ...renameMap, ...newRenames };
      }
    }
  };

  const handleBatchDeletePool = async () => {
    const names = Array.from(selectedPoolSkills);
    if (names.length === 0) return;
    const confirmed = await new Promise<boolean>((resolve) => {
      Modal.confirm({
        title: t("skillPool.batchDeleteTitle", { count: names.length }),
        content: (
          <ul style={{ margin: "8px 0", paddingLeft: 20 }}>
            {names.map((n) => (
              <li key={n}>{n}</li>
            ))}
          </ul>
        ),
        okText: t("common.delete"),
        danger: true,
        cancelText: t("common.cancel"),
        onOk: () => resolve(true),
        onCancel: () => resolve(false),
      });
    });
    if (!confirmed) return;
    try {
      const { results } = await api.batchDeletePoolSkills(names);
      const failed = Object.entries(results).filter(([, r]) => !r.success);
      if (failed.length > 0) {
        toast.warning(
          t("skillPool.batchDeletePartial", {
            deleted: names.length - failed.length,
            failed: failed.length,
          }),
        );
      } else {
        toast.success(
          t("skillPool.batchDeleteSuccess", { count: names.length }),
        );
      }
      clearPoolSelection();

      await loadData(true);
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("skillPool.batchDeleteFailed"),
      );
    }
  };

  const poolMoreMenuItems: MenuProps["items"] = [
    {
      key: "broadcast",
      icon: <Send />,
      label: t("skillPool.broadcast"),
      onClick: () => openBroadcast(),
    },
    {
      key: "importBuiltin",
      icon: <RefreshCcw />,
      label: t("skillPool.updateBuiltin"),
      onClick: () => void openImportBuiltin(),
    },
    {
      key: "uploadZip",
      icon: <Download />,
      label: t("common.import"),
      onClick: () => zipInputRef.current?.click(),
    },
    {
      key: "batchOperation",
      icon: <Trash2 />,
      label: t("common.batchDelete"),
      onClick: toggleBatchMode,
      danger: true,
    },
  ];
  const headerMoreMenuItems: MenuProps["items"] = [
    ...poolMoreMenuItems,
    { type: "divider" },
    {
      key: "refresh",
      icon: <RefreshCw size={14} className={loading ? "icon-spin" : ""} />,
      label: t("common.refreshPage"),
      onClick: handleRefresh,
      disabled: loading,
    },
  ];

  const headerExtra: ReactNode = (
    <div className={styles.headerRight}>
      <input
        type="file"
        accept={ZIP_FILE_ACCEPT}
        ref={zipInputRef}
        onChange={handleZipImport}
        style={{ display: "none" }}
      />
      {poolBatchMode ? (
        <div className={styles.batchActions}>
          <span className={styles.batchCount}>
            {t("skills.selectedCount", {
              count: selectedPoolSkills.size,
            })}
          </span>
          <Btn variant="filled" onClick={selectAllPool}>
            {t("skills.selectAll")}
          </Btn>
          <Btn
            variant="danger-filled"
            icon={<Trash2 />}
            onClick={handleBatchDeletePool}
          >
            {t("common.delete")} ({selectedPoolSkills.size})
          </Btn>
          <Btn variant="filled" icon={<X />} onClick={toggleBatchMode} />
        </div>
      ) : isMobileMode ? (
        <>
          <Tooltip title={t("skills.createSkillHint")}>
            <Btn variant="filled" icon={<Plus />} onClick={openCreate} />
          </Tooltip>
          <Dropdown menu={{ items: headerMoreMenuItems }} trigger={["click"]}>
            <Btn variant="filled" icon={<MoreVertical />} />
          </Dropdown>
        </>
      ) : (
        <>
          <Tooltip title={t("skills.createSkillHint")}>
            <Btn variant="filled" icon={<Plus />} onClick={openCreate} />
          </Tooltip>
          <Dropdown menu={{ items: headerMoreMenuItems }} trigger={["click"]}>
            <Btn variant="filled" icon={<MoreVertical />} />
          </Dropdown>
        </>
      )}
    </div>
  );

  return (
    <Page className={styles.skillsPage}>
      {embedded && headerExtraContainer
        ? createPortal(headerExtra, headerExtraContainer)
        : null}
      {!embedded && (
        <PageHeader
          items={[{ title: t("nav.skillPool") }]}
          onRefresh={handleRefresh}
          refreshing={loading}
          showMoreMenu={!poolBatchMode}
          moreMenuItems={poolMoreMenuItems}
          extra={
            <div className={styles.headerRight}>
              <input
                type="file"
                accept={ZIP_FILE_ACCEPT}
                ref={zipInputRef}
                onChange={handleZipImport}
                style={{ display: "none" }}
              />
              {poolBatchMode ? (
                <div className={styles.batchActions}>
                  <span className={styles.batchCount}>
                    {t("skills.selectedCount", {
                      count: selectedPoolSkills.size,
                    })}
                  </span>
                  <Btn variant="filled" onClick={selectAllPool}>
                    {t("skills.selectAll")}
                  </Btn>
                  <Btn
                    variant="danger-filled"
                    icon={<Trash2 />}
                    onClick={handleBatchDeletePool}
                  >
                    {t("common.delete")} ({selectedPoolSkills.size})
                  </Btn>
                  <Btn
                    variant="filled"
                    icon={<X />}
                    onClick={toggleBatchMode}
                  />
                </div>
              ) : (
                <Tooltip title={t("skills.createSkillHint")}>
                  <Btn variant="filled" icon={<Plus />} onClick={openCreate} />
                </Tooltip>
              )}
            </div>
          }
        />
      )}

      {}
      <PageBody>
        {}
        {!loading && skills.length > 0 && (
          <div className={styles.toolbar}>
            <TextInput
              className={styles.searchInput}
              placeholder={t("skills.searchPlaceholder")}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
            />
            <Dropdown
              menu={{
                items: [
                  { key: "name-asc", label: t("skills.sortByName") + " A→Z" },
                  { key: "name-desc", label: t("skills.sortByName") + " Z→A" },
                  {
                    key: "lastUpdated-desc",
                    label: t("skills.sortByLastUpdated") + " ↓",
                  },
                  {
                    key: "lastUpdated-asc",
                    label: t("skills.sortByLastUpdated") + " ↑",
                  },
                ],
                selectedKeys: [`${sortField}-${sortOrder}`],
                onClick: ({ key }: { key: string }) => {
                  const idx = key.lastIndexOf("-");
                  setSortField(key.slice(0, idx) as "name" | "lastUpdated");
                  setSortOrder(key.slice(idx + 1) as "asc" | "desc");
                },
              }}
              trigger={["click"]}
            >
              <Btn variant="outline" icon={<ArrowUpNarrowWide />} />
            </Dropdown>
            <div className={styles.toolbarRight}>
              <div className={styles.viewToggle}>
                {!isMobileMode && (
                  <>
                    <Btn
                      icon={<List />}
                      variant={viewMode === "list" ? "filled" : "outline"}
                      onClick={() => setViewMode("list")}
                      title={t("skills.listView")}
                    />
                    <Btn
                      icon={<LayoutGrid />}
                      variant={viewMode === "card" ? "filled" : "outline"}
                      onClick={() => setViewMode("card")}
                      title={t("skills.gridView")}
                    />
                  </>
                )}
              </div>
            </div>
          </div>
        )}

        {loading && skills.length === 0 ? (
          <div className={styles.loading}>
            <span className={styles.loadingText}>{t("common.loading")}</span>
          </div>
        ) : effectiveViewMode === "card" ? (
          <div className={styles.skillsGrid}>
            {filteredSkills.map((skill) => (
              <SkillPoolCard
                key={skill.name}
                skill={skill}
                isSelected={selectedPoolSkills.has(skill.name)}
                batchMode={poolBatchMode}
                onToggleSelect={togglePoolSelect}
                onEdit={openEdit}
                onBroadcast={openBroadcast}
                onDelete={(s) => void handleDelete(s)}
              />
            ))}
          </div>
        ) : (
          <StackedList className={styles.skillsList}>
            {filteredSkills.map((skill) => {
              const isSelected = selectedPoolSkills.has(skill.name);
              return (
                <div
                  key={skill.name}
                  className={`${styles.skillListItem} ${
                    isSelected ? styles.selectedListItem : ""
                  }`}
                  onClick={() => {
                    if (poolBatchMode) {
                      togglePoolSelect(skill.name);
                    } else {
                      openEdit(skill);
                    }
                  }}
                >
                  {poolBatchMode && (
                    <Check
                      checked={isSelected}
                      onClick={(e) => {
                        e.stopPropagation();
                        togglePoolSelect(skill.name);
                      }}
                    />
                  )}
                  <div className={styles.listItemLeft}>
                    <span className={styles.fileIcon}>
                      {getSkillVisual(
                        skill.name,
                        skill.content,
                        skill.emoji,
                        poolSkillIconUrl(skill),
                      )}
                    </span>
                    <div className={styles.listItemHeader}>
                      <span className={styles.skillTitle}>{skill.name}</span>
                      {skill.install_meta?.installed_from && (
                        <Tag>{skill.install_meta.installed_from}</Tag>
                      )}
                    </div>
                    <p className={styles.listItemDesc}>
                      {skill.description || "-"}
                    </p>
                  </div>
                  <div className={styles.listItemRight}>
                    <Btn
                      aria-label={t("skillPool.broadcast")}
                      disabled={poolBatchMode}
                      icon={<Send />}
                      onClick={(e) => {
                        e.stopPropagation();
                        openBroadcast(skill);
                      }}
                    />
                    <Btn
                      aria-label={t("skillPool.delete")}
                      variant="danger"
                      disabled={poolBatchMode}
                      icon={<Trash2 />}
                      onClick={(e) => {
                        e.stopPropagation();
                        void handleDelete(skill);
                      }}
                    />
                  </div>
                </div>
              );
            })}
          </StackedList>
        )}
      </PageBody>

      <BroadcastModal
        open={mode === "broadcast"}
        skills={skills}
        workspaces={workspaces}
        initialSkillNames={broadcastInitialNames}
        onClose={closeModal}
        onConfirm={handleBroadcast}
      />

      <ImportBuiltinModal
        open={importBuiltinModalOpen}
        loading={importBuiltinLoading}
        sources={builtinSources}
        onClose={closeImportBuiltin}
        onConfirm={handleImportBuiltins}
      />

      <Drawer
        placement="right"
        title={
          mode === "edit" ? t("skills.viewSkill") : t("skills.createSkill")
        }
        open={mode === "create" || mode === "edit"}
        onClose={closeDrawer}
        destroyOnClose
        styles={{ body: { overflow: "auto" } }}
        footer={
          <>
            <Btn variant="outline" onClick={closeDrawer}>
              {t("common.cancel")}
            </Btn>
            <Btn onClick={handleSavePoolSkill}>
              {mode === "edit" ? t("common.save") : t("common.create")}
            </Btn>
          </>
        }
      >
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          {mode === "create" ? (
            <FormField
              label={t("common.name")}
              error={poolFormErrors.name?.message}
              required
            >
              <Controller
                name="name"
                control={poolFormControl}
                render={({ field }) => (
                  <TextInput
                    {...field}
                    placeholder={t("skills.skillNamePlaceholder")}
                  />
                )}
              />
            </FormField>
          ) : (
            <FormField label={t("common.name")}>
              <Controller
                name="name"
                control={poolFormControl}
                render={({ field }) => <TextInput {...field} />}
              />
            </FormField>
          )}

          <FormField error={poolFormErrors.content?.message}>
            <Controller
              name="content"
              control={poolFormControl}
              rules={{
                validate: async (value) => {
                  try {
                    await validateFrontmatter(undefined, value);
                    return true;
                  } catch (e) {
                    return e instanceof Error ? e.message : "Invalid";
                  }
                },
              }}
              render={() => (
                <MarkdownCopy
                  content={drawerContent}
                  showMarkdown={showMarkdown}
                  onShowMarkdownChange={setShowMarkdown}
                  showCopyButton={false}
                  editable={true}
                  onContentChange={handleDrawerContentChange}
                  markdownViewerProps={{
                    style: { height: "calc(100vh - 260px)" },
                  }}
                  textareaProps={{
                    ...(mode === "create" && {
                      placeholder: t("skills.contentPlaceholder"),
                    }),
                    style: { height: "calc(100vh - 260px)" },
                  }}
                />
              )}
            />
          </FormField>

          {mode === "edit" && activeSkill?.install_meta && (
            <div>
              <SkillInstallInfo meta={activeSkill.install_meta} />
              {activeSkill.install_meta.source_url && (
                <div style={{ marginTop: 8 }}>
                  <SkillUpdateButton
                    skillName={activeSkill.name}
                    scope="pool"
                    onUpdated={() => void loadData(true)}
                  />
                </div>
              )}
            </div>
          )}
        </div>
      </Drawer>

      {conflictRenameModal}
    </Page>
  );
}

export default SkillPoolPage;
