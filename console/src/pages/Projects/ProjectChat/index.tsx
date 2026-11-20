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
import { useLocation, useNavigate } from "react-router-dom";
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

import { conversationApi } from "@/api/modules/conversation";
import {
  projectChatsApi,
  type ProjectResourcePolicyView,
} from "@/api/modules/projectChats";
import type {
  ACPChatStatus,
  ChatHistory,
  ChatSpec,
  ModelSlot,
} from "@/api/types";
import type { ProjectDescriptor } from "@/api/types/project";
import { deviceConnectivityLabel } from "@/components/DeviceConnectivity";
import { useMultiDeviceFeatureAvailability } from "@/hooks/useMultiDeviceFeatureAvailability";
import { useAgentStore } from "@/stores/agentStore";
import { ChatOptionsModal } from "@/pages/Chat";
import LlmDebugCaptureControl from "@/pages/Chat/LlmDebugCaptureControl";
import ChatHeader from "@/pages/Chat/ChatHeader";
import ACPEnterDialog from "@/pages/Chat/ACPEnterDialog";
import {
  chatDraftScope,
  loadDraftConversationAcp,
  saveDraftConversationAcp,
} from "@/pages/Chat/conversationLifecycle";
import ModelSelector from "@/pages/Chat/ModelSelector";
import ChatDeviceChip from "@/pages/Chat/ChatDeviceChip";
import { openChatDraft } from "@/pages/Chat/chatNavigation";
import { historyProvesConversationMaterialized } from "@/pages/Chat/conversationRouteController";
import {
  chatPath,
  resolveStandaloneChatPath,
} from "@/pages/Chat/conversationAddress";
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
  discardProjectChatLocalState,
  projectChatDraftKey,
  projectChatExecutionAgent,
  rememberProjectChat,
  resolveProjectChatPath,
} from "../projectChatNavigation";
import ChatAgentSelector from "@/pages/Chat/ChatAgentSelector";
import { useChatAgentSelection } from "@/pages/Chat/useChatAgentSelection";
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
    scope: chatDraftScope(projectId),
    conversationId: chatId,
    unmaterialized: initialData.unmaterialized,
  });
  const [history, setHistory] = useState<ChatHistory | null>(
    initialData.initialHistory ?? null,
  );
  useEffect(
    () =>
      subscribeConversationMetadata((address, chat, change) => {
        if (change && !change.fields.includes("name")) return;
        if (address.chatId !== chatId) return;
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
  const {
    selectedAgentId: executionAgentId,
    observeHistory,
    restoreDraftAgent,
    fixedExecutor,
    selectorProps,
    submitDisabledReason: agentDisabledReason,
  } = useChatAgentSelection(
    chatId,
    projectId,
    lazy
      ? projectChatExecutionAgent(projectId, chatId)
      : initialData.initialHistory?.agent_id ?? "default",
    initialData.initialHistory,
    lazyRef,
    history,
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
  const projectTarget = useMemo(
    () => ({
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
    lazy ? undefined : chatId,
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

  const archived = projectLifecycle === "archived";
  const availabilityLabel = archived
    ? t("projects.archived")
    : deviceConnectivityLabel(t, projectDevice?.connectivity_state);
  const submitDisabledReason = archived
    ? t("projects.chat.archivedSubmitDisabled")
    : agentDisabledReason;

  const handleStatefulOperationAccepted = useCallback(() => {
    if (!lazyRef.current) return;
    statefulOperationAccepted();
    invalidateConversationNavigation({ chatId }, false, projectId);
    clearProjectChatDraftState(projectId, chatId);
  }, [chatId, lazyRef, projectId, statefulOperationAccepted]);

  const handleHistoryChange = useCallback(
    (nextHistory: ChatHistory | null) => {
      setHistory(nextHistory);
      if (!nextHistory) return;

      if (lazyRef.current) {
        if (!authoritativeHistoryObserved(nextHistory)) return;
        clearProjectChatDraftState(projectId, chatId);
      }

      if (historyProvesConversationMaterialized(nextHistory)) {
        const navigationKey = JSON.stringify([
          projectId,
          chatId,
          nextHistory.agent_id,
          nextHistory.title,
          nextHistory.status,
        ]);
        if (navigationHistoryKeyRef.current !== navigationKey) {
          navigationHistoryKeyRef.current = navigationKey;
          invalidateConversationNavigation({ chatId }, false, projectId);
        }
      }
      observeHistory(nextHistory);
    },
    [authoritativeHistoryObserved, chatId, lazyRef, projectId, observeHistory],
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
    const nextChatId = openChatDraft(
      projectId,
      lazy ? draftModel : history?.model,
    );
    if (nextChatId === chatId) {
      restoreDraftAgent(useAgentStore.getState().selectedAgent);
    }
    return nextChatId;
  }, [chatId, draftModel, history?.model, lazy, projectId, restoreDraftAgent]);

  const handleNewChat = useCallback(() => {
    setDrawer(null);
    const nextChatId = createInheritedProjectChat();
    navigate(chatPath(nextChatId));
  }, [createInheritedProjectChat, navigate]);

  useLayoutEffect(
    () => registerNewConversationAction({ chatId }, handleNewChat),
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
      navigate(chatPath(chat.id), {
        replace: mobile,
      });
      setDrawer(null);
    },
    [chatId, compactLayout, navigate, projectId],
  );

  const handleChatUpdated = useCallback(
    (chat: ChatSpec) => {
      publishConversationMetadata({ chatId: chat.id }, chat, projectId);
      if (chat.id !== chatId) return;
      setHistory((current) =>
        current ? { ...current, title: chat.name?.trim() || null } : current,
      );
    },
    [chatId, projectId],
  );

  const handleChatDeleted = useCallback(
    (chat: ChatSpec, remainingKnownChats: ChatSpec[]) => {
      invalidateConversationNavigation({ chatId: chat.id }, false, projectId);
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
        navigate(chatPath(nextChat.id), { replace: true });
        return;
      }

      const nextChatId = createInheritedProjectChat();
      navigate(chatPath(nextChatId), { replace: true });
    },
    [chatId, createInheritedProjectChat, navigate, projectId],
  );

  useEffect(
    () =>
      subscribeConversationDeleted((address, remaining) => {
        if (address.chatId !== chatId) return false;
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
      const result = await exportConversationAsMarkdown(chatId);
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
  }, [chatId, lazy, t]);

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
            disabled: acpActive || fixedExecutor,
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
    <div className={`rua-owned-chat-page ${styles.page}`} data-rua-project-chat>
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
        <ChatAgentSelector
          {...selectorProps}
          switchable={!archived && selectorProps.switchable}
          switchBlockedReason={
            archived
              ? t("projects.chat.archivedSubmitDisabled")
              : selectorProps.switchBlockedReason
          }
        />
        <ProjectSelector
          currentProject={{
            ...initialData.project,
            lifecycle: projectLifecycle,
          }}
          projects={projects}
          projectDeviceLabel={projectDeviceLabel}
          onSelect={handleProjectSelect}
          onSelectIndependent={() => navigate(resolveStandaloneChatPath())}
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
          {!fixedExecutor && (
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
            key={chatId}
            address={{ chatId }}
            unmaterialized={lazy}
            deferInitialAttach={lazy}
            projectId={projectId}
            executionAgentId={executionAgentId || "default"}
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
          lazy
            ? loadDraftConversationAcp(chatDraftScope(projectId), chatId)
            : null
        }
        onDraftSelected={(selection) =>
          saveDraftConversationAcp(chatDraftScope(projectId), chatId, selection)
        }
        open={acpEnterOpen}
        onClose={() => setAcpEnterOpen(false)}
        agentId={executionAgentId || "default"}
        target={projectTarget}
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
        chatId={chatId}
        currentDeviceId={currentDeviceId}
      />
    </div>
  );
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
