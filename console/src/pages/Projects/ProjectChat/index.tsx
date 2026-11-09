/* eslint-disable react-refresh/only-export-components */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  useLoaderData,
  useLocation,
  useNavigate,
  type LoaderFunctionArgs,
} from "react-router-dom";
import {
  FolderPlus,
  ListTodo,
  MessageSquarePlus,
  MessagesSquare,
  PanelRight,
  PencilLine,
  RefreshCw,
  SlidersHorizontal,
  Upload,
  Zap,
} from "lucide-react";

import {
  agentConversationTarget,
  conversationApi,
} from "@/api/modules/conversation";
import {
  projectChatsApi,
  type ProjectResourcePolicyView,
} from "@/api/modules/projectChats";
import { projectsApi } from "@/api/modules/projects";
import type {
  ACPChatStatus,
  ChatHistory,
  ChatSpec,
  ModelSlot,
} from "@/api/types";
import type {
  ProjectAgentSwitchBlocker,
  ProjectChatAgentSelection,
  ProjectChatAgentState,
  ProjectDescriptor,
} from "@/api/types/project";
import { deviceConnectivityLabel } from "@/components/DeviceConnectivity";
import { useMultiDeviceFeatureAvailability } from "@/hooks/useMultiDeviceFeatureAvailability";
import { useAgentStore } from "@/stores/agentStore";
import { ChatOptionsModal } from "@/pages/Chat";
import LlmDebugCaptureControl from "@/pages/Chat/LlmDebugCaptureControl";
import ChatHeader from "@/pages/Chat/ChatHeader";
import ACPEnterDialog from "@/pages/Chat/ACPEnterDialog";
import {
  loadDraftConversationAcp,
  saveDraftConversationAcp,
} from "@/pages/Chat/conversationLifecycle";
import ModelSelector from "@/pages/Chat/ModelSelector";
import ChatDeviceChip from "@/pages/Chat/ChatDeviceChip";
import { saveDraftConversationModel } from "@/pages/Chat/conversationLifecycle";
import {
  historyProvesConversationMaterialized,
  resolveConversationRoute,
} from "@/pages/Chat/conversationRouteController";
import { resolveAgentChatPath } from "@/pages/Chat/conversationAddress";
import { registerProjectNavigationAgent } from "@/layouts/conversationNavigationAgent";
import {
  invalidateConversationNavigation,
  subscribeConversationDeleted,
  subscribeConversationMetadata,
  publishConversationMetadata,
  registerNewConversationAction,
} from "@/layouts/conversationNavigationEvents";
import { useConversationLifecycle } from "@/pages/Chat/useConversationLifecycle";
import { exportConversationAsMarkdown } from "@/pages/Chat/exportConversation";
import {
  acpStatusFromConversationControl,
  removeAcpBindingFromHistory,
  resolveChatPlacement,
} from "@/pages/Chat/chatPlacementState";
import {
  readChatBlockPreferences,
  writeChatBlockPreferences,
  type ChatBlockPreferenceKey,
} from "@/pages/Chat/owned/blockPreferences";
import OwnedConversationSurface from "@/pages/Chat/owned/OwnedConversationSurface";
import { useConversationActivityWake } from "@/pages/Chat/owned/useConversationActivityWake";
import type { AcpHistoryPreviewPayload } from "@/pages/Chat/owned/useConversationViewRuntime";
import TaskActivityDrawer from "@/pages/Chat/tasks/TaskActivityDrawer";
import ProjectSelector from "@/components/ProjectSelector";
import { toast } from "@/ui";
import { isMobile } from "@/utils/isMobile";

import CreateProjectDialog from "../CreateProjectDialog";
import {
  clearProjectChatDraftState,
  createProjectChat,
  discardProjectChatLocalState,
  projectConversationScope,
  projectChatDraftKey,
  projectChatExecutionAgent,
  projectChatPath,
  projectLastSelectedAgent,
  rememberProjectChat,
  resolveProjectChatPath,
  setProjectChatDraftAgent,
  setProjectLastSelectedAgent,
} from "../projectChatNavigation";
import ProjectAgentSelector from "./ProjectAgentSelector";
import ProjectChatsDrawer from "./ProjectChatsDrawer";
import {
  ProjectShellDrawer,
  type ProjectDrawerKind,
} from "./ProjectShellControls";
import styles from "./index.module.less";

type ProjectChatInitialData = {
  project: ProjectDescriptor;
  chatId: string;
  projects?: ProjectDescriptor[];
  initialHistory?: ChatHistory | null;
  unmaterialized: boolean;
};

export async function loader({
  params,
}: LoaderFunctionArgs): Promise<ProjectChatInitialData> {
  const projectId = params.projectId;
  const chatId = params.chatId;
  if (!projectId || !chatId) {
    throw new Response("Project Chat route is incomplete", { status: 400 });
  }
  const [project, projects] = await Promise.all([
    projectsApi.get(projectId),
    projectsApi.list("active"),
  ]);
  const route = await resolveConversationRoute({
    scope: projectConversationScope(projectId),
    conversationId: chatId,
    loadHistory: () =>
      projectChatsApi.getChat(projectId, chatId, { turnLimit: 1 }),
  });
  return {
    project,
    projects,
    chatId,
    initialHistory: route.history,
    unmaterialized: route.unmaterialized,
  };
}

export function RouteComponent() {
  const initialData = useLoaderData() as ProjectChatInitialData;
  return (
    <ProjectChatPage
      key={`${initialData.project.project_id}:${initialData.chatId}`}
      initialData={initialData}
    />
  );
}

export default function ProjectChatPage({
  initialData,
}: {
  initialData: ProjectChatInitialData;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const navigationHistoryKeyRef = useRef("");
  const projectId = initialData.project.project_id;
  const chatId = initialData.chatId;
  const {
    lazy,
    lazyRef,
    draftModel,
    setDraftModel,
    statefulOperationAccepted,
    authoritativeHistoryObserved,
  } = useConversationLifecycle({
    scope: projectConversationScope(projectId),
    conversationId: chatId,
    unmaterialized: initialData.unmaterialized,
  });
  const initialSelection = initialData.initialHistory
    ? projectAgentSelectionFromHistory(initialData.initialHistory)
    : null;
  const lastSelectionRevisionRef = useRef(initialSelection?.revision ?? -1);
  const fallbackAgentId = projectLastSelectedAgent(projectId) || "default";
  const [history, setHistory] = useState<ChatHistory | null>(
    initialData.initialHistory ?? null,
  );
  useEffect(
    () =>
      subscribeConversationMetadata((address, chat, change) => {
        if (change && !change.fields.includes("name")) return;
        if (
          address.scope !== "project" ||
          address.projectId !== projectId ||
          address.chatId !== chatId
        )
          return;
        setHistory((current) =>
          current ? { ...current, title: chat.name?.trim() || null } : current,
        );
      }),
    [chatId, projectId],
  );
  const [compactLayout, setCompactLayout] = useState(
    () => isMobile() || window.innerWidth <= 768,
  );
  useEffect(() => {
    const resize = () =>
      setCompactLayout(isMobile() || window.innerWidth <= 768);
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);
  const [drawer, setDrawer] = useState<ProjectDrawerKind | null>(null);
  useEffect(() => {
    const search = new URLSearchParams(location.search);
    if (search.get("conversations") !== "1") return;
    setDrawer("chats");
    search.delete("conversations");
    navigate(
      {
        pathname: location.pathname,
        search: search.toString(),
        hash: location.hash,
      },
      { replace: true },
    );
  }, [location.hash, location.pathname, location.search, navigate]);
  const [createProjectOpen, setCreateProjectOpen] = useState(false);
  const [projectLifecycle] = useState(initialData.project.lifecycle);
  const [resourcePolicy, setResourcePolicy] =
    useState<ProjectResourcePolicyView | null>(null);
  const [resourcePolicyLoading, setResourcePolicyLoading] = useState(false);
  const [resourcePolicyError, setResourcePolicyError] = useState<string | null>(
    null,
  );
  const [chatOptionsOpen, setChatOptionsOpen] = useState(false);
  const [contextEditorOpen, setContextEditorOpen] = useState(false);
  const [taskDrawerOpen, setTaskDrawerOpen] = useState(false);
  const [acpEnterOpen, setAcpEnterOpen] = useState(false);
  const [optimisticAcpStatus, setOptimisticAcpStatus] =
    useState<ACPChatStatus | null>(null);
  const [acpHistoryPreview, setAcpHistoryPreview] =
    useState<AcpHistoryPreviewPayload | null>(null);
  const [blockPreferences, setBlockPreferences] = useState(
    readChatBlockPreferences,
  );
  const optimisticAcpControlBaselineRef = useRef<string | null>(null);
  const agents = useAgentStore((state) => state.agents);
  const agentsLoading = useAgentStore(
    (state) =>
      state.catalogLoading ||
      (!state.catalogLoaded && state.catalogError === null),
  );
  const agentsLoaded = useAgentStore((state) => state.catalogLoaded);
  const ensureAgentCatalogLoaded = useAgentStore(
    (state) => state.ensureCatalogLoaded,
  );
  const [switchingAgent, setSwitchingAgent] = useState(false);
  const [executionAgentId, setExecutionAgentId] = useState(() =>
    lazy
      ? projectChatExecutionAgent(projectId, chatId)
      : initialSelection?.agent_id ?? fallbackAgentId,
  );
  useLayoutEffect(
    () =>
      registerProjectNavigationAgent({
        projectId,
        chatId,
        agentId: executionAgentId,
      }),
    [projectId, chatId, executionAgentId],
  );
  const [agentResolved, setAgentResolved] = useState(
    lazy || initialSelection !== null || fallbackAgentId.length > 0,
  );
  const [selectedAgentAvailable, setSelectedAgentAvailable] = useState(
    initialData.initialHistory?.meta?.project_chat_selected_agent_available ===
      true,
  );
  const [switchBlockers, setSwitchBlockers] = useState<
    ProjectAgentSwitchBlocker[]
  >([]);
  const projectTarget = useMemo(
    () => ({
      scope: "project" as const,
      projectId,
    }),
    [projectId],
  );
  const { devices } = useMultiDeviceFeatureAvailability();
  const currentDeviceId =
    devices.find((device) => device.is_self)?.device_id ?? null;
  const projectDevice = devices.find(
    (device) => device.device_id === initialData.project.resource_device_id,
  );
  const chatActivityWake = useConversationActivityWake(
    { scope: "project", projectId },
    chatId,
  );
  const projectDeviceLabel = useCallback(
    (project: ProjectDescriptor) => {
      const device = devices.find(
        (candidate) => candidate.device_id === project.resource_device_id,
      );
      return (
        device?.friendly_name ||
        device?.label ||
        project.resource_device_id.slice(0, 12)
      );
    },
    [devices],
  );

  useEffect(() => {
    void ensureAgentCatalogLoaded().catch((error) => {
      console.error("Failed to load Project Agent candidates:", error);
      setSelectedAgentAvailable(false);
      toast.error(t("projects.chat.agentLoadFailed"));
    });
  }, [ensureAgentCatalogLoaded, t]);

  useEffect(() => {
    if (!agentsLoaded || !lazyRef.current) return;
    const current = projectChatExecutionAgent(projectId, chatId);
    const valid = agents.some((agent) => agent.id === current && agent.enabled);
    const next = valid ? current : "default";
    if (next !== current) setProjectChatDraftAgent(projectId, chatId, next);
    setExecutionAgentId(next);
    setAgentResolved(true);
    setSelectedAgentAvailable(
      agents.some((agent) => agent.id === next && agent.enabled),
    );
  }, [agents, agentsLoaded, chatId, lazyRef, projectId]);

  useEffect(() => {
    if (drawer !== "workspace" || lazy) {
      setResourcePolicy(null);
      setResourcePolicyLoading(false);
      setResourcePolicyError(null);
      return;
    }
    let cancelled = false;
    setResourcePolicyLoading(true);
    setResourcePolicyError(null);
    projectChatsApi
      .getResourcePolicy(projectId, chatId)
      .then((policy) => {
        if (!cancelled) setResourcePolicy(policy);
      })
      .catch((error) => {
        if (cancelled) return;
        console.error("Failed to load Project Resource Policy:", error);
        setResourcePolicy(null);
        setResourcePolicyError(t("projects.workspace.policyUnavailable"));
      })
      .finally(() => {
        if (!cancelled) setResourcePolicyLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [chatId, drawer, executionAgentId, lazy, projectId, t]);

  useEffect(() => {
    if (!agentResolved || agentsLoading || lazy) return;
    setSelectedAgentAvailable(
      agents.some((agent) => agent.id === executionAgentId && agent.enabled),
    );
  }, [agentResolved, agents, agentsLoading, executionAgentId, lazy]);

  const archived = projectLifecycle === "archived";
  const availabilityLabel = archived
    ? t("projects.archived")
    : deviceConnectivityLabel(t, projectDevice?.connectivity_state);
  const switchBlockedReason = switchBlockers[0]
    ? t(projectAgentSwitchBlockerTranslationKey(switchBlockers[0]))
    : undefined;
  const submitDisabledReason = archived
    ? t("projects.chat.archivedSubmitDisabled")
    : !agentResolved || agentsLoading
    ? t("projects.chat.resolvingAgent")
    : !selectedAgentAvailable
    ? t("projects.chat.selectAvailableAgent")
    : undefined;

  const handleStatefulOperationAccepted = useCallback(() => {
    if (!lazyRef.current) return;
    setProjectLastSelectedAgent(projectId, executionAgentId);
    statefulOperationAccepted();
    invalidateConversationNavigation({ scope: "project", projectId, chatId });
    clearProjectChatDraftState(projectId, chatId);
    setAgentResolved(true);
  }, [chatId, executionAgentId, lazyRef, projectId, statefulOperationAccepted]);

  const handleHistoryChange = useCallback(
    (nextHistory: ChatHistory | null) => {
      setHistory(nextHistory);
      if (!nextHistory) return;

      if (lazyRef.current) {
        if (!authoritativeHistoryObserved(nextHistory)) return;
        setProjectLastSelectedAgent(projectId, executionAgentId);
        clearProjectChatDraftState(projectId, chatId);
        setAgentResolved(true);
      }

      if (historyProvesConversationMaterialized(nextHistory)) {
        const navigationKey = JSON.stringify([
          projectId,
          chatId,
          nextHistory.title,
          nextHistory.status,
        ]);
        if (navigationHistoryKeyRef.current !== navigationKey) {
          navigationHistoryKeyRef.current = navigationKey;
          invalidateConversationNavigation({
            scope: "project",
            projectId,
            chatId,
          });
        }
      }
      const agentState = projectAgentStateFromHistory(nextHistory);
      const selection =
        agentState?.selection ?? projectAgentSelectionFromHistory(nextHistory);
      if (selection && selection.revision >= lastSelectionRevisionRef.current) {
        lastSelectionRevisionRef.current = selection.revision;
        setExecutionAgentId(selection.agent_id);
        setAgentResolved(true);
        if (agentState) {
          setSelectedAgentAvailable(agentState.selected_agent_available);
          setSwitchBlockers(agentState.blockers);
          return;
        }
      }
      setSwitchBlockers(projectAgentSwitchBlockersFromHistory(nextHistory));
    },
    [
      authoritativeHistoryObserved,
      chatId,
      executionAgentId,
      lazyRef,
      projectId,
    ],
  );

  const applyCommittedChatModel = useCallback((model: ChatHistory["model"]) => {
    setHistory((current) =>
      current ? { ...current, model: model ?? null } : current,
    );
  }, []);

  const handleChatModelChange = useCallback(
    (model: ModelSlot) => {
      if (lazyRef.current) {
        setDraftModel(model);
        return;
      }
      applyCommittedChatModel(model);
    },
    [applyCommittedChatModel, lazyRef, setDraftModel],
  );

  const handleAgentSelect = useCallback(
    async (agentId: string) => {
      if (lazyRef.current) {
        setProjectChatDraftAgent(projectId, chatId, agentId);
        setProjectLastSelectedAgent(projectId, agentId);
        setExecutionAgentId(agentId);
        setSelectedAgentAvailable(true);
        return;
      }
      if (!executionAgentId) return;
      setSwitchingAgent(true);
      try {
        const state = await projectChatsApi.switchAgent(
          projectId,
          chatId,
          executionAgentId,
          agentId,
        );
        lastSelectionRevisionRef.current = state.selection.revision;
        setExecutionAgentId(state.selection.agent_id);
        setSelectedAgentAvailable(state.selected_agent_available);
        setSwitchBlockers(state.blockers);
        setProjectLastSelectedAgent(projectId, state.selection.agent_id);
        toast.success(t("projects.chat.agentSwitched"));
      } catch (error) {
        toast.error(
          error instanceof Error
            ? error.message
            : t("projects.chat.agentSwitchFailed"),
        );
      } finally {
        setSwitchingAgent(false);
      }
    },
    [chatId, executionAgentId, lazyRef, projectId, t],
  );

  const handleProjectSelect = useCallback(
    async (project: ProjectDescriptor) => {
      navigate(await resolveProjectChatPath(project.project_id));
    },
    [navigate],
  );

  const handleProjectCreated = useCallback(
    async (outcome: import("@/api/types/project").ProjectCreationOutcome) => {
      setCreateProjectOpen(false);
      toast.success(t("projects.created"));
      invalidateConversationNavigation();
      navigate(await resolveProjectChatPath(outcome.project.project_id));
    },
    [navigate, t],
  );

  const handleDuplicateProject = useCallback(
    async (project: ProjectDescriptor) => {
      setCreateProjectOpen(false);
      navigate(await resolveProjectChatPath(project.project_id));
    },
    [navigate],
  );

  const createInheritedProjectChat = useCallback(() => {
    const nextChatId = createProjectChat(projectId, executionAgentId);
    const inheritedModel = lazy ? draftModel : history?.model ?? null;
    if (inheritedModel) {
      saveDraftConversationModel(
        projectConversationScope(projectId),
        nextChatId,
        inheritedModel,
      );
    }
    return nextChatId;
  }, [draftModel, executionAgentId, history?.model, lazy, projectId]);

  const handleNewChat = useCallback(() => {
    setDrawer(null);
    const nextChatId = createInheritedProjectChat();
    navigate(
      `/projects/${encodeURIComponent(projectId)}/chats/${encodeURIComponent(
        nextChatId,
      )}`,
    );
  }, [createInheritedProjectChat, navigate, projectId]);

  useLayoutEffect(
    () =>
      registerNewConversationAction(
        { scope: "project", projectId, chatId },
        handleNewChat,
      ),
    [projectId, chatId, handleNewChat],
  );

  const handleChatSelect = useCallback(
    (chat: ChatSpec) => {
      const mobile = compactLayout;
      rememberProjectChat(projectId, chat.id);
      if (mobile && chat.id === chatId) {
        setDrawer(null);
        return;
      }
      navigate(projectChatPath(projectId, chat.id), {
        replace: mobile,
      });
      setDrawer(null);
    },
    [chatId, compactLayout, navigate, projectId],
  );

  const handleChatUpdated = useCallback(
    (chat: ChatSpec) => {
      publishConversationMetadata(
        {
          scope: "project",
          projectId,
          chatId: chat.id,
        },
        chat,
      );
      if (chat.id !== chatId) return;
      setHistory((current) =>
        current ? { ...current, title: chat.name?.trim() || null } : current,
      );
    },
    [chatId, projectId],
  );

  const handleChatDeleted = useCallback(
    (chat: ChatSpec, remainingKnownChats: ChatSpec[]) => {
      invalidateConversationNavigation({
        scope: "project",
        projectId,
        chatId: chat.id,
      });
      discardProjectChatLocalState(projectId, chat.id);
      if (chat.id !== chatId) {
        rememberProjectChat(projectId, chatId);
        return;
      }

      setDrawer(null);
      const nextChat = mostRecentlyActiveChat(
        remainingKnownChats.filter((chat) => !chat.archived),
      );
      if (nextChat) {
        rememberProjectChat(projectId, nextChat.id);
        navigate(projectChatPath(projectId, nextChat.id), { replace: true });
        return;
      }

      const nextChatId = createInheritedProjectChat();
      navigate(projectChatPath(projectId, nextChatId), { replace: true });
    },
    [chatId, createInheritedProjectChat, navigate, projectId],
  );

  useEffect(
    () =>
      subscribeConversationDeleted((address, remaining) => {
        if (
          address.scope !== "project" ||
          address.projectId !== projectId ||
          address.chatId !== chatId
        )
          return false;
        handleChatDeleted({ id: address.chatId } as ChatSpec, remaining);
        return true;
      }),
    [chatId, handleChatDeleted, projectId],
  );

  const projects = (initialData.projects ?? [initialData.project]).map(
    (project) =>
      project.project_id === projectId
        ? { ...project, lifecycle: projectLifecycle }
        : project,
  );
  const deviceLabel = projectDeviceLabel(initialData.project);
  const projectPlacement = resolveChatPlacement({
    chatId,
    meta: history?.meta,
    workspaceDirFallback: initialData.project.display_path,
    requireAcpStatus: false,
  });
  const controlAcpStatus = acpStatusFromConversationControl(
    history?.control,
    chatId,
  );
  const currentAcpStatus =
    optimisticAcpStatus ?? controlAcpStatus ?? projectPlacement.acpStatus;
  const acpActive =
    optimisticAcpStatus?.active ??
    history?.control?.acp.active ??
    currentAcpStatus?.active ??
    false;
  const acpControlVersion = JSON.stringify(history?.control?.acp ?? null);
  useEffect(() => {
    if (
      optimisticAcpStatus &&
      optimisticAcpControlBaselineRef.current !== acpControlVersion
    ) {
      setOptimisticAcpStatus(null);
      optimisticAcpControlBaselineRef.current = acpControlVersion;
    }
  }, [acpControlVersion, optimisticAcpStatus]);

  const setBlockPreference = useCallback(
    (key: ChatBlockPreferenceKey, expanded: boolean) => {
      setBlockPreferences((current) => {
        const next = { ...current, [key]: expanded };
        writeChatBlockPreferences(next);
        return next;
      });
    },
    [],
  );

  const exitAcp = useCallback(async () => {
    if (lazy) return;
    try {
      await conversationApi.exitAcp(projectTarget, chatId);
      optimisticAcpControlBaselineRef.current = acpControlVersion;
      setOptimisticAcpStatus({
        active: false,
        runner: null,
        session_id: null,
        cwd: null,
        chat_id: chatId,
        workspace_dir:
          currentAcpStatus?.workspace_dir ?? initialData.project.display_path,
        target_device_id: initialData.project.resource_device_id,
      });
      setHistory(removeAcpBindingFromHistory);
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("projects.chat.exitAcpFailed"),
      );
    }
  }, [
    acpControlVersion,
    chatId,
    currentAcpStatus?.workspace_dir,
    initialData.project.display_path,
    initialData.project.resource_device_id,
    lazy,
    projectTarget,
    t,
  ]);

  const exportCurrentChat = useCallback(async () => {
    if (lazy) return;
    try {
      const result = await exportConversationAsMarkdown(projectTarget, chatId);
      if (result.saved) {
        toast.success(
          result.folderHint
            ? t("chat.export.successMobile", {
                folder: result.folderHint,
                filename: result.filename,
              })
            : t("chat.export.success"),
        );
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }, [chatId, lazy, projectTarget, t]);

  const moreMenuItems = [
    {
      key: "new-project",
      icon: <FolderPlus size={14} />,
      label: t("projects.newProject"),
      onClick: () => setCreateProjectOpen(true),
    },
    ...(!archived
      ? [
          {
            key: "acp-enter",
            icon: <Zap size={14} />,
            label: t("chat.acpEnter.menu"),
            disabled: acpActive,
            onClick: () => setAcpEnterOpen(true),
          },
        ]
      : []),
    {
      key: "export",
      icon: <Upload size={14} />,
      label: t("chat.export.tooltip"),
      disabled: lazy,
      onClick: () => void exportCurrentChat(),
    },
    {
      key: "edit-model-context",
      icon: <PencilLine size={14} />,
      label: t("chat.contextEditor.menu", { defaultValue: "编辑模型上下文" }),
      disabled: lazy,
      onClick: () => setContextEditorOpen(true),
    },
    {
      type: "divider" as const,
    },
    {
      key: "refresh",
      icon: <RefreshCw size={14} />,
      label: t("common.refreshPage"),
      onClick: () => window.location.reload(),
    },
    ...(!archived
      ? [
          {
            key: "chat-options",
            icon: <SlidersHorizontal size={14} />,
            label: t("chat.options.menu"),
            onClick: () => setChatOptionsOpen(true),
          },
        ]
      : []),
  ];

  return (
    <div
      className={`rua-owned-chat-page ${styles.page}`}
      data-rua-project-chat
    >
      <ChatHeader
        actions={[
          {
            key: "new-chat",
            label: t("projects.chat.new"),
            tooltip: t("chat.newChatTooltip"),
            icon: <MessageSquarePlus />,
            disabled: archived,
            onClick: handleNewChat,
          },
          {
            key: "conversations",
            label: t("navigation.manageConversations"),
            icon: <MessagesSquare />,
            pressed: drawer === "chats",
            onClick: () =>
              setDrawer((current) => (current === "chats" ? null : "chats")),
          },
          {
            key: "workspace",
            label: t("projects.workspace.title"),
            icon: <PanelRight />,
            pressed: drawer === "workspace",
            onClick: () =>
              setDrawer((current) =>
                current === "workspace" ? null : "workspace",
              ),
          },
          {
            key: "tasks",
            label: t("chat.tasks.title", { defaultValue: "Task activity" }),
            icon: <ListTodo />,
            disabled: lazy,
            onClick: () => setTaskDrawerOpen(true),
          },
        ]}
        moreLabel={t("projects.moreActions")}
        menuItems={moreMenuItems}
        menuFooter={
          executionAgentId ? (
            <LlmDebugCaptureControl
              agentId={executionAgentId}
              chatId={chatId}
              projectId={projectId}
            />
          ) : null
        }
      >
        <ProjectAgentSelector
          agents={agents}
          selectedAgentId={executionAgentId}
          loading={agentsLoading || !agentResolved}
          switching={switchingAgent}
          confirmSelection={!lazy}
          switchable={
            !archived && agentResolved && (lazy || switchBlockers.length === 0)
          }
          switchBlockedReason={switchBlockedReason}
          onSelect={handleAgentSelect}
        />
        <ProjectSelector
          currentProject={{
            ...initialData.project,
            lifecycle: projectLifecycle,
          }}
          projects={projects}
          projectDeviceLabel={projectDeviceLabel}
          onSelect={handleProjectSelect}
          onSelectIndependent={() =>
            navigate(resolveAgentChatPath(executionAgentId))
          }
          onCreate={() => setCreateProjectOpen(true)}
          onOverview={() => navigate("/projects?overview=1")}
        />
      </ChatHeader>
      {currentAcpStatus?.active && !lazy ? (
        <div className="rua-owned-chat-acp-banner" role="status">
          <span className="rua-owned-chat-acp-banner-icon" aria-hidden="true">
            <Zap size={13} />
          </span>
          <span className="rua-owned-chat-acp-banner-label">ACP</span>
          <span className="rua-owned-chat-acp-banner-divider">·</span>
          <span className="rua-owned-chat-acp-banner-runner">
            {currentAcpStatus.runner}
          </span>
          {currentAcpStatus.cwd ? (
            <>
              <span className="rua-owned-chat-acp-banner-divider">·</span>
              <span
                className="rua-owned-chat-acp-banner-cwd"
                title={currentAcpStatus.cwd}
              >
                {currentAcpStatus.cwd}
              </span>
            </>
          ) : null}
          {!history?.meta?.collaboration && (
            <button
              type="button"
              className="rua-owned-chat-acp-banner-exit"
              onClick={() => void exitAcp()}
            >
              {t("chat.acpBanner.exit")}
            </button>
          )}
        </div>
      ) : null}
      <main className={styles.stage}>
        <div className={styles.conversation}>
          <OwnedConversationSurface
            onOpenTasks={() => setTaskDrawerOpen(true)}
            key={`project:${projectId}:${chatId}`}
            address={{ scope: "project", projectId, chatId }}
            unmaterialized={lazy}
            deferInitialAttach={lazy}
            projectExecutionAgentId={executionAgentId || "default"}
            draftKey={projectChatDraftKey(projectId, chatId)}
            authorityPlacementDeviceId={initialData.project.resource_device_id}
            currentDeviceId={currentDeviceId}
            submitDisabled={Boolean(submitDisabledReason)}
            submitDisabledReason={submitDisabledReason}
            activeAcpRunner={
              currentAcpStatus?.active ? currentAcpStatus.runner : null
            }
            acpHistoryPreview={acpHistoryPreview}
            activityWake={chatActivityWake}
            blockPreferences={blockPreferences}
            contextEditorOpen={contextEditorOpen}
            onContextEditorClose={() => setContextEditorOpen(false)}
            chatModel={
              lazy
                ? draftModel
                : history === null
                ? undefined
                : history.model ?? null
            }
            composerLeftTools={
              <ModelSelector
                key={`${projectId}:${chatId}`}
                chatId={chatId}
                chatModel={lazy ? draftModel : history?.model ?? null}
                target={projectTarget}
                draftSelection={lazy}
                disabled={archived}
                onChatModelChange={handleChatModelChange}
              />
            }
            composerDeviceChip={
              <ChatDeviceChip
                currentDeviceId={initialData.project.resource_device_id}
                devices={devices}
                readOnly
              />
            }
            onSubmitAccepted={handleStatefulOperationAccepted}
            onHistoryChange={handleHistoryChange}
          />
        </div>
        {drawer === "chats" ? (
          <ProjectChatsDrawer
            projectId={projectId}
            currentChatId={chatId}
            currentChatRunning={history?.status === "running"}
            readOnly={archived}
            onClose={() => setDrawer(null)}
            onSelectChat={handleChatSelect}
            onChatUpdated={handleChatUpdated}
            onChatDeleted={handleChatDeleted}
          />
        ) : null}
        {drawer === "workspace" ? (
          <ProjectShellDrawer
            localResourceDevice={
              devices.find(
                (device) =>
                  device.device_id === initialData.project.resource_device_id,
              )?.is_self === true
            }
            workingDirectoryId={
              lazy
                ? initialData.project.resources?.primary_directory_id ??
                  "initial"
                : String(
                    history?.meta?.project_working_directory_id ?? "initial",
                  )
            }
            project={{
              ...initialData.project,
              lifecycle: projectLifecycle,
            }}
            deviceLabel={deviceLabel}
            availabilityLabel={availabilityLabel}
            lazy={lazy}
            resourcePolicy={resourcePolicy}
            policyLoading={resourcePolicyLoading}
            policyError={resourcePolicyError}
            onClose={() => setDrawer(null)}
          />
        ) : null}
      </main>
      <CreateProjectDialog
        open={createProjectOpen}
        devices={devices}
        onClose={() => setCreateProjectOpen(false)}
        onCreated={(outcome) => void handleProjectCreated(outcome)}
        onViewDuplicate={(project) => void handleDuplicateProject(project)}
      />
      <ChatOptionsModal
        open={chatOptionsOpen}
        chatId={chatId}
        selectedAgent={executionAgentId || "default"}
        target={projectTarget}
        blockPreferences={blockPreferences}
        onBlockPreferenceChange={setBlockPreference}
        onClose={() => setChatOptionsOpen(false)}
      />
      <ACPEnterDialog
        unmaterialized={lazy}
        draftSelection={
          lazy ? loadDraftConversationAcp(`project:${projectId}`, chatId) : null
        }
        onDraftSelected={(selection) =>
          saveDraftConversationAcp(`project:${projectId}`, chatId, selection)
        }
        open={acpEnterOpen}
        onClose={() => setAcpEnterOpen(false)}
        agentId={executionAgentId || "default"}
        target={projectTarget}
        discoveryTarget={agentConversationTarget(executionAgentId || "default")}
        chatId={chatId}
        targetDeviceId={initialData.project.resource_device_id}
        defaultCwd={
          currentAcpStatus?.workspace_dir || initialData.project.display_path
        }
        onEntered={(status, historyPreview) => {
          optimisticAcpControlBaselineRef.current = acpControlVersion;
          setOptimisticAcpStatus(status);
          if (historyPreview?.trim()) {
            setAcpHistoryPreview({
              id: Date.now(),
              text: historyPreview,
            });
          }
        }}
      />
      <TaskActivityDrawer
        open={taskDrawerOpen}
        backgroundWork={history?.control?.background_work}
        onClose={() => setTaskDrawerOpen(false)}
        agentId={executionAgentId || "default"}
        target={projectTarget}
        chatId={chatId}
        currentDeviceId={currentDeviceId}
      />
    </div>
  );
}

function projectAgentSelectionFromHistory(
  history: ChatHistory,
): ProjectChatAgentSelection | null {
  return projectAgentStateFromHistory(history)?.selection ?? null;
}

function projectAgentStateFromHistory(
  history: ChatHistory,
): ProjectChatAgentState | null {
  const controlState = history.control?.project_chat_agent_state;
  if (controlState) return controlState;
  const state = history.meta?.project_chat_agent_state;
  if (!state || typeof state !== "object") return null;
  const candidate = state as Record<string, unknown>;
  if (
    !candidate.selection ||
    typeof candidate.selection !== "object" ||
    typeof candidate.selected_agent_available !== "boolean" ||
    typeof candidate.switchable !== "boolean" ||
    !Array.isArray(candidate.blockers)
  )
    return null;
  return state as unknown as ProjectChatAgentState;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function projectAgentSwitchBlockersFromHistory(
  history: ChatHistory,
): ProjectAgentSwitchBlocker[] {
  const state = projectAgentStateFromHistory(history);
  if (state) {
    return state.blockers.filter(isProjectAgentSwitchBlocker);
  }

  const blockers: ProjectAgentSwitchBlocker[] = [];
  if (history.active_turn || history.status === "running") {
    blockers.push("active_turn");
  }
  if ((history.control?.queue?.entries?.length || 0) > 0) {
    blockers.push("queued_input");
  }
  if (
    history.control?.acp?.active ||
    (history.meta?.acp_binding && typeof history.meta.acp_binding === "object")
  ) {
    blockers.push("active_acp_binding");
  }
  const hasPrompt = history.messages.some(
    (message) =>
      isRecord(message) &&
      (message.type === "ask_user_request" ||
        (isRecord(message.metadata) &&
          message.metadata.action_type === "ask_user")),
  );
  if (hasPrompt) blockers.push("unanswered_prompt");
  return blockers;
}

function isProjectAgentSwitchBlocker(
  value: unknown,
): value is ProjectAgentSwitchBlocker {
  return (
    typeof value === "string" &&
    [
      "active_turn",
      "queued_input",
      "unresolved_approval",
      "unanswered_prompt",
      "active_acp_binding",
    ].includes(value)
  );
}

function projectAgentSwitchBlockerTranslationKey(
  blocker: ProjectAgentSwitchBlocker,
): string {
  switch (blocker) {
    case "active_turn":
      return "projects.chat.switchBlocker.activeTurn";
    case "queued_input":
      return "projects.chat.switchBlocker.queuedInput";
    case "unresolved_approval":
      return "projects.chat.switchBlocker.unresolvedApproval";
    case "unanswered_prompt":
      return "projects.chat.switchBlocker.unansweredPrompt";
    case "active_acp_binding":
      return "projects.chat.switchBlocker.activeAcp";
  }
}

function mostRecentlyActiveChat(chats: ChatSpec[]): ChatSpec | null {
  return chats.reduce<ChatSpec | null>((latest, chat) => {
    if (!latest) return chat;
    return chatActivityTime(chat) > chatActivityTime(latest) ? chat : latest;
  }, null);
}

function chatActivityTime(chat: ChatSpec): number {
  const value = chat.updated_at || chat.created_at;
  if (!value) return 0;
  const timestamp = new Date(value).getTime();
  return Number.isNaN(timestamp) ? 0 : timestamp;
}
