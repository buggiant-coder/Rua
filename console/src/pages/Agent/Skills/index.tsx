import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { PoolSkillSpec, SkillSpec } from "../../../api/types";
import {
  SkillCard,
  SkillDrawer,
  useConflictRenameModal,
  PoolTransferModal,
  getSkillVisual,
  skillIconUrl,
} from "./components";
import { useSkills } from "./useSkills";
import { useTranslation } from "react-i18next";
import { useAgentStore } from "../../../stores/agentStore";
import api, { agentsApi } from "../../../api";
import { parseErrorDetail } from "../../../utils/error";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { EmptyState } from "@/components/EmptyState";
import { isMobile } from "@/utils/isMobile";
import { getDownloadFolderHint, saveTextFile } from "@/utils/saveFile";
import { ZIP_FILE_ACCEPT } from "@/utils/fileInputAccept";
import styles from "./index.module.less";
import {
  ArrowLeftRight,
  ArrowUpNarrowWide,
  CloudDownload,
  CloudUpload,
  Download,
  LayoutGrid,
  List,
  MoreVertical,
  Plus,
  RefreshCw,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import {
  Btn,
  Check,
  Dropdown,
  Loading,
  Modal,
  StackedList,
  Tag,
  TextInput,
  Toggle,
  Tooltip,
  toast,
  type MenuProps,
} from "@/ui";
type DownloadConflict =
  | {
      skill_name: string;
      reason: "conflict";
    }
  | {
      skill_name: string;
      reason: "builtin_upgrade";
      current_version_text: string;
      source_version_text: string;
    };

interface SkillsPageProps {
  embedded?: boolean;
  headerExtraContainer?: HTMLElement | null;
}

function SkillsPage({
  embedded = false,
  headerExtraContainer,
}: SkillsPageProps) {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const {
    skills,
    loading,
    loadError,
    uploading,
    createSkill,
    uploadSkill,
    toggleEnabled,
    deleteSkill,
    refreshSkills,
    hardRefresh,
  } = useSkills();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [editingSkill, setEditingSkill] = useState<SkillSpec | null>(null);
  const [hoverKey, setHoverKey] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [poolSkills, setPoolSkills] = useState<PoolSkillSpec[]>([]);
  const [poolModal, setPoolModal] = useState<"upload" | "download" | null>(
    null,
  );
  const { showConflictRenameModal, conflictRenameModal } =
    useConflictRenameModal();
  const [selectedSkills, setSelectedSkills] = useState<Set<string>>(new Set());
  const [batchModeEnabled, setBatchModeEnabled] = useState(false);
  const batchMode = batchModeEnabled;
  const isMobileMode = useMemo(() => isMobile(), []);
  const VIEW_MODE_KEY = "agent.skills.viewMode";
  const [viewMode, setViewMode] = useState<"card" | "list">(() => {
    try {
      const v = localStorage.getItem(VIEW_MODE_KEY);
      if (v === "card" || v === "list") return v;
    } catch {}
    return "card";
  });

  const effectiveViewMode: "card" | "list" = isMobileMode ? "card" : viewMode;
  const [searchQuery, setSearchQuery] = useState("");
  type SkillSortField = "name" | "lastUpdated" | "enabled";
  const SORT_FIELD_KEY = "agent.skills.sortField";
  const SORT_ORDER_KEY = "agent.skills.sortOrder";
  const [sortField, setSortField] = useState<SkillSortField>(() => {
    try {
      const v = localStorage.getItem(SORT_FIELD_KEY);
      if (v === "name" || v === "lastUpdated" || v === "enabled") return v;
    } catch {}
    return "name";
  });
  const [sortOrder, setSortOrder] = useState<"asc" | "desc">(() => {
    try {
      const v = localStorage.getItem(SORT_ORDER_KEY);
      if (v === "asc" || v === "desc") return v;
    } catch {}
    return "asc";
  });

  useEffect(() => {
    try {
      localStorage.setItem(SORT_FIELD_KEY, sortField);
    } catch {}
  }, [sortField]);
  useEffect(() => {
    try {
      localStorage.setItem(SORT_ORDER_KEY, sortOrder);
    } catch {}
  }, [sortOrder]);
  useEffect(() => {
    try {
      localStorage.setItem(VIEW_MODE_KEY, viewMode);
    } catch {}
  }, [viewMode]);

  const filteredSkills = useMemo(() => {
    const query = searchQuery.toLowerCase();
    const filtered = skills.filter((skill) => {
      return (
        skill.name.toLowerCase().includes(query) ||
        (skill.description && skill.description.toLowerCase().includes(query))
      );
    });

    return filtered.slice().sort((a, b) => {
      let cmp: number;
      if (sortField === "lastUpdated") {
        const ta = a.last_updated ? new Date(a.last_updated).getTime() : 0;
        const tb = b.last_updated ? new Date(b.last_updated).getTime() : 0;
        cmp = ta - tb;
      } else if (sortField === "enabled") {
        const ea = a.enabled ? 1 : 0;
        const eb = b.enabled ? 1 : 0;
        cmp = eb - ea;
        if (cmp === 0) cmp = a.name.localeCompare(b.name);
      } else {
        cmp = a.name.localeCompare(b.name);
      }
      return sortOrder === "asc" ? cmp : -cmp;
    });
  }, [skills, searchQuery, sortField, sortOrder]);

  const confirmOverwrite = (title: string, content: ReactNode) =>
    new Promise<boolean>((resolve) => {
      Modal.confirm({
        title,
        content,
        okText: t("common.confirm"),
        cancelText: t("common.cancel"),
        onOk: () => resolve(true),
        onCancel: () => resolve(false),
      });
    });

  const toggleSelect = (name: string) => {
    setSelectedSkills((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };

  const clearSelection = () => {
    setSelectedSkills(new Set());
  };

  const selectAll = () => setSelectedSkills(new Set(skills.map((s) => s.name)));

  const MAX_UPLOAD_SIZE_MB = 100;

  const toggleBatchMode = () => {
    if (batchModeEnabled) {
      clearSelection();
      setBatchModeEnabled(false);
    } else {
      setBatchModeEnabled(true);
    }
  };

  useEffect(() => {
    if (poolModal === "upload" || poolModal === "download") {
      void api
        .listSkillPoolSkills()
        .then(setPoolSkills)
        .catch(() => undefined);
    }
  }, [poolModal]);

  const closePoolModal = () => {
    setPoolModal(null);
  };

  const handleUploadClick = () => {
    fileInputRef.current?.click();
  };

  const handleFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    e.target.value = "";

    if (!file.name.toLowerCase().endsWith(".zip")) {
      toast.warning(t("skills.zipOnly"));
      return;
    }

    const sizeMB = file.size / (1024 * 1024);
    if (sizeMB > MAX_UPLOAD_SIZE_MB) {
      toast.warning(
        t("skills.fileSizeExceeded", {
          limit: MAX_UPLOAD_SIZE_MB,
          size: sizeMB.toFixed(1),
        }),
      );
      return;
    }

    let renameMap: Record<string, string> | undefined;
    while (true) {
      const result = await uploadSkill(file, undefined, renameMap);
      if (result.success || !result.conflict) break;

      const conflicts = Array.isArray(result.conflict.conflicts)
        ? result.conflict.conflicts
        : [];
      if (conflicts.length === 0) break;

      const newRenames = await showConflictRenameModal(
        conflicts.map((c: { skill_name: string; suggested_name: string }) => ({
          key: c.skill_name,
          label: c.skill_name,
          suggested_name: c.suggested_name,
        })),
      );
      if (!newRenames) break;
      renameMap = { ...renameMap, ...newRenames };
    }
  };

  const handleCreate = () => {
    setEditingSkill(null);
    setDrawerOpen(true);
  };

  const handleEdit = (skill: SkillSpec) => {
    setEditingSkill(skill);
    setDrawerOpen(true);
  };

  const handleToggleEnabled = async (skill: SkillSpec, e: React.MouseEvent) => {
    e.stopPropagation();
    await toggleEnabled(skill);
    await refreshSkills();
  };

  const handleDelete = async (skill: SkillSpec, e?: React.MouseEvent) => {
    e?.stopPropagation();
    await deleteSkill(skill);
  };

  const handleDrawerClose = () => {
    setDrawerOpen(false);
    setEditingSkill(null);
  };

  const handleSubmit = async (values: SkillSpec) => {
    if (editingSkill) {
      const sourceName = editingSkill.name;
      const targetName = values.name;
      const saveEditedSkill = async (overwrite = false) => {
        const result = await agentsApi.saveAgentSkill(selectedAgent, {
          name: targetName,
          content: values.content,
          source_name: sourceName !== targetName ? sourceName : undefined,
          config: values.config,
          overwrite,
        });
        const sideUpdates: Promise<unknown>[] = [];
        const newChannels = values.channels || ["all"];
        if (
          JSON.stringify(newChannels) !==
          JSON.stringify(editingSkill.channels || ["all"])
        ) {
          sideUpdates.push(
            agentsApi.updateAgentSkillChannels(
              selectedAgent,
              result.name,
              newChannels,
            ),
          );
        }
        await Promise.all(sideUpdates);
        if (result.mode === "noop") {
          setDrawerOpen(false);
          await refreshSkills();
          return;
        }
        toast.success(
          result.mode === "rename"
            ? `${t("common.save")}: ${result.name}`
            : t("common.save"),
        );
        setDrawerOpen(false);
        await refreshSkills();
      };
      try {
        await saveEditedSkill();
      } catch (error) {
        const detail = parseErrorDetail(error);
        if (detail?.reason === "conflict") {
          const confirmed = await confirmOverwrite(
            t("skillPool.overwriteConfirm"),
            <div style={{ display: "grid", gap: 8 }}>
              <div>{t("skills.overwriteExistingList")}</div>
              <ul style={{ margin: 0, paddingLeft: 20 }}>
                <li>{targetName}</li>
              </ul>
            </div>,
          );
          if (!confirmed) return;
          try {
            await saveEditedSkill(true);
          } catch (retryError) {
            toast.error(
              retryError instanceof Error
                ? retryError.message
                : t("common.save"),
            );
          }
        } else {
          toast.error(
            error instanceof Error ? error.message : t("common.save"),
          );
        }
      }
    } else {
      const submitName = values.name;
      const result = await createSkill(
        submitName,
        values.content,
        values.config,
        true,
      );
      if (result.success) {
        await agentsApi.updateAgentSkillChannels(
          selectedAgent,
          submitName,
          values.channels || ["all"],
        );
        setDrawerOpen(false);
        await refreshSkills();
        return;
      }
      if (result.conflict?.suggested_name) {
        const renameMap = await showConflictRenameModal([
          {
            key: submitName,
            label: submitName,
            suggested_name: result.conflict!.suggested_name,
          },
        ]);
        if (renameMap) {
          const newName = Object.values(renameMap)[0];
          if (newName) {
            await handleSubmit({ ...values, name: newName });
          }
        }
      }
    }
  };

  const handleUploadToPool = async (workspaceSkillNames: string[]) => {
    if (workspaceSkillNames.length === 0) return;
    try {
      const conflictingNames: string[] = [];
      for (const skillName of workspaceSkillNames) {
        try {
          await api.uploadWorkspaceSkillToPool({
            workspace_id: selectedAgent,
            skill_name: skillName,
            preview_only: true,
          });
        } catch (error) {
          const detail = parseErrorDetail(error);
          if (detail?.reason === "conflict") {
            conflictingNames.push(skillName);
            continue;
          }
          throw error;
        }
      }
      if (conflictingNames.length > 0) {
        const confirmed = await confirmOverwrite(
          t("skillPool.overwriteConfirm"),
          <div style={{ display: "grid", gap: 8 }}>
            <div>{t("skills.overwriteExistingList")}</div>
            <ul style={{ margin: 0, paddingLeft: 20 }}>
              {conflictingNames.map((name) => (
                <li key={name}>{name}</li>
              ))}
            </ul>
          </div>,
        );
        if (!confirmed) return;
      }
      for (const skillName of workspaceSkillNames) {
        await api.uploadWorkspaceSkillToPool({
          workspace_id: selectedAgent,
          skill_name: skillName,
          overwrite: conflictingNames.includes(skillName),
        });
      }
      toast.success(t("skills.uploadedToPool"));
      closePoolModal();
      await refreshSkills();
      setPoolSkills(await api.listSkillPoolSkills());
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("skills.uploadFailed"),
      );
    }
  };

  const handleDownloadFromPool = async (poolSkillNames: string[]) => {
    if (poolSkillNames.length === 0) return;
    try {
      const conflicts: DownloadConflict[] = [];
      for (const skillName of poolSkillNames) {
        try {
          await api.downloadSkillPoolSkill({
            skill_name: skillName,
            targets: [{ workspace_id: selectedAgent }],
            preview_only: true,
          });
        } catch (error) {
          const detail = parseErrorDetail(error);
          const returnedConflicts = Array.isArray(detail?.conflicts)
            ? detail.conflicts
            : [];
          if (!returnedConflicts.length) {
            throw error;
          }
          conflicts.push(
            ...returnedConflicts.map((conflict) =>
              conflict?.reason === "builtin_upgrade"
                ? {
                    skill_name: conflict.skill_name || skillName,
                    reason: "builtin_upgrade" as const,
                    current_version_text: conflict.current_version_text || "",
                    source_version_text: conflict.source_version_text || "",
                  }
                : {
                    skill_name: conflict?.skill_name || skillName,
                    reason: "conflict" as const,
                  },
            ),
          );
        }
      }
      if (conflicts.length > 0) {
        const allBuiltinUpgrades = conflicts.every(
          (conflict) => conflict.reason === "builtin_upgrade",
        );
        const confirmed = await confirmOverwrite(
          allBuiltinUpgrades
            ? t("skills.builtinUpgradeTitle")
            : t("skillPool.overwriteConfirm"),
          <div style={{ display: "grid", gap: 8 }}>
            <div>
              {allBuiltinUpgrades
                ? t("skillPool.builtinOverwriteTargetsContent")
                : t("skills.overwriteExistingList")}
            </div>
            {conflicts.map((conflict) => (
              <div key={conflict.skill_name}>
                <strong>{conflict.skill_name}</strong>
                {conflict.reason === "builtin_upgrade" ? (
                  <>
                    {"  "}
                    {t("skillPool.currentVersion")}:{" "}
                    {conflict.current_version_text || "-"}
                    {"  ->  "}
                    {t("skillPool.sourceVersion")}:{" "}
                    {conflict.source_version_text || "-"}
                  </>
                ) : null}
              </div>
            ))}
          </div>,
        );
        if (!confirmed) return;
      }
      for (const skillName of poolSkillNames) {
        const shouldOverwrite = conflicts.some(
          (conflict) => conflict.skill_name === skillName,
        );
        await api.downloadSkillPoolSkill({
          skill_name: skillName,
          targets: [{ workspace_id: selectedAgent }],
          overwrite: shouldOverwrite,
        });
      }
      toast.success(t("skills.downloadedToWorkspace"));
      closePoolModal();
      await refreshSkills();
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("common.download") + " failed",
      );
    }
  };

  const handleBatchDelete = async () => {
    const names = Array.from(selectedSkills);
    if (names.length === 0) return;
    const confirmed = await new Promise<boolean>((resolve) => {
      Modal.confirm({
        title: t("skills.batchDeleteTitle", { count: names.length }),
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
      const { results } = await agentsApi.batchDeleteAgentSkills(
        selectedAgent,
        names,
      );
      const failed = Object.entries(results).filter(([, r]) => !r.success);
      if (failed.length > 0) {
        toast.warning(
          t("skills.batchDeletePartial", {
            deleted: names.length - failed.length,
            failed: failed.length,
          }),
        );
      } else {
        toast.success(t("skills.batchDeleteSuccess", { count: names.length }));
      }
      clearSelection();
      await refreshSkills();
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("skills.batchDeleteFailed"),
      );
    }
  };

  const handleExportSkills = async () => {
    const payload = {
      workspace_id: selectedAgent,
      exported_at: new Date().toISOString(),
      skills: filteredSkills.map((skill) => ({
        name: skill.name,
        description: skill.description,
        content: skill.content,
        config: skill.config,
        channels: skill.channels,
        enabled: skill.enabled,
        source: skill.source,
      })),
    };
    const filename = `skills-${selectedAgent}.json`;
    const result = await saveTextFile(
      JSON.stringify(payload, null, 2),
      filename,
      "application/json;charset=utf-8",
    );
    if (result.ok) {
      const hint = result.usedPicker ? "" : getDownloadFolderHint();
      toast.success(
        hint
          ? `${t("common.export")} (${hint}${filename})`
          : t("common.export"),
      );
    }
  };

  const moreMenuItems: MenuProps["items"] = [
    {
      key: "downloadFromPool",
      icon: <CloudDownload />,
      label: t("skills.downloadFromPool"),
      onClick: () => setPoolModal("download"),
    },
    {
      key: "uploadToPool",
      icon: <CloudUpload />,
      label: t("skills.uploadToPool"),
      onClick: () => setPoolModal("upload"),
    },
    {
      key: "uploadZip",
      icon: <Download />,
      label: t("common.import"),
      onClick: handleUploadClick,
      disabled: uploading,
    },
    {
      key: "export",
      icon: <Upload />,
      label: t("common.export"),
      onClick: handleExportSkills,
      disabled: filteredSkills.length === 0,
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
    ...moreMenuItems,
    { type: "divider" },
    {
      key: "refresh",
      icon: <RefreshCw size={14} className={loading ? "icon-spin" : ""} />,
      label: t("common.refreshPage"),
      onClick: hardRefresh,
      disabled: loading,
    },
  ];

  const headerExtra = (
    <div className={styles.headerRight}>
      <input
        type="file"
        accept={ZIP_FILE_ACCEPT}
        ref={fileInputRef}
        onChange={handleFileChange}
        style={{ display: "none" }}
      />
      {batchMode ? (
        <div className={styles.batchActions}>
          <span className={styles.batchCount}>
            {t("skills.selectedCount", {
              count: selectedSkills.size,
            })}
          </span>
          <Btn variant="filled" onClick={selectAll}>
            {t("skills.selectAll")}
          </Btn>
          <Btn
            variant="filled"
            onClick={() => {
              const names = Array.from(selectedSkills);
              if (names.length === 0) return;
              clearSelection();
              void handleUploadToPool(names);
            }}
            icon={<ArrowLeftRight />}
          >
            {t("skills.uploadToPool")}
          </Btn>
          <Btn
            variant="danger-filled"
            icon={<Trash2 />}
            onClick={handleBatchDelete}
          >
            {t("common.delete")} ({selectedSkills.size})
          </Btn>
          <Btn variant="filled" icon={<X />} onClick={toggleBatchMode} />
        </div>
      ) : isMobileMode ? (
        <>
          <Tooltip title={t("skills.createSkillHint")}>
            <Btn variant="filled" onClick={handleCreate} icon={<Plus />} />
          </Tooltip>
          <Dropdown menu={{ items: headerMoreMenuItems }} trigger={["click"]}>
            <Btn variant="filled" icon={<MoreVertical />} />
          </Dropdown>
        </>
      ) : (
        <>
          <Tooltip title={t("skills.createSkillHint")}>
            <Btn variant="filled" onClick={handleCreate} icon={<Plus />} />
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
          agentScoped
          items={[{ title: t("skills.title") }]}
          onRefresh={hardRefresh}
          refreshing={loading}
          showMoreMenu={!batchMode}
          moreMenuItems={moreMenuItems}
          extra={
            <div className={styles.headerRight}>
              <input
                type="file"
                accept={ZIP_FILE_ACCEPT}
                ref={fileInputRef}
                onChange={handleFileChange}
                style={{ display: "none" }}
              />
              {batchMode ? (
                <div className={styles.batchActions}>
                  <span className={styles.batchCount}>
                    {t("skills.selectedCount", {
                      count: selectedSkills.size,
                    })}
                  </span>
                  <Btn variant="filled" onClick={selectAll}>
                    {t("skills.selectAll")}
                  </Btn>
                  <Btn
                    variant="filled"
                    onClick={() => {
                      const names = Array.from(selectedSkills);
                      if (names.length === 0) return;
                      clearSelection();
                      void handleUploadToPool(names);
                    }}
                    icon={<ArrowLeftRight />}
                  >
                    {t("skills.uploadToPool")}
                  </Btn>
                  <Btn
                    variant="danger-filled"
                    icon={<Trash2 />}
                    onClick={handleBatchDelete}
                  >
                    {t("common.delete")} ({selectedSkills.size})
                  </Btn>
                  <Btn
                    variant="filled"
                    icon={<X />}
                    onClick={toggleBatchMode}
                  />
                </div>
              ) : (
                <Tooltip title={t("skills.createSkillHint")}>
                  <Btn
                    variant="filled"
                    onClick={handleCreate}
                    icon={<Plus />}
                  />
                </Tooltip>
              )}
            </div>
          }
        />
      )}

      <PageBody>
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
                  {
                    key: "enabled-desc",
                    label: t("skills.sortByEnabled") + " ↓",
                  },
                  {
                    key: "enabled-asc",
                    label: t("skills.sortByEnabled") + " ↑",
                  },
                ],
                selectedKeys: [`${sortField}-${sortOrder}`],
                onClick: ({ key }: { key: string }) => {
                  const idx = key.lastIndexOf("-");
                  setSortField(key.slice(0, idx) as SkillSortField);
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
            <Loading size="large" tip={t("common.loading")} />
          </div>
        ) : loadError && skills.length === 0 ? (
          <EmptyState
            variant="error"
            description={loadError}
            onRetry={hardRefresh}
          />
        ) : skills.length === 0 ? (
          <EmptyState description={t("skills.emptyStateTitle")} />
        ) : filteredSkills.length === 0 ? (
          <EmptyState variant="no-results" />
        ) : effectiveViewMode === "card" ? (
          <div className={styles.skillsGrid}>
            {filteredSkills.map((skill) => (
              <SkillCard
                key={skill.name}
                skill={skill}
                isHover={hoverKey === skill.name}
                selected={
                  batchMode ? selectedSkills.has(skill.name) : undefined
                }
                onSelect={() => toggleSelect(skill.name)}
                onClick={() => handleEdit(skill)}
                onMouseEnter={() => setHoverKey(skill.name)}
                onMouseLeave={() => setHoverKey(null)}
                onToggleEnabled={(e) => handleToggleEnabled(skill, e)}
                onDelete={(e) => handleDelete(skill, e)}
              />
            ))}
          </div>
        ) : (
          <StackedList className={styles.skillsList}>
            {filteredSkills.map((skill) => {
              const channels = skill.channels?.join(", ") || "all";
              const showChannels = channels !== "all";
              const isSelected = selectedSkills.has(skill.name);
              return (
                <div
                  key={skill.name}
                  className={`${styles.skillListItem} ${
                    isSelected ? styles.selectedListItem : ""
                  }`}
                  onClick={() => {
                    if (batchMode) {
                      toggleSelect(skill.name);
                    } else {
                      handleEdit(skill);
                    }
                  }}
                >
                  {batchMode && (
                    <Check
                      checked={isSelected}
                      onClick={(e) => {
                        e.stopPropagation();
                        toggleSelect(skill.name);
                      }}
                    />
                  )}
                  <div className={styles.listItemLeft}>
                    <span className={styles.fileIcon}>
                      {getSkillVisual(
                        skill.name,
                        skill.content,
                        skill.emoji,
                        skillIconUrl(skill),
                      )}
                    </span>
                    <div className={styles.listItemHeader}>
                      <span className={styles.skillTitle}>{skill.name}</span>
                      {skill.install_meta?.installed_from && (
                        <Tag>{skill.install_meta.installed_from}</Tag>
                      )}
                      {showChannels && (
                        <span className={styles.channelBadge}>{channels}</span>
                      )}
                    </div>
                    <p className={styles.listItemDesc}>
                      {skill.description || "-"}
                    </p>
                  </div>
                  <div
                    className={styles.listItemRight}
                    onClick={(e) => e.stopPropagation()}
                  >
                    <Toggle
                      checked={skill.enabled}
                      disabled={batchMode}
                      onClick={(e) => e.stopPropagation()}
                      onChange={async (e) => {
                        e.stopPropagation();
                        await toggleEnabled(skill);
                        await refreshSkills();
                      }}
                    />
                    <Btn
                      variant="danger"
                      icon={<Trash2 />}
                      disabled={batchMode}
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDelete(skill);
                      }}
                    />
                  </div>
                </div>
              );
            })}
          </StackedList>
        )}
      </PageBody>

      <PoolTransferModal
        mode={poolModal}
        skills={skills}
        poolSkills={poolSkills}
        onClose={closePoolModal}
        onUpload={handleUploadToPool}
        onDownload={handleDownloadFromPool}
      />

      {conflictRenameModal}

      <SkillDrawer
        open={drawerOpen}
        editingSkill={editingSkill}
        agentId={selectedAgent}
        onClose={handleDrawerClose}
        onSubmit={handleSubmit}
      />
    </Page>
  );
}

export default SkillsPage;
