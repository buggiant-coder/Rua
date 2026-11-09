/* eslint-disable react-refresh/only-export-components */
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLoaderData, useNavigate } from "react-router-dom";
import {
  FilterBar,
  SessionDrawer,
  SessionList,
  type Session,
  type SortField,
  type SortOrder,
} from "./components";
import {
  SESSIONS_PAGE_SIZE,
  useSessions,
  type SessionsInitialData,
} from "./useSessions";
import api from "../../../api";
import { projectChatsApi } from "@/api/modules/projectChats";
import { projectsApi } from "@/api/modules/projects";
import type { ProjectDescriptor } from "@/api/types/project";
import ProjectSelector from "@/components/ProjectSelector";
import { useAgentStore } from "@/stores/agentStore";
import { saveFile, getDownloadFolderHint } from "@/utils/saveFile";
import { PageHeader } from "@/components/PageHeader";
import { Page, PageBody } from "@/components/Page";
import { EmptyState } from "@/components/EmptyState";
import styles from "./index.module.less";
import { navigateWithPageTransition } from "@/layouts/pageTransition";
import { Check, ListChecks, Trash2, Upload, X } from "lucide-react";
import { Btn, Modal, Pagination, toast, type MenuProps } from "@/ui";
import { filterAndSortSessions } from "./components/sessionListModel";
import { routeAgentId } from "@/routes/routeAgent";
import {
  agentChatPath,
  projectChatPath,
} from "@/pages/Chat/conversationAddress";

const SORT_FIELD_STORAGE_KEY = "rua.sessions.sortField";
const SORT_ORDER_STORAGE_KEY = "rua.sessions.sortOrder";
const VALID_SORT_FIELDS: ReadonlyArray<SortField> = [
  "default",
  "name",
  "user_id",
  "channel",
];

type BatchAction = "delete" | "export";

const loadSortOrder = (): SortOrder => {
  try {
    const v = localStorage.getItem(SORT_ORDER_STORAGE_KEY);
    if (v === "asc" || v === "desc") return v;
  } catch {}
  return "desc";
};

const loadSortField = (): SortField => {
  try {
    const v = localStorage.getItem(SORT_FIELD_STORAGE_KEY);
    if (v && (VALID_SORT_FIELDS as ReadonlyArray<string>).includes(v)) {
      return v as SortField;
    }
  } catch {}
  return "default";
};

interface SessionsPageProps {
  initialData?: SessionsInitialData;
}

export async function loader(): Promise<SessionsInitialData> {
  const agentId = routeAgentId();
  const response = await api.listChats(
    { offset: 0, limit: SESSIONS_PAGE_SIZE },
    agentId,
  );
  return {
    agentId,
    sessions: response.data as Session[],
    total: response.total,
  };
}

export function RouteComponent() {
  return <SessionsPage initialData={useLoaderData() as SessionsInitialData} />;
}

function SessionsPage({ initialData }: SessionsPageProps) {
  const { selectedAgent } = useAgentStore();
  const [projects, setProjects] = useState<ProjectDescriptor[]>([]);
  const [project, setProject] = useState<ProjectDescriptor | null>(null);
  const [useInitialData, setUseInitialData] = useState(true);

  useEffect(() => {
    let active = true;
    void projectsApi
      .list("active")
      .then((items) => {
        if (active) setProjects(items);
      })
      .catch((error: unknown) => {
        if (active)
          toast.error(error instanceof Error ? error.message : String(error));
      });
    return () => {
      active = false;
    };
  }, []);

  return (
    <ScopedSessionsPage
      key={project ? `project:${project.project_id}` : `agent:${selectedAgent}`}
      initialData={useInitialData ? initialData : undefined}
      project={project}
      projects={projects}
      onSelectProject={(nextProject) => {
        setUseInitialData(false);
        setProject(nextProject);
      }}
      agentId={selectedAgent}
    />
  );
}

function ScopedSessionsPage({
  initialData,
  project,
  projects,
  onSelectProject,
  agentId,
}: SessionsPageProps & {
  project: ProjectDescriptor | null;
  projects: ProjectDescriptor[];
  onSelectProject: (project: ProjectDescriptor | null) => void;
  agentId: string;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const {
    sessions,
    loading,
    page,
    pageSize,
    unpinnedTotal,
    updateSession,
    deleteSession,
    fetchSessions,
    batchDeleteSessions,
  } = useSessions(initialData, project?.project_id);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [editingSession, setEditingSession] = useState<Session | null>(null);
  const [saving, setSaving] = useState(false);

  const [selectedRowKeys, setSelectedRowKeys] = useState<React.Key[]>([]);
  const [batchAction, setBatchAction] = useState<BatchAction | null>(null);
  const batchMode = batchAction !== null;

  const [searchTitle, setSearchTitle] = useState<string>("");

  const [sortField, setSortFieldState] = useState<SortField>(loadSortField);
  const setSortField = (value: SortField) => {
    setSortFieldState(value);
    try {
      localStorage.setItem(SORT_FIELD_STORAGE_KEY, value);
    } catch {}
  };

  const [sortOrder, setSortOrderState] = useState<SortOrder>(loadSortOrder);
  const setSortOrder = (value: SortOrder) => {
    setSortOrderState(value);
    try {
      localStorage.setItem(SORT_ORDER_STORAGE_KEY, value);
    } catch {}
  };

  const filteredSessions = useMemo<Session[]>(
    () => filterAndSortSessions(sessions, searchTitle, sortField, sortOrder),
    [sessions, searchTitle, sortField, sortOrder],
  );
  const visibleSessionIds = useMemo(
    () => filteredSessions.map((session) => session.id),
    [filteredSessions],
  );
  const allVisibleSelected =
    visibleSessionIds.length > 0 &&
    visibleSessionIds.every((id) => selectedRowKeys.includes(id));

  const handleEdit = (session: Session) => {
    setEditingSession(session);
    setDrawerOpen(true);
  };

  const handleOpenChat = (chatId: string) => {
    navigateWithPageTransition(
      navigate,
      project
        ? projectChatPath(project.project_id, chatId)
        : agentChatPath(agentId, chatId),
    );
  };

  const handleDelete = (chatId: string) => {
    Modal.confirm({
      title: t("sessions.confirmDelete"),
      content: t("sessions.deleteConfirm"),
      okText: t("cronJobs.deleteText"),
      okType: "primary",
      cancelText: t("cronJobs.cancelText"),
      onOk: async () => {
        await deleteSession(chatId);
      },
    });
  };

  const enterBatchMode = (action: BatchAction) => {
    setSelectedRowKeys([]);
    setBatchAction(action);
  };

  const cancelBatchMode = () => {
    setSelectedRowKeys([]);
    setBatchAction(null);
  };

  const toggleSelectAllVisible = () => {
    setSelectedRowKeys(allVisibleSelected ? [] : visibleSessionIds);
  };

  const confirmBatchDelete = () => {
    if (selectedRowKeys.length === 0) {
      toast.warning(t("sessions.batchDeleteConfirm", { count: 0 }));
      return;
    }

    Modal.confirm({
      title: t("sessions.confirmDelete"),
      content: t("sessions.batchDeleteConfirm", {
        count: selectedRowKeys.length,
      }),
      okText: t("cronJobs.deleteText"),
      danger: true,
      cancelText: t("cronJobs.cancelText"),
      onOk: async () => {
        const success = await batchDeleteSessions(selectedRowKeys as string[]);
        if (success) {
          setSelectedRowKeys([]);
          setBatchAction(null);
        }
      },
    });
  };

  const handleExport = async (target: Session[]) => {
    if (target.length === 0) {
      toast.warning(t("sessions.exportEmpty"));
      return false;
    }

    try {
      const payload = await Promise.all(
        target.map(async (session) => {
          const detail = project
            ? await projectChatsApi.getChat(project.project_id, session.id)
            : await api.getChat(session.id, agentId);
          return {
            ...session,
            ...detail,
            id: session.id,
          };
        }),
      );
      const blob = new Blob([JSON.stringify(payload, null, 2)], {
        type: "application/json",
      });
      const stamp = new Date().toISOString().replace(/[:.]/g, "-");
      const filename = `sessions-${stamp}.json`;
      const result = await saveFile(blob, filename);
      if (result.ok) {
        const hint = result.usedPicker ? "" : getDownloadFolderHint();
        const base = t("sessions.exportSuccess", { count: target.length });
        toast.success(hint ? `${base} (${hint}${filename})` : base);
        return true;
      }
      toast.error(t("sessions.exportFailed"));
      return false;
    } catch (error) {
      console.error("Failed to export sessions:", error);
      toast.error(t("sessions.exportFailed"));
      return false;
    }
  };

  const confirmBatchExport = async () => {
    if (selectedRowKeys.length === 0) {
      toast.warning(t("sessions.batchDeleteDisabledTip"));
      return;
    }

    const selectedSet = new Set(selectedRowKeys as string[]);
    const target = filteredSessions.filter((s) => selectedSet.has(s.id));
    const success = await handleExport(target);
    if (success) {
      setSelectedRowKeys([]);
      setBatchAction(null);
    }
  };

  const confirmBatchAction = () => {
    if (batchAction === "export") {
      void confirmBatchExport();
      return;
    }
    confirmBatchDelete();
  };

  const handleDrawerClose = () => {
    setDrawerOpen(false);
    setEditingSession(null);
  };

  const handleSubmit = async (values: Session) => {
    if (editingSession) {
      setSaving(true);
      try {
        const updated = {
          name: values.name,
        };
        const success = await updateSession(editingSession.id, updated);
        if (success) {
          setDrawerOpen(false);
        }
      } finally {
        setSaving(false);
      }
    }
  };

  const showEmpty = !loading && filteredSessions.length === 0;

  return (
    <Page className={styles.sessionsPage}>
      <PageHeader
        agentScoped={!project}
        items={[{ title: t("sessions.title") }]}
        afterBreadcrumb={
          <ProjectSelector
            currentProject={project}
            projects={projects}
            onSelect={onSelectProject}
            onSelectIndependent={() => onSelectProject(null)}
          />
        }
        onRefresh={() => void fetchSessions(page)}
        showMoreMenu={!batchMode}
        moreMenuItems={
          [
            {
              key: "export",
              icon: <Upload />,
              label: t("sessions.batchExport"),
              onClick: () => enterBatchMode("export"),
            },
            {
              key: "batchDelete",
              icon: <Trash2 />,
              label: t("common.batchDelete"),
              onClick: () => enterBatchMode("delete"),
              danger: true,
            },
          ] as MenuProps["items"]
        }
        extra={
          batchMode ? (
            <div className={styles.headerRight}>
              <span className={styles.selectionCount}>
                {t("common.selectedN", { count: selectedRowKeys.length })}
              </span>
              <Btn
                variant="filled"
                icon={<ListChecks />}
                onClick={toggleSelectAllVisible}
                disabled={visibleSessionIds.length === 0}
              >
                {allVisibleSelected
                  ? t("common.clearSelection", { defaultValue: "Clear" })
                  : t("common.selectAll", { defaultValue: "Select all" })}
              </Btn>
              <Btn icon={<X />} onClick={cancelBatchMode} />
              <Btn
                variant={batchAction === "delete" ? "danger-filled" : "filled"}
                icon={<Check />}
                onClick={confirmBatchAction}
                disabled={selectedRowKeys.length === 0}
              />
            </div>
          ) : undefined
        }
      />

      <PageBody className={styles.body}>
        <div className={styles.filterRow}>
          <FilterBar
            searchTitle={searchTitle}
            onSearchTitleChange={setSearchTitle}
            sortField={sortField}
            onSortFieldChange={setSortField}
            sortOrder={sortOrder}
            onSortOrderChange={setSortOrder}
          />
        </div>

        {loading ? (
          <div className={styles.loading}>
            <span className={styles.loadingText}>{t("common.loading")}</span>
          </div>
        ) : showEmpty ? (
          <EmptyState description={t("sessions.emptyText")} />
        ) : (
          <>
            <SessionList
              sessions={filteredSessions}
              batchMode={batchMode}
              selectedKeys={selectedRowKeys}
              onSelectChange={setSelectedRowKeys}
              onOpenChat={handleOpenChat}
              onEdit={handleEdit}
              onDelete={handleDelete}
              t={t}
            />
            {!searchTitle.trim() && unpinnedTotal > pageSize && (
              <div className={styles.paginationRow}>
                <Pagination
                  current={page}
                  total={unpinnedTotal}
                  pageSize={pageSize}
                  onChange={(nextPage) => void fetchSessions(nextPage)}
                />
              </div>
            )}
          </>
        )}
      </PageBody>

      <SessionDrawer
        open={drawerOpen}
        editingSession={editingSession}
        saving={saving}
        onClose={handleDrawerClose}
        onSubmit={handleSubmit}
      />
    </Page>
  );
}

export default SessionsPage;
