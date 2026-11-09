/* eslint-disable react-refresh/only-export-components */
import { useState, useRef, useEffect, useCallback, useMemo } from "react";
import { useLoaderData, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { agentsApi } from "../../../api/modules/agents";
import { skillApi } from "../../../api/modules/skill";
import { subagentTemplatesApi } from "../../../api/modules/subagentTemplates";
import { agentTemplatesApi } from "@/api/modules/agentTemplates";
import type { AgentSummary } from "../../../api/types/agents";
import type { SubAgentTemplate } from "../../../api/types/subagentTemplates";
import type { AgentTemplate } from "@/api/types/agentTemplates";
import { useAgentStore } from "../../../stores/agentStore";
import { useAgents, type AgentsInitialData } from "./useAgents";
import {
  AgentCardGrid,
  AgentModal,
  SubAgentCardGrid,
  SubAgentModal,
  TemplateCardGrid,
} from "./components";
import type { AgentFormValues } from "./components/AgentModal";
import type { SubAgentFormValues } from "./components/SubAgentModal";
import { TemplateModal } from "../AgentTemplates/components/TemplateModal";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { reorderAgents } from "./reorder";
import styles from "./index.module.less";
import { Plus, Trash2 } from "lucide-react";
import { Btn, Modal, Tabs, Tooltip, toast } from "@/ui";

type TabKey = "agents" | "subagents" | "templates";

const VALID_TABS: TabKey[] = ["agents", "subagents", "templates"];

interface AgentsPageProps {
  initialData?: AgentsInitialData;
}

export async function loader(): Promise<AgentsInitialData> {
  const response = await agentsApi.listAgents();
  return { agents: response.agents };
}

export function RouteComponent() {
  return <AgentsPage initialData={useLoaderData() as AgentsInitialData} />;
}

export default function AgentsPage({ initialData }: AgentsPageProps) {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab") as TabKey | null;
  const initialTab: TabKey =
    tabParam && VALID_TABS.includes(tabParam) ? tabParam : "agents";
  const [activeTab, setActiveTab] = useState<TabKey>(initialTab);

  useEffect(() => {
    if (tabParam && VALID_TABS.includes(tabParam) && tabParam !== activeTab) {
      setActiveTab(tabParam);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabParam]);

  const [selectMode, setSelectMode] = useState(false);
  const [selectedAgentIds, setSelectedAgentIds] = useState<Set<string>>(
    new Set(),
  );
  const [selectedSubAgentIds, setSelectedSubAgentIds] = useState<Set<string>>(
    new Set(),
  );
  const [selectedTemplateIds, setSelectedTemplateIds] = useState<Set<string>>(
    new Set(),
  );

  const handleTabChange = (key: string) => {
    const next = key as TabKey;
    setActiveTab(next);

    setSelectMode(false);
    setSelectedAgentIds(new Set());
    setSelectedSubAgentIds(new Set());
    setSelectedTemplateIds(new Set());
    const params = new URLSearchParams(searchParams);
    if (next === "agents") {
      params.delete("tab");
    } else {
      params.set("tab", next);
    }
    setSearchParams(params, { replace: true });
  };

  const enterSelectMode = useCallback(() => {
    setSelectMode(true);
    setSelectedAgentIds(new Set());
    setSelectedSubAgentIds(new Set());
    setSelectedTemplateIds(new Set());
  }, []);

  const exitSelectMode = useCallback(() => {
    setSelectMode(false);
    setSelectedAgentIds(new Set());
    setSelectedSubAgentIds(new Set());
    setSelectedTemplateIds(new Set());
  }, []);

  const toggleAgentSelect = useCallback((id: string) => {
    setSelectedAgentIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const toggleSubAgentSelect = useCallback((id: string) => {
    setSelectedSubAgentIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const toggleTemplateSelect = useCallback((id: string) => {
    setSelectedTemplateIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const { agents, loading, deleteAgent, toggleAgent, loadAgents, setAgents } =
    useAgents(initialData);
  const { selectedAgent, setSelectedAgent } = useAgentStore();
  const [modalVisible, setModalVisible] = useState(false);
  const [editingAgent, setEditingAgent] = useState<AgentSummary | null>(null);
  const [reordering, setReordering] = useState(false);
  const [agentConfig, setAgentConfig] =
    useState<Partial<AgentFormValues> | null>(null);
  const [selectedSkills, setSelectedSkills] = useState<string[]>([]);
  const installedSkillsRef = useRef<string[]>([]);

  const [subAgentTemplates, setSubAgentTemplates] = useState<
    SubAgentTemplate[]
  >([]);
  const [subAgentLoading, setSubAgentLoading] = useState(false);
  const [subAgentModalVisible, setSubAgentModalVisible] = useState(false);
  const [editingSubAgent, setEditingSubAgent] =
    useState<SubAgentTemplate | null>(null);

  const loadSubAgentTemplates = useCallback(async () => {
    setSubAgentLoading(true);
    try {
      const templates = await subagentTemplatesApi.list();
      setSubAgentTemplates(templates);
    } catch (error) {
      console.error("Failed to load sub-agent templates:", error);
    } finally {
      setSubAgentLoading(false);
    }
  }, []);

  useEffect(() => {
    loadSubAgentTemplates();
  }, [loadSubAgentTemplates]);

  const [agentTemplates, setAgentTemplates] = useState<AgentTemplate[]>([]);
  const [templateLoading, setTemplateLoading] = useState(false);
  const [templateModalVisible, setTemplateModalVisible] = useState(false);
  const [editingTemplate, setEditingTemplate] = useState<AgentTemplate | null>(
    null,
  );

  const loadAgentTemplates = useCallback(async () => {
    setTemplateLoading(true);
    try {
      const data = await agentTemplatesApi.list();
      setAgentTemplates(data);
    } catch (err) {
      console.error("Failed to load agent templates:", err);
      toast.error(t("template.loadFailed"));
    } finally {
      setTemplateLoading(false);
    }
  }, [t]);

  useEffect(() => {
    loadAgentTemplates();
  }, [loadAgentTemplates]);

  const handleCreate = () => {
    setEditingAgent(null);
    setAgentConfig(null);
    setSelectedSkills([]);
    installedSkillsRef.current = [];
    setModalVisible(true);
  };

  const handleEdit = async (agent: AgentSummary) => {
    try {
      setSelectedSkills([]);
      installedSkillsRef.current = [];
      const config = await agentsApi.getAgent(agent.id);
      setEditingAgent(agent);
      setAgentConfig(config);
      setModalVisible(true);
    } catch (error) {
      console.error("Failed to load agent config:", error);
      toast.error(t("agent.loadConfigFailed"));
    }
  };

  const handleDelete = async (agentId: string) => {
    try {
      await deleteAgent(agentId);
      if (selectedAgent === agentId) {
        setSelectedAgent("default");
        toast.info(t("agent.switchedToDefault"));
      }
    } catch {
      toast.error(t("agent.deleteFailed"));
    }
  };

  const handleToggle = async (agentId: string, currentEnabled: boolean) => {
    const newEnabled = !currentEnabled;
    try {
      await toggleAgent(agentId, newEnabled);
      if (!newEnabled && selectedAgent === agentId) {
        setSelectedAgent("default");
        toast.info(t("agent.switchedToDefault"));
      }
    } catch {}
  };

  const handleInstalledSkillsLoaded = useCallback((skills: string[]) => {
    installedSkillsRef.current = skills;
  }, []);

  const handleSubmit = async (values: AgentFormValues) => {
    try {
      const workspaceRaw = values.workspace_dir;
      const workspace_dir =
        typeof workspaceRaw === "string"
          ? workspaceRaw.trim() || undefined
          : workspaceRaw;
      const payload = { ...values, workspace_dir };

      if (editingAgent) {
        let currentId = editingAgent.id;
        const newId = payload.id?.trim();
        if (newId && newId !== editingAgent.id) {
          await agentsApi.renameAgent(editingAgent.id, newId);
          toast.success(t("agent.renameSuccess"));
          if (selectedAgent === editingAgent.id) {
            setSelectedAgent(newId);
          }
          currentId = newId;
        }

        const previousInstalledSkills = installedSkillsRef.current;
        const newSkills = selectedSkills.filter(
          (skill) => !previousInstalledSkills.includes(skill),
        );
        const removedSkills = previousInstalledSkills.filter(
          (skill) => !selectedSkills.includes(skill),
        );

        for (const skill of newSkills) {
          await skillApi.downloadSkillPoolSkill({
            skill_name: skill,
            targets: [{ workspace_id: currentId }],
          });
        }
        for (const skill of removedSkills) {
          await skillApi.deleteSkill(skill, currentId);
        }
        await agentsApi.updateAgent(currentId, payload);
        installedSkillsRef.current = [...selectedSkills];
        toast.success(t("agent.updateSuccess"));
      } else {
        const idRaw = payload.id?.trim();
        const result = await agentsApi.createAgent({
          ...payload,
          id: idRaw || undefined,
          skill_names: selectedSkills,
        });
        toast.success(`${t("agent.createSuccess")} (ID: ${result.id})`);
      }

      setModalVisible(false);
      await loadAgents();
    } catch (error: unknown) {
      console.error("Failed to save agent:", error);
      toast.error(
        error instanceof Error ? error.message : t("agent.saveFailed"),
      );
    }
  };

  const handleReorder = async (activeId: string, overId: string) => {
    const nextAgents = reorderAgents(agents, activeId, overId);
    if (nextAgents === agents) return;

    const previousAgents = agents;
    setAgents(nextAgents);
    setReordering(true);

    try {
      await agentsApi.reorderAgents(nextAgents.map((agent) => agent.id));
      toast.success(t("agent.reorderSuccess"));
    } catch (error) {
      console.error("Failed to reorder agents:", error);
      setAgents(previousAgents);
      toast.error(t("agent.reorderFailed"));
    } finally {
      setReordering(false);
    }
  };

  const handleCreateSubAgent = () => {
    setEditingSubAgent(null);
    setSubAgentModalVisible(true);
  };

  const handleEditSubAgent = (template: SubAgentTemplate) => {
    setEditingSubAgent(template);
    setSubAgentModalVisible(true);
  };

  const handleDeleteSubAgent = async (templateId: string) => {
    try {
      await subagentTemplatesApi.delete(templateId);
      toast.success(t("subAgent.deleteSuccess"));
      await loadSubAgentTemplates();
    } catch (error: unknown) {
      toast.error(
        error instanceof Error ? error.message : t("subAgent.saveFailed"),
      );
    }
  };

  const handleSubAgentSubmit = async (values: SubAgentFormValues) => {
    try {
      if (editingSubAgent) {
        await subagentTemplatesApi.update(editingSubAgent.id, values);
      } else {
        await subagentTemplatesApi.create(values);
      }
      toast.success(t("subAgent.saveSuccess"));
      setSubAgentModalVisible(false);
      await loadSubAgentTemplates();
    } catch (error: unknown) {
      console.error("Failed to save sub-agent template:", error);
      toast.error(
        error instanceof Error ? error.message : t("subAgent.saveFailed"),
      );
    }
  };

  const handleCreateTemplate = () => {
    setEditingTemplate(null);
    setTemplateModalVisible(true);
  };

  const handleEditTemplate = async (template: AgentTemplate) => {
    try {
      const detail = await agentTemplatesApi.get(template.id);
      setEditingTemplate(detail);
      setTemplateModalVisible(true);
    } catch {
      toast.error(t("template.loadFailed"));
    }
  };

  const handleDeleteTemplate = async (templateId: string) => {
    try {
      await agentTemplatesApi.delete(templateId);
      toast.success(t("template.deleteSuccess"));
      await loadAgentTemplates();
    } catch (err: unknown) {
      toast.error(
        err instanceof Error ? err.message : t("template.deleteFailed"),
      );
    }
  };

  const handleSaveTemplate = async (values: {
    name: string;
    description: string;
    prompt_files: string[];
  }) => {
    try {
      if (editingTemplate) {
        await agentTemplatesApi.update(editingTemplate.id, {
          name: values.name,
          description: values.description,
          prompt_files: values.prompt_files,
        });
        toast.success(t("template.updateSuccess"));
      } else {
        await agentTemplatesApi.create(values);
        toast.success(t("template.createSuccess"));
      }
      setTemplateModalVisible(false);
      await loadAgentTemplates();
    } catch (err: unknown) {
      toast.error(
        err instanceof Error ? err.message : t("template.saveFailed"),
      );
    }
  };

  const currentSelectionSize = useMemo(() => {
    if (activeTab === "agents") return selectedAgentIds.size;
    if (activeTab === "subagents") return selectedSubAgentIds.size;
    return selectedTemplateIds.size;
  }, [activeTab, selectedAgentIds, selectedSubAgentIds, selectedTemplateIds]);

  const confirmBatchDelete = useCallback(() => {
    if (currentSelectionSize === 0) return;
    Modal.confirm({
      title: t("common.batchDeleteTitle"),
      content: t("common.batchDeleteConfirm", { count: currentSelectionSize }),
      okText: t("common.delete"),
      danger: true,
      cancelText: t("common.cancel"),
      onOk: async () => {
        let deleted = 0;
        let failed = 0;

        if (activeTab === "agents") {
          const ids = Array.from(selectedAgentIds).filter(
            (id) => id !== "default",
          );
          for (const id of ids) {
            try {
              await agentsApi.deleteAgent(id);
              if (selectedAgent === id) setSelectedAgent("default");
              deleted++;
            } catch {
              failed++;
            }
          }
          await loadAgents();
        } else if (activeTab === "subagents") {
          const builtinIds = new Set(
            subAgentTemplates.filter((t) => t.builtin).map((t) => t.id),
          );
          const ids = Array.from(selectedSubAgentIds).filter(
            (id) => !builtinIds.has(id),
          );
          for (const id of ids) {
            try {
              await subagentTemplatesApi.delete(id);
              deleted++;
            } catch {
              failed++;
            }
          }
          await loadSubAgentTemplates();
        } else {
          const builtinIds = new Set(
            agentTemplates.filter((t) => t.builtin).map((t) => t.id),
          );
          const ids = Array.from(selectedTemplateIds).filter(
            (id) => !builtinIds.has(id),
          );
          for (const id of ids) {
            try {
              await agentTemplatesApi.delete(id);
              deleted++;
            } catch {
              failed++;
            }
          }
          await loadAgentTemplates();
        }

        if (failed === 0) {
          toast.success(t("common.batchDeleteSuccess", { count: deleted }));
        } else {
          toast.warning(t("common.batchDeletePartial", { deleted, failed }));
        }
        exitSelectMode();
      },
    });
  }, [
    currentSelectionSize,
    activeTab,
    selectedAgentIds,
    selectedSubAgentIds,
    selectedTemplateIds,
    selectedAgent,
    setSelectedAgent,
    subAgentTemplates,
    agentTemplates,
    loadAgents,
    loadSubAgentTemplates,
    loadAgentTemplates,
    exitSelectMode,
    t,
  ]);

  const headerCreate = useMemo(() => {
    if (activeTab === "agents") {
      return {
        title: t("agentMgmt.createAgent"),
        onClick: handleCreate,
      };
    }
    if (activeTab === "subagents") {
      return {
        title: t("agentMgmt.createSubAgent"),
        onClick: handleCreateSubAgent,
      };
    }
    return {
      title: t("agentMgmt.createTemplate"),
      onClick: handleCreateTemplate,
    };
  }, [activeTab, t]);

  const handleRefreshAll = useCallback(() => {
    loadAgents();
    loadSubAgentTemplates();
    loadAgentTemplates();
  }, [loadAgents, loadSubAgentTemplates, loadAgentTemplates]);

  return (
    <Page className={styles.agentsPage}>
      <PageHeader
        current={t("agent.agents")}
        onRefresh={handleRefreshAll}
        refreshing={loading}
        showMoreMenu={!selectMode}
        moreMenuItems={[
          {
            key: "batchDelete",
            label: t("common.batchDelete"),
            icon: <Trash2 size={14} />,
            onClick: enterSelectMode,
          },
        ]}
        extra={
          selectMode ? (
            <div
              className={styles.headerRight}
              style={{ alignItems: "center" }}
            >
              <span style={{ color: "var(--text-2)", fontSize: 13 }}>
                {t("common.selectedN", { count: currentSelectionSize })}
              </span>
              <Btn variant="outline" onClick={exitSelectMode}>
                {t("common.cancel")}
              </Btn>
              <Btn
                variant="danger-filled"
                icon={<Trash2 />}
                disabled={currentSelectionSize === 0}
                onClick={confirmBatchDelete}
              >
                {t("common.delete")}
              </Btn>
            </div>
          ) : (
            <div className={styles.headerRight}>
              <Tooltip title={headerCreate.title}>
                <Btn
                  icon={<Plus />}
                  onClick={headerCreate.onClick}
                  aria-label={headerCreate.title}
                  title={headerCreate.title}
                />
              </Tooltip>
            </div>
          )
        }
      />

      <PageBody>
        <Tabs
          className={styles.mainTabs}
          activeKey={activeTab}
          onChange={handleTabChange}
          items={[
            {
              key: "agents",
              label: t("agentMgmt.tabAgent"),
              children: (
                <div className={styles.tabContent}>
                  <AgentCardGrid
                    agents={agents}
                    loading={loading || reordering}
                    reordering={reordering}
                    onEdit={handleEdit}
                    onDelete={handleDelete}
                    onToggle={handleToggle}
                    onReorder={handleReorder}
                    selectMode={selectMode}
                    selectedIds={selectedAgentIds}
                    onSelectToggle={toggleAgentSelect}
                  />
                </div>
              ),
            },
            {
              key: "subagents",
              label: t("agentMgmt.tabSubAgent"),
              children: (
                <div className={styles.tabContent}>
                  <SubAgentCardGrid
                    templates={subAgentTemplates}
                    agents={agents}
                    loading={subAgentLoading}
                    onEdit={handleEditSubAgent}
                    onDelete={handleDeleteSubAgent}
                    selectMode={selectMode}
                    selectedIds={selectedSubAgentIds}
                    onSelectToggle={toggleSubAgentSelect}
                  />
                </div>
              ),
            },
            {
              key: "templates",
              label: t("agentMgmt.tabTemplates"),
              children: (
                <div className={styles.tabContent}>
                  <TemplateCardGrid
                    templates={agentTemplates}
                    loading={templateLoading}
                    onEdit={handleEditTemplate}
                    onDelete={handleDeleteTemplate}
                    selectMode={selectMode}
                    selectedIds={selectedTemplateIds}
                    onSelectToggle={toggleTemplateSelect}
                  />
                </div>
              ),
            },
          ]}
        />
      </PageBody>

      <AgentModal
        open={modalVisible}
        editingAgent={editingAgent}
        initialValues={agentConfig ?? undefined}
        selectedSkills={selectedSkills}
        onSelectedSkillsChange={setSelectedSkills}
        onInstalledSkillsLoaded={handleInstalledSkillsLoaded}
        onSave={handleSubmit}
        onClose={() => setModalVisible(false)}
      />

      <SubAgentModal
        open={subAgentModalVisible}
        editingTemplate={editingSubAgent}
        agents={agents}
        onSave={handleSubAgentSubmit}
        onClose={() => setSubAgentModalVisible(false)}
      />

      <TemplateModal
        open={templateModalVisible}
        editingTemplate={editingTemplate}
        onSave={handleSaveTemplate}
        onClose={() => setTemplateModalVisible(false)}
      />
    </Page>
  );
}
