import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type SetStateAction,
  type UIEvent,
} from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import ChatHeader from "./ChatHeader";
import { ChatConfigurationUpdateButton } from "./ChatConfigurationUpdateButton";
import { androidRuntimeActions } from "@/utils/androidRuntime";
import { configurationChanges } from "./configurationDiff";
import {
  ListTodo,
  MessageSquarePlus,
  MessagesSquare,
  Monitor,
  PencilLine,
  RefreshCw,
  SlidersHorizontal,
  Wrench,
  ShieldCheck,
  Upload,
  Zap,
} from "lucide-react";
import {
  Btn,
  Modal,
  Radio,
  RadioGroup,
  Spinner,
  Tag,
  TextInput,
  Tooltip,
  toast,
} from "@/ui";
import type { MenuProps } from "@/ui";
import { chatApi } from "@/api/modules/chat";
import { agentsApi } from "@/api/modules/agents";
import { projectsApi } from "@/api/modules/projects";
import type { ProjectDescriptor } from "@/api/types/project";
import ProjectSelector from "@/components/ProjectSelector";
import CreateProjectDialog from "@/pages/Projects/CreateProjectDialog";
import { resolveProjectChatPath } from "@/pages/Projects/projectChatNavigation";
import {
  invalidateConversationNavigation,
  publishConversationMetadata,
  registerNewConversationAction,
  subscribeConversationDeleted,
  subscribeConversationMetadata,
  subscribeConversationNavigation,
} from "@/layouts/conversationNavigationEvents";
import { historyProvesConversationMaterialized } from "./conversationRouteController";
import type { DeviceDto } from "@/api/modules/sync";
import type {
  AgentsRunningConfig,
  ChatHistory,
  ChatRunningConfig,
  ConversationListView,
  ModelSlot,
} from "@/api/types";
import type { ChatSearchResult, ChatSpec, ChatTag } from "@/api/types";
import type { ACPChatStatus } from "@/api/types";
import {
  DEFAULT_CHAT_LIST_SORT,
  type ChatListSort,
} from "@/api/types/chatListSort";
import { acpApi } from "@/api/modules/acp";
import {
  agentConversationTarget,
  conversationApi,
  type ConversationApiTarget,
} from "@/api/modules/conversation";
import { useAgentStore } from "@/stores/agentStore";
import AgentBadge from "@/components/AgentBadge";
import ContentTransitionFrame from "@/layouts/ContentTransitionFrame";
import {
  ChatOptionRow,
  ChatOptionSection,
  ChatOptionsLayout,
  RunningConfigOptions,
} from "@/components/RunningConfigOptions";
import ChatHistoryDrawer from "./ChatHistoryDrawer";
import { nextChatTag } from "./chatTag";
import * as drafts from "./drafts";
import ModelSelector from "./ModelSelector";
import OwnedConversationSurface from "./owned/OwnedConversationSurface";
import { useConversationActivityWake } from "./owned/useConversationActivityWake";
import type { AcpHistoryPreviewPayload } from "./owned/useConversationViewRuntime";
import ACPEnterDialog from "./ACPEnterDialog";
import {
  loadDraftConversationAcp,
  saveDraftConversationAcp,
} from "./conversationLifecycle";
import ChatDeviceChip from "./ChatDeviceChip";
import LlmDebugCaptureControl from "./LlmDebugCaptureControl";
import TaskActivityDrawer from "./tasks/TaskActivityDrawer";
import { exportConversationAsMarkdown } from "./exportConversation";
import {
  readChatBlockPreferences,
  writeChatBlockPreferences,
  type ChatBlockCollapsePreferences,
  type ChatBlockPreferenceKey,
} from "./owned/blockPreferences";
import {
  CHAT_SUBMITTED_EVENT,
  readChatViewTitleCache,
  type ChatSubmittedEventDetail,
} from "./owned/useConversationViewRuntime";
import {
  acpStatusFromConversationControl,
  removeAcpBindingFromHistory,
  resolveChatPlacement,
} from "./chatPlacementState";
import {
  clearDraftChatId,
  forgetChatId,
  loadDraftChatId,
  loadDraftChatModel,
  resolveChatEntry,
  resolveOrCreateDraftChatId,
  saveDraftChatModel,
  saveLastChatId,
} from "./chatNavigation";
import { useConversationLifecycle } from "./useConversationLifecycle";
import { agentChatPath, resolveAgentChatPath } from "./conversationAddress";
import { navigateWithPageTransition } from "../../layouts/pageTransition";
import {
  cancelPreparedChatTransition,
  prepareChatTransition,
} from "./preparedChatTransition";
import { useMultiDeviceFeatureAvailability } from "@/hooks/useMultiDeviceFeatureAvailability";
import "./owned-chat-page.css";

const CHAT_DRAWER_PAGE_SIZE = 10;
type SessionLoadMode = "replace" | "append";

const CHAT_BLOCK_SETTING_ROWS: Array<{
  key: ChatBlockPreferenceKey;
  labelKey: string;
  tooltipKey: string;
  defaultLabel: string;
  defaultTooltip: string;
}> = [
  {
    key: "aggregateExpanded",
    labelKey: "chat.blocks.aggregate",
    tooltipKey: "chat.blocks.aggregateTooltip",
    defaultLabel: "Thinking/tool group",
    defaultTooltip:
      "Controls whether the combined thinking/tool group opens by default.",
  },
  {
    key: "thinkingExpanded",
    labelKey: "chat.blocks.thinking",
    tooltipKey: "chat.blocks.thinkingTooltip",
    defaultLabel: "Reasoning block",
    defaultTooltip:
      "Controls whether individual reasoning blocks open by default.",
  },
  {
    key: "toolExpanded",
    labelKey: "chat.blocks.tool",
    tooltipKey: "chat.blocks.toolTooltip",
    defaultLabel: "Tool block",
    defaultTooltip: "Controls whether individual tool blocks open by default.",
  },
];

function chatTitle(chat: ChatSpec): string {
  return chat.name?.trim() || chat.id;
}

function formatChatDate(raw: string | null | undefined): string {
  if (!raw) return "";
  const date = new Date(raw);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString();
}

function highlightSnippet(snippet: string, query: string): ReactNode {
  const trimmed = query.trim();
  if (!trimmed) return snippet;
  const escaped = trimmed.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const matcher = new RegExp(`(${escaped})`, "gi");
  return snippet.split(matcher).map((part, index) => {
    if (part.toLowerCase() !== trimmed.toLowerCase()) return part;
    return (
      <mark key={`${part}-${index}`} className="rua-owned-search-mark">
        {part}
      </mark>
    );
  });
}

const REMOTE_TARGET_NOTICE_KEY = "rua:remote-target-first-use-notice";
const AUTHORITY_SWITCH_CACHE_NOTICE_DISABLED_KEY =
  "rua:authority-switch-cache-notice-disabled";

type CurrentChatSnapshot = {
  key: string;
  history: ChatHistory | null;
};

function controlAuthorityPlacementDeviceId(
  history: ChatHistory | null,
): string | null | undefined {
  if (!history?.control) return undefined;
  const deviceId = history?.control?.placement?.authority_device_id;
  return typeof deviceId === "string" && deviceId ? deviceId : null;
}

function controlAuthorityPlacementLocked(
  history: ChatHistory | null,
): boolean | undefined {
  const allowed = history?.control?.allowed_controls?.switch_authority;
  return allowed === undefined ? undefined : !allowed;
}

function applyConfirmedAuthorityPlacement(
  history: ChatHistory | null,
  meta: Record<string, unknown>,
): ChatHistory | null {
  if (!history) return history;
  const rawPlacement = meta.authority_placement;
  if (
    !history.control ||
    !rawPlacement ||
    typeof rawPlacement !== "object" ||
    Array.isArray(rawPlacement)
  ) {
    return { ...history, meta };
  }
  const placement = rawPlacement as Record<string, unknown>;
  const deviceId = placement.device_id;
  if (typeof deviceId !== "string" || !deviceId) {
    return { ...history, meta };
  }
  const authorityEpoch = placement.authority_epoch;
  return {
    ...history,
    meta,
    control: {
      ...history.control,
      authority_device_id: deviceId,
      placement: {
        ...history.control.placement,
        authority_device_id: deviceId,
        authority_epoch:
          typeof authorityEpoch === "string"
            ? authorityEpoch
            : history.control.placement.authority_epoch,
      },
    },
  };
}

function shouldShowRemoteTargetFirstUseNotice(): boolean {
  try {
    return localStorage.getItem(REMOTE_TARGET_NOTICE_KEY) !== "true";
  } catch {
    return true;
  }
}

function isAuthoritySwitchCacheNoticeDisabled(): boolean {
  try {
    return (
      localStorage.getItem(AUTHORITY_SWITCH_CACHE_NOTICE_DISABLED_KEY) ===
      "true"
    );
  } catch {
    return false;
  }
}

function DeviceSwitchNotice({
  cacheMessage,
  remoteMessage,
  dontRemindLabel,
}: {
  cacheMessage?: string;
  remoteMessage?: string;
  dontRemindLabel: string;
}) {
  const [cacheNoticeDisabled, setCacheNoticeDisabled] = useState(false);

  return (
    <div className="rua-owned-device-switch-notice">
      {remoteMessage && <span>{remoteMessage}</span>}
      {cacheMessage && (
        <span className="rua-owned-device-switch-cache-notice">
          <span>{cacheMessage}</span>
          {!cacheNoticeDisabled && (
            <button
              type="button"
              onClick={() => {
                try {
                  localStorage.setItem(
                    AUTHORITY_SWITCH_CACHE_NOTICE_DISABLED_KEY,
                    "true",
                  );
                } catch {
                  // Preference persistence is best-effort.
                }
                setCacheNoticeDisabled(true);
              }}
            >
              {dontRemindLabel}
            </button>
          )}
        </span>
      )}
    </div>
  );
}

function showDeviceSwitchNotice({
  cacheMessage,
  remoteMessage,
  dontRemindLabel,
}: {
  cacheMessage?: string;
  remoteMessage?: string;
  dontRemindLabel: string;
}) {
  if (!cacheMessage && !remoteMessage) return;
  if (remoteMessage) {
    try {
      localStorage.setItem(REMOTE_TARGET_NOTICE_KEY, "true");
    } catch {
      // Notice persistence is best-effort.
    }
  }
  toast.info(
    <DeviceSwitchNotice
      cacheMessage={cacheMessage}
      remoteMessage={remoteMessage}
      dontRemindLabel={dontRemindLabel}
    />,
    8000,
  );
}

function deviceDisplayName(device: DeviceDto): string {
  return (
    device.label?.trim() || device.friendly_name?.trim() || device.device_id
  );
}

interface OwnedChatSearchResultsProps {
  query: string;
  agentId: string;
  onResultClick: (chatId: string, title?: string) => void;
}

function OwnedChatSearchResults({
  query,
  agentId,
  onResultClick,
}: OwnedChatSearchResultsProps) {
  const { t } = useTranslation();
  const [results, setResults] = useState<ChatSearchResult[]>([]);
  const [loading, setLoading] = useState(false);
  const seqRef = useRef(0);

  useEffect(() => {
    const trimmed = query.trim();
    const seq = ++seqRef.current;
    if (!trimmed) {
      setResults([]);
      setLoading(false);
      return;
    }

    setLoading(true);
    const timer = window.setTimeout(() => {
      chatApi
        .searchChats(trimmed, agentId)
        .then((response) => {
          if (seq !== seqRef.current) return;
          setResults(response.results || []);
        })
        .catch((err) => {
          if (seq !== seqRef.current) return;
          toast.error(err instanceof Error ? err.message : String(err));
          setResults([]);
        })
        .finally(() => {
          if (seq === seqRef.current) setLoading(false);
        });
    }, 300);

    return () => window.clearTimeout(timer);
  }, [agentId, query]);

  const trimmed = query.trim();
  if (!trimmed) return null;
  if (loading) {
    return (
      <div className="rua-owned-chat-search-spinner">
        <Spinner size="sm" />
      </div>
    );
  }
  if (results.length === 0) {
    return (
      <div className="rua-owned-chat-empty-list">
        {t("chat.noSearchResults")}
      </div>
    );
  }

  return (
    <>
      <div className="rua-owned-chat-search-count">
        {t("chat.searchResultCount", { count: results.length })}
      </div>
      {results.map((result, index) => (
        <button
          key={`${result.chat_id}-${result.card_index}-${index}`}
          type="button"
          className="rua-owned-chat-search-result"
          onClick={() => {
            sessionStorage.setItem(
              "rua_search_scroll_target",
              JSON.stringify({
                cardIndex: result.card_index,
                query: trimmed,
                messageId: result.message_id,
                turnId: result.turn_id,
                conversationId: result.chat_id,
                ownerKey: agentId,
              }),
            );
            onResultClick(
              result.chat_id,
              result.chat_name?.trim() || undefined,
            );
            window.dispatchEvent(new Event("rua-search-scroll"));
          }}
        >
          <div className="rua-owned-chat-search-header">
            <span className="rua-owned-chat-search-title">
              {result.chat_name || result.chat_id}
            </span>
            <Tag
              color={result.role === "user" ? "primary" : "success"}
              style={{ marginRight: 0 }}
            >
              {result.role}
            </Tag>
          </div>
          <div className="rua-owned-chat-search-snippet">
            {highlightSnippet(result.snippet, trimmed)}
          </div>
        </button>
      ))}
    </>
  );
}

const OPTION_SECTION_STORAGE_KEY = "rua:chat-option-section-open";

const DEFAULT_OPTION_SECTIONS: Record<string, boolean> = {
  display: false,
  runtime: false,
  compaction: false,
  aggressive: false,
  token: false,
};

function readOptionSectionOpen(): Record<string, boolean> {
  try {
    const raw = window.localStorage.getItem(OPTION_SECTION_STORAGE_KEY);
    if (!raw) return { ...DEFAULT_OPTION_SECTIONS };
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const result = { ...DEFAULT_OPTION_SECTIONS };
    for (const key of Object.keys(result)) {
      if (typeof parsed[key] === "boolean")
        result[key] = parsed[key] as boolean;
    }
    return result;
  } catch {
    return { ...DEFAULT_OPTION_SECTIONS };
  }
}

function writeOptionSectionOpen(state: Record<string, boolean>) {
  try {
    window.localStorage.setItem(
      OPTION_SECTION_STORAGE_KEY,
      JSON.stringify(state),
    );
  } catch {
    // Section state persistence is best-effort.
  }
}

function ChatBlockPreferenceControl({
  expanded,
  onChange,
}: {
  expanded: boolean;
  onChange: (expanded: boolean) => void;
}) {
  const { t } = useTranslation();
  return (
    <span className="rua-owned-chat-options-segmented">
      <button
        type="button"
        aria-pressed={!expanded}
        className={!expanded ? "is-selected" : undefined}
        onClick={() => onChange(false)}
      >
        {t("chat.blocks.collapsed", { defaultValue: "Collapsed" })}
      </button>
      <button
        type="button"
        aria-pressed={expanded}
        className={expanded ? "is-selected" : undefined}
        onClick={() => onChange(true)}
      >
        {t("chat.blocks.expanded", { defaultValue: "Expanded" })}
      </button>
    </span>
  );
}

export function ChatOptionsModal({
  open,
  chatId,
  selectedAgent,
  target,
  blockPreferences,
  onBlockPreferenceChange,
  onClose,
}: {
  open: boolean;
  chatId?: string;
  selectedAgent: string;
  target?: ConversationApiTarget;
  blockPreferences: ChatBlockCollapsePreferences;
  onBlockPreferenceChange: (
    key: ChatBlockPreferenceKey,
    expanded: boolean,
  ) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<ChatRunningConfig | null>(null);
  const [original, setOriginal] = useState<ChatRunningConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [applyingDefaults, setApplyingDefaults] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>(
    readOptionSectionOpen,
  );
  const conversationTarget = useMemo(
    () => target ?? agentConversationTarget(selectedAgent),
    [selectedAgent, target],
  );

  useEffect(() => {
    writeOptionSectionOpen(openGroups);
  }, [openGroups]);

  function toggleGroup(id: string) {
    setOpenGroups((current) => ({ ...current, [id]: !current[id] }));
  }

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setLoadError(null);
    setDraft(null);
    setOriginal(null);
    if (!chatId) {
      setLoading(false);
      return;
    }
    conversationApi
      .getRunningConfig(conversationTarget, chatId)
      .then((config) => {
        if (cancelled) return;
        setDraft(config);
        setOriginal(config);
      })
      .catch((err) => {
        if (cancelled) return;
        const message =
          err instanceof Error ? err.message : t("chat.options.loadFailed");
        setLoadError(message);
        toast.error(message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [chatId, conversationTarget, open, selectedAgent, t]);

  function setRunningField<K extends keyof ChatRunningConfig>(
    key: K,
    value: ChatRunningConfig[K],
  ) {
    setDraft((current) =>
      current
        ? {
            ...current,
            [key]: value,
          }
        : current,
    );
  }

  function setContextField<
    K extends keyof AgentsRunningConfig["context_compact"],
  >(key: K, value: AgentsRunningConfig["context_compact"][K]) {
    setDraft((current) =>
      current
        ? {
            ...current,
            context_compact: {
              ...current.context_compact,
              [key]: value,
            },
          }
        : current,
    );
  }

  function setToolResultField<
    K extends keyof AgentsRunningConfig["tool_result_compact"],
  >(key: K, value: AgentsRunningConfig["tool_result_compact"][K]) {
    setDraft((current) =>
      current
        ? {
            ...current,
            tool_result_compact: {
              ...current.tool_result_compact,
              [key]: value,
            },
          }
        : current,
    );
  }

  function setThinkingTurnsToKeep(value: number) {
    setDraft((current) =>
      current
        ? {
            ...current,
            context_compact: {
              ...current.context_compact,
              compact_with_thinking_block: value !== 0,
              thinking_turns_to_keep: value,
            },
          }
        : current,
    );
  }

  async function saveOptions() {
    if (!draft || !chatId) return;
    const changes = original ? configurationChanges(original, draft) : [];
    if (changes.length > 0) {
      const confirmed = await Modal.confirm({
        title: t("chat.options.title"),
        content: (
          <dl>
            {changes.map((change) => (
              <div key={change.field}>
                <dt>
                  <code>{change.field}</code>
                </dt>
                <dd>
                  <del>{JSON.stringify(change.before) ?? "—"}</del>
                  {" → "}
                  <ins>{JSON.stringify(change.after) ?? "—"}</ins>
                </dd>
              </div>
            ))}
          </dl>
        ),
        okText: t("common.save"),
        cancelText: t("common.cancel"),
      });
      if (!confirmed) return;
    }
    setSaving(true);
    try {
      const updated = await conversationApi.updateRunningConfig(
        conversationTarget,
        chatId,
        draft,
      );
      setDraft(updated);
      toast.success(t("chat.options.saveSuccess"));
      onClose();
    } catch (err) {
      const message =
        err instanceof Error ? err.message : t("chat.options.saveFailed");
      toast.error(message);
    } finally {
      setSaving(false);
    }
  }

  async function applyGlobalRunningConfig() {
    if (!chatId) return;
    setApplyingDefaults(true);
    try {
      const [current, defaults] = await Promise.all([
        conversationApi.getRunningConfig(conversationTarget, chatId),
        agentsApi.getAgentChatRunningDefaults(selectedAgent),
      ]);
      const changes = configurationChanges(current, defaults);
      const confirmed = await Modal.confirm({
        title: t("chat.options.updateConfiguration", {
          defaultValue: "应用全局配置",
        }),
        content: (
          <dl>
            {changes.map((change) => (
              <div key={change.field}>
                <dt>
                  <code>{change.field}</code>
                </dt>
                <dd>
                  <del>{JSON.stringify(change.before) ?? "—"}</del>
                  {" → "}
                  <ins>{JSON.stringify(change.after) ?? "—"}</ins>
                </dd>
              </div>
            ))}
          </dl>
        ),
        okText: t("common.save"),
        cancelText: t("common.cancel"),
      });
      if (!confirmed) return;
      const config = await conversationApi.updateRunningConfig(
        conversationTarget,
        chatId,
        defaults,
      );
      setDraft(config);
      setOriginal(config);
      setLoadError(null);
      toast.success(t("chat.options.saveSuccess"));
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("chat.options.saveFailed"),
      );
    } finally {
      setApplyingDefaults(false);
    }
  }

  const context = draft?.context_compact;
  const tools = draft?.tool_result_compact;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("chat.options.title", { defaultValue: "Chat options" })}
      width={720}
      className="rua-owned-chat-options-modal"
      footer={
        <>
          <ChatConfigurationUpdateButton
            target={conversationTarget}
            chatId={chatId ?? ""}
            disabled={loading || saving || applyingDefaults}
            onApplied={onClose}
          />
          <Btn
            variant="outline"
            onClick={() => void applyGlobalRunningConfig()}
            loading={applyingDefaults}
            disabled={!chatId || loading || saving || applyingDefaults}
          >
            {t("chat.options.updateConfiguration", {
              defaultValue: "应用全局配置",
            })}
          </Btn>
          <Btn
            variant="outline"
            onClick={onClose}
            disabled={saving || applyingDefaults}
          >
            {t("common.cancel")}
          </Btn>
          <Btn
            onClick={() => void saveOptions()}
            loading={saving}
            disabled={!draft || applyingDefaults}
          >
            {t("common.save")}
          </Btn>
        </>
      }
    >
      <ChatOptionsLayout>
        <ChatOptionSection
          title={t("chat.blocks.menuTitle", {
            defaultValue: "Block display defaults",
          })}
          open={openGroups.display}
          onToggle={() => toggleGroup("display")}
        >
          {CHAT_BLOCK_SETTING_ROWS.map((row) => (
            <ChatOptionRow
              key={row.key}
              label={t(row.labelKey, { defaultValue: row.defaultLabel })}
              tooltip={t(row.tooltipKey, {
                defaultValue: row.defaultTooltip,
              })}
            >
              <ChatBlockPreferenceControl
                expanded={blockPreferences[row.key]}
                onChange={(expanded) =>
                  onBlockPreferenceChange(row.key, expanded)
                }
              />
            </ChatOptionRow>
          ))}
        </ChatOptionSection>

        {loading && (
          <div className="rua-owned-chat-options-loading">
            <Spinner size="sm" />
          </div>
        )}
        {!loading && loadError && (
          <div className="rua-owned-chat-options-error">{loadError}</div>
        )}
        {!loading && draft && context && tools && (
          <RunningConfigOptions
            value={draft}
            openSections={{
              runtime: openGroups.runtime,
              compaction: openGroups.compaction,
              aggressive: openGroups.aggressive,
              token: openGroups.token,
            }}
            onToggleSection={toggleGroup}
            onRunningChange={(key, value) =>
              setRunningField(key, value as never)
            }
            onContextChange={(key, value) =>
              setContextField(key, value as never)
            }
            onToolResultChange={(key, value) =>
              setToolResultField(key, value as never)
            }
            onThinkingTurnsChange={setThinkingTurnsToKeep}
          />
        )}
      </ChatOptionsLayout>
    </Modal>
  );
}

function AgentChatPage({
  agentId,
  chatId,
}: {
  agentId: string;
  chatId: string;
}) {
  const { t } = useTranslation();
  const location = useLocation();
  const navigate = useNavigate();
  const selectedAgent = agentId;
  const setSelectedAgent = useAgentStore((state) => state.setSelectedAgent);
  const [sessionDrawerPath, setSessionDrawerPath] = useState<string | null>(
    null,
  );
  const sessionDrawerOpen = sessionDrawerPath === location.pathname;
  const closeSessionList = () => setSessionDrawerPath(null);
  const [projects, setProjects] = useState<ProjectDescriptor[]>([]);
  const [createProjectOpen, setCreateProjectOpen] = useState(false);
  const navigationHistoryKeyRef = useRef("");
  const renamedChatTitleRef = useRef<{
    key: string;
    title: string;
    previousTitles: string[];
  } | null>(null);

  useEffect(() => {
    let requestId = 0;
    const loadProjects = () => {
      const currentRequest = ++requestId;
      void projectsApi
        .list("active")
        .then((items) => {
          if (currentRequest === requestId) setProjects(items);
        })
        .catch((error: unknown) => {
          if (currentRequest === requestId)
            toast.error(error instanceof Error ? error.message : String(error));
        });
    };
    loadProjects();
    const unsubscribe = subscribeConversationNavigation((address) => {
      if (!address) loadProjects();
    });
    return () => {
      requestId += 1;
      unsubscribe();
    };
  }, []);

  useEffect(() => {
    const search = new URLSearchParams(location.search);
    if (search.get("conversations") !== "1") return;
    setSessionDrawerPath(location.pathname);
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

  const selectProject = async (project: ProjectDescriptor) => {
    const path = await resolveProjectChatPath(project.project_id);
    cancelPreparedChatTransition();
    navigateWithPageTransition(navigate, path);
  };
  const [taskDrawerOpen, setTaskDrawerOpen] = useState(false);
  const [sessions, setSessions] = useState<ChatSpec[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [sessionsLoadingMore, setSessionsLoadingMore] = useState(false);
  const [sessionsTotal, setSessionsTotal] = useState(0);
  const [sessionsPage, setSessionsPage] = useState(1);
  const [sessionSearch, setSessionSearch] = useState("");
  const [sessionsArchived, setSessionsArchived] = useState(false);
  const [sessionsRevision, setSessionsRevision] = useState(0);
  const [pendingArchiveChatIds, setPendingArchiveChatIds] = useState<
    Set<string>
  >(() => new Set());
  const [conversationView, setConversationView] =
    useState<ConversationListView>("user");
  const [collaborationFilters, setCollaborationFilters] = useState({
    sourceChatId: "",
    executor: "",
    status: "",
  });
  const sessionListSearch =
    sessionsArchived || conversationView === "collaboration"
      ? sessionSearch.trim()
      : "";
  const [sessionSort, setSessionSort] = useState<ChatListSort>(
    DEFAULT_CHAT_LIST_SORT,
  );
  const [sessionTagFilter, setSessionTagFilter] = useState<ChatTag | null>(
    null,
  );
  const [currentChatTitle, setCurrentChatTitle] = useState("");
  const [currentChatSnapshot, setCurrentChatSnapshot] =
    useState<CurrentChatSnapshot | null>(null);
  const [blockPreferences, setBlockPreferences] =
    useState<ChatBlockCollapsePreferences>(() => readChatBlockPreferences());
  const { devices: syncDevices, available: multiDeviceFeaturesAvailable } =
    useMultiDeviceFeatureAvailability();
  const currentDeviceId =
    syncDevices.find((device) => device.is_self)?.device_id ?? null;
  const [authorityPlacementModalOpen, setAuthorityPlacementModalOpen] =
    useState(false);
  const [
    authorityPlacementPendingDeviceId,
    setAuthorityPlacementPendingDeviceId,
  ] = useState<string | null>(null);
  const authorityPlacementSaving = authorityPlacementPendingDeviceId !== null;
  const [chatOptionsOpen, setChatOptionsOpen] = useState(false);
  const [contextEditorOpen, setContextEditorOpen] = useState(false);
  const [acpEnterOpen, setACPEnterOpen] = useState(false);
  const [optimisticAcpStatus, setOptimisticAcpStatus] =
    useState<ACPChatStatus | null>(null);
  const optimisticAcpControlBaselineRef = useRef<string | null>(null);
  const [acpHistoryPreview, setAcpHistoryPreview] =
    useState<AcpHistoryPreviewPayload | null>(null);
  const [authorityPlacementDraft, setAuthorityPlacementDraft] = useState("");
  const submittedTitleRef = useRef<Record<string, string>>({});
  const chatTitleHintRef = useRef<Record<string, string>>({});
  const chatHasUserMessagesRef = useRef<Record<string, boolean>>({});
  const sessionsRequestSeqRef = useRef(0);
  const authorityPlacementMutationSeqRef = useRef(0);
  const authorityPlacementMutationInFlightRef = useRef(false);
  const effectiveChatId = chatId;
  const activeChatKey =
    effectiveChatId && selectedAgent
      ? `${selectedAgent}:${effectiveChatId}`
      : null;
  const lazy =
    Boolean(effectiveChatId) &&
    loadDraftChatId(selectedAgent) === effectiveChatId;
  const {
    lazy: conversationLazy,
    draftModel: draftChatModel,
    draftDevice: draftChatDevice,
    setDraftDevice,
    creationStarted,
    setDraftModel,
    statefulOperationAccepted,
    authoritativeHistoryObserved,
  } = useConversationLifecycle({
    scope: selectedAgent,
    conversationId: effectiveChatId || "",
    unmaterialized: lazy,
  });
  const activeChatKeyRef = useRef(activeChatKey);
  activeChatKeyRef.current = activeChatKey;
  const currentChatHistory =
    activeChatKey && currentChatSnapshot?.key === activeChatKey
      ? currentChatSnapshot.history
      : null;
  const activeChatModel = draftChatModel ?? currentChatHistory?.model ?? null;
  const setCurrentChatHistory = useCallback(
    (update: SetStateAction<ChatHistory | null>) => {
      if (!activeChatKey) return;
      setCurrentChatSnapshot((current) => {
        if (activeChatKeyRef.current !== activeChatKey) return current;
        const previous =
          current?.key === activeChatKey ? current.history : null;
        const next = typeof update === "function" ? update(previous) : update;
        if (current?.key === activeChatKey && next === previous) {
          return current;
        }
        return { key: activeChatKey, history: next };
      });
    },
    [activeChatKey],
  );
  useEffect(
    () =>
      subscribeConversationMetadata((address, chat, change) => {
        if (change && !change.fields.includes("name")) return;
        if (
          address.scope !== "agent" ||
          address.agentId !== selectedAgent ||
          address.chatId !== effectiveChatId ||
          !activeChatKey
        )
          return;
        const title = chat.name?.trim() || "";
        const previousRename = renamedChatTitleRef.current;
        renamedChatTitleRef.current = {
          key: activeChatKey,
          title,
          previousTitles:
            previousRename?.key === activeChatKey
              ? [...previousRename.previousTitles, previousRename.title]
              : [currentChatTitle],
        };
        chatTitleHintRef.current[activeChatKey] = title;
        setCurrentChatTitle(title);
        setCurrentChatHistory((current) =>
          current ? { ...current, title } : current,
        );
      }),
    [
      activeChatKey,
      currentChatTitle,
      effectiveChatId,
      selectedAgent,
      setCurrentChatHistory,
    ],
  );

  useEffect(() => {
    setAcpHistoryPreview(null);
  }, [effectiveChatId, selectedAgent]);

  useEffect(() => {
    setSelectedAgent(selectedAgent);
  }, [selectedAgent, setSelectedAgent]);

  useEffect(() => {
    if (!chatId) return;
    saveLastChatId(selectedAgent, chatId);
  }, [chatId, selectedAgent]);

  const chatPlacement = useMemo(
    () =>
      resolveChatPlacement({
        chatId: effectiveChatId,
        meta: currentChatHistory?.meta,
        currentDeviceId,
        requireAcpStatus: false,
      }),
    [currentChatHistory?.meta, currentDeviceId, effectiveChatId],
  );
  const controlAuthorityPlacement =
    controlAuthorityPlacementDeviceId(currentChatHistory);
  const authorityPlacementDeviceId = conversationLazy
    ? draftChatDevice ?? currentDeviceId
    : controlAuthorityPlacement !== undefined
    ? controlAuthorityPlacement
    : chatPlacement.authorityPlacementDeviceId;

  const startNewChat = () => {
    const draftId = resolveOrCreateDraftChatId(selectedAgent);
    const inheritedModel = activeChatModel;
    if (inheritedModel && !loadDraftChatModel(selectedAgent, draftId)) {
      saveDraftChatModel(selectedAgent, draftId, inheritedModel);
    }
    cancelPreparedChatTransition();
    closeSessionList();
    navigateWithPageTransition(navigate, agentChatPath(selectedAgent, draftId));
  };

  useLayoutEffect(() => {
    if (!effectiveChatId) return;
    return registerNewConversationAction(
      { scope: "agent", agentId: selectedAgent, chatId: effectiveChatId },
      startNewChat,
    );
  });

  const titleForActiveChat = useCallback(
    (agentId: string, chatIdForTitle: string | undefined): string => {
      if (!chatIdForTitle) return t("chat.untitled");
      const chatKey = `${agentId}:${chatIdForTitle}`;
      return (
        submittedTitleRef.current[chatKey]?.trim() ||
        chatTitleHintRef.current[chatKey]?.trim() ||
        readChatViewTitleCache(agentId, chatIdForTitle) ||
        ""
      );
    },
    [t],
  );

  useLayoutEffect(() => {
    setOptimisticAcpStatus(null);
    optimisticAcpControlBaselineRef.current = null;
    setCurrentChatTitle(titleForActiveChat(selectedAgent, effectiveChatId));
  }, [effectiveChatId, selectedAgent, titleForActiveChat]);

  const handleConversationHistoryChange = useCallback(
    (history: ChatHistory | null) => {
      const sourceChatKey = activeChatKey;
      const renamed = renamedChatTitleRef.current;
      if (history && renamed?.key === sourceChatKey) {
        if (renamed.previousTitles.includes(history.title?.trim() || ""))
          history = { ...history, title: renamed.title };
        else renamedChatTitleRef.current = null;
      }
      setCurrentChatHistory((current) => {
        const confirmed = current?.meta?.authority_placement;
        if (
          !confirmed ||
          history?.meta?.authority_placement ||
          controlAuthorityPlacementDeviceId(history)
        )
          return history;
        return applyConfirmedAuthorityPlacement(history ?? current, {
          ...history?.meta,
          authority_placement: confirmed,
        });
      });
      if (!sourceChatKey || activeChatKeyRef.current !== sourceChatKey) {
        return;
      }
      if (!effectiveChatId || !history) return;
      authoritativeHistoryObserved(history);
      if (historyProvesConversationMaterialized(history)) {
        const navigationKey = JSON.stringify([
          sourceChatKey,
          history.title,
          history.status,
        ]);
        if (navigationHistoryKeyRef.current !== navigationKey) {
          navigationHistoryKeyRef.current = navigationKey;
          invalidateConversationNavigation({
            scope: "agent",
            agentId: selectedAgent,
            chatId: effectiveChatId,
          });
        }
      }

      const submittedTitle = submittedTitleRef.current[sourceChatKey] || "";
      const historyTitle =
        typeof history.title === "string" ? history.title.trim() : "";
      const title = historyTitle || submittedTitle || "";
      chatHasUserMessagesRef.current[sourceChatKey] = Boolean(title);
      setCurrentChatTitle(title);
    },
    [
      activeChatKey,
      authoritativeHistoryObserved,
      effectiveChatId,
      selectedAgent,
      setCurrentChatHistory,
    ],
  );

  const handleSubmitAccepted = useCallback(() => {
    if (!effectiveChatId || !conversationLazy) return;
    statefulOperationAccepted();
    invalidateConversationNavigation({
      scope: "agent",
      agentId: selectedAgent,
      chatId: effectiveChatId,
    });
  }, [
    conversationLazy,
    effectiveChatId,
    selectedAgent,
    statefulOperationAccepted,
  ]);

  const applyCommittedChatModel = useCallback(
    (model: ModelSlot) => {
      setCurrentChatHistory((current) =>
        current
          ? { ...current, model }
          : {
              status: "idle",
              messages: [],
              model,
            },
      );
    },
    [setCurrentChatHistory],
  );
  const handleChatModelChange = useCallback(
    (model: ModelSlot) => {
      if (conversationLazy && effectiveChatId) {
        setDraftModel(model);
      }
      applyCommittedChatModel(model);
    },
    [applyCommittedChatModel, conversationLazy, effectiveChatId, setDraftModel],
  );

  useEffect(() => {
    const onSubmitted = (event: Event) => {
      const detail = (event as CustomEvent<ChatSubmittedEventDetail>).detail;
      if (detail?.agentId && detail?.chatId) {
        clearDraftChatId(detail.agentId, detail.chatId);
      }
      if (
        detail?.agentId === selectedAgent &&
        detail.chatId === effectiveChatId &&
        detail.title.trim()
      ) {
        const chatKey = `${detail.agentId}:${detail.chatId}`;
        if (chatHasUserMessagesRef.current[chatKey]) {
          return;
        }
        const title = detail.title.trim();
        submittedTitleRef.current[chatKey] = title;
        chatHasUserMessagesRef.current[chatKey] = true;
        setCurrentChatTitle(title);
      }
    };
    window.addEventListener(CHAT_SUBMITTED_EVENT, onSubmitted);
    return () => {
      window.removeEventListener(CHAT_SUBMITTED_EVENT, onSubmitted);
    };
  }, [effectiveChatId, selectedAgent]);

  const loadSessions = useCallback(
    async (page = 1, mode: SessionLoadMode = "replace") => {
      const requestSeq = ++sessionsRequestSeqRef.current;
      if (mode === "append") {
        setSessionsLoadingMore(true);
      } else {
        setSessionsLoading(true);
        setSessionsLoadingMore(false);
      }
      try {
        const offset = (page - 1) * CHAT_DRAWER_PAGE_SIZE;
        const { data, total } = await chatApi.listChats(
          {
            offset,
            limit: CHAT_DRAWER_PAGE_SIZE,
            archived: sessionsArchived,
            ...(sessionsArchived
              ? { view: "all" as const }
              : conversationView === "collaboration"
              ? { view: "collaboration" as const }
              : {}),
            ...(!sessionsArchived &&
            conversationView === "collaboration" &&
            collaborationFilters.sourceChatId.trim()
              ? { source_chat_id: collaborationFilters.sourceChatId.trim() }
              : {}),
            ...(!sessionsArchived &&
            conversationView === "collaboration" &&
            collaborationFilters.executor.trim()
              ? { executor: collaborationFilters.executor.trim() }
              : {}),
            ...(!sessionsArchived &&
            conversationView === "collaboration" &&
            collaborationFilters.status
              ? { status: collaborationFilters.status }
              : {}),
            ...(sessionListSearch ? { search: sessionListSearch } : {}),
            ...sessionSort,
            ...(!sessionsArchived &&
            conversationView === "user" &&
            sessionTagFilter
              ? { tag: sessionTagFilter }
              : {}),
          },
          selectedAgent,
        );
        if (requestSeq !== sessionsRequestSeqRef.current) return;
        setSessionsPage(page);
        setSessionsTotal(total);
        setSessions((current) => {
          if (mode === "replace") return data;
          const existing = new Set(current.map((chat) => chat.id));
          return [...current, ...data.filter((chat) => !existing.has(chat.id))];
        });
      } catch (err) {
        if (requestSeq === sessionsRequestSeqRef.current) {
          toast.error(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (requestSeq === sessionsRequestSeqRef.current) {
          if (mode === "append") {
            setSessionsLoadingMore(false);
          } else {
            setSessionsLoading(false);
            setSessionsLoadingMore(false);
          }
        }
      }
    },
    [
      sessionListSearch,
      sessionsArchived,
      collaborationFilters,
      conversationView,
      selectedAgent,
      sessionSort,
      sessionTagFilter,
    ],
  );

  const chatActivityWake = useConversationActivityWake(
    { scope: "agent", agentId: selectedAgent },
    effectiveChatId,
  );

  const isSessionSearchActive =
    !sessionsArchived &&
    conversationView === "user" &&
    sessionSearch.trim().length > 0;
  const hasMoreSessions = sessions.length < sessionsTotal;

  const loadMoreSessions = useCallback(() => {
    if (
      isSessionSearchActive ||
      sessionsLoading ||
      sessionsLoadingMore ||
      !hasMoreSessions
    ) {
      return;
    }
    void loadSessions(sessionsPage + 1, "append");
  }, [
    hasMoreSessions,
    isSessionSearchActive,
    loadSessions,
    sessionsLoading,
    sessionsLoadingMore,
    sessionsPage,
  ]);

  const handleSessionListScroll = useCallback(
    (event: UIEvent<HTMLDivElement>) => {
      const target = event.currentTarget;
      const remaining =
        target.scrollHeight - target.scrollTop - target.clientHeight;
      if (remaining <= 96) loadMoreSessions();
    },
    [loadMoreSessions],
  );

  useEffect(() => {
    if (sessionDrawerOpen) setSessionSearch("");
  }, [sessionDrawerOpen]);

  useEffect(() => {
    if (!sessionDrawerOpen) return;
    setSessions([]);
    setSessionsTotal(0);
    setSessionsPage(1);
    void loadSessions(1);
    return () => {
      sessionsRequestSeqRef.current += 1;
    };
  }, [loadSessions, sessionDrawerOpen, sessionsRevision]);

  const switchToChat = async (id: string, title?: string) => {
    if (id === effectiveChatId) {
      closeSessionList();
      return;
    }

    const trimmedTitle = title?.trim();

    try {
      const ready = await prepareChatTransition(selectedAgent, id);
      if (!ready) return;
      if (trimmedTitle) {
        chatTitleHintRef.current[`${selectedAgent}:${id}`] = trimmedTitle;
        setCurrentChatTitle(trimmedTitle);
      }
      closeSessionList();
      navigateWithPageTransition(navigate, agentChatPath(selectedAgent, id));
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  const renameChat = async (chat: ChatSpec) => {
    let nextName = chat.name || "";
    const confirmed = await Modal.confirm({
      title: t("chat.renameChat"),
      content: (
        <TextInput
          aria-label={t("chat.renameChatInput")}
          allowClear
          autoFocus
          clearLabel={t("common.clear")}
          defaultValue={nextName}
          onChange={(event) => {
            nextName = event.target.value;
          }}
          onClear={() => {
            nextName = "";
          }}
        />
      ),
      okText: t("common.save"),
      cancelText: t("common.cancel"),
    });
    const trimmed = nextName.trim();
    if (!confirmed || !trimmed || trimmed === (chat.name || "")) return;

    setSessions((items) =>
      items.map((item) =>
        item.id === chat.id ? { ...item, name: trimmed } : item,
      ),
    );
    if (chat.id === effectiveChatId) {
      setCurrentChatTitle(trimmed);
    }
    try {
      const updated = await chatApi.updateChat(
        chat.id,
        { name: trimmed },
        selectedAgent,
      );
      const persistedName = updated.name?.trim() || "";
      invalidateConversationNavigation({
        scope: "agent",
        agentId: selectedAgent,
        chatId: chat.id,
      });
      setSessions((items) =>
        items.map((item) =>
          item.id === updated.id && item.name === trimmed
            ? { ...item, name: updated.name }
            : item,
        ),
      );
      if (updated.id === effectiveChatId) {
        setCurrentChatTitle((current) =>
          current === trimmed ? persistedName : current,
        );
      }
    } catch (err) {
      setSessions((items) =>
        items.map((item) =>
          item.id === chat.id && item.name === trimmed
            ? { ...item, name: chat.name }
            : item,
        ),
      );
      if (chat.id === effectiveChatId) {
        setCurrentChatTitle((current) =>
          current === trimmed ? chat.name?.trim() || "" : current,
        );
      }
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  const togglePinChat = async (chat: ChatSpec) => {
    const optimistic = {
      ...chat,
      pinned: !chat.pinned,
      pin_order: chat.pinned ? 0 : Date.now(),
    };
    setSessions((items) =>
      items.map((item) => (item.id === chat.id ? optimistic : item)),
    );
    try {
      const updated = await chatApi.updateChat(
        chat.id,
        {
          pinned: optimistic.pinned,
          pin_order: optimistic.pin_order,
        },
        selectedAgent,
      );
      invalidateConversationNavigation({
        scope: "agent",
        agentId: selectedAgent,
        chatId: chat.id,
      });
      setSessions((items) =>
        items.map((item) =>
          item.id === updated.id &&
          item.pinned === optimistic.pinned &&
          item.pin_order === optimistic.pin_order
            ? {
                ...item,
                pinned: updated.pinned,
                pin_order: updated.pin_order,
              }
            : item,
        ),
      );
    } catch (err) {
      setSessions((items) =>
        items.map((item) =>
          item.id === chat.id &&
          item.pinned === optimistic.pinned &&
          item.pin_order === optimistic.pin_order
            ? {
                ...item,
                pinned: chat.pinned,
                pin_order: chat.pin_order,
              }
            : item,
        ),
      );
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  const cycleChatTag = async (chat: ChatSpec) => {
    const optimisticTag = nextChatTag(chat.tag);
    setSessions((items) =>
      items.map((item) =>
        item.id === chat.id ? { ...item, tag: optimisticTag } : item,
      ),
    );
    try {
      const updated = await chatApi.updateChat(
        chat.id,
        { tag: optimisticTag },
        selectedAgent,
      );
      invalidateConversationNavigation({
        scope: "agent",
        agentId: selectedAgent,
        chatId: chat.id,
      });
      const leavesCurrentFilter =
        !sessionsArchived &&
        conversationView === "user" &&
        sessionTagFilter !== null &&
        updated.tag !== sessionTagFilter;
      setSessions((items) =>
        leavesCurrentFilter
          ? items.filter((item) => item.id !== updated.id)
          : items.map((item) =>
              item.id === updated.id && item.tag === optimisticTag
                ? { ...item, tag: updated.tag }
                : item,
            ),
      );
      if (leavesCurrentFilter) {
        setSessionsTotal((current) => Math.max(0, current - 1));
      }
    } catch (err) {
      setSessions((items) =>
        items.map((item) =>
          item.id === chat.id && item.tag === optimisticTag
            ? { ...item, tag: chat.tag }
            : item,
        ),
      );
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  const changeChatArchive = async (chat: ChatSpec) => {
    if (pendingArchiveChatIds.has(chat.id)) return;
    setPendingArchiveChatIds((current) => new Set(current).add(chat.id));
    try {
      const updated = await chatApi.updateChat(
        chat.id,
        { archived: !chat.archived },
        selectedAgent,
      );
      publishConversationMetadata(
        { scope: "agent", agentId: selectedAgent, chatId: chat.id },
        updated,
      );
      setSessionsRevision((revision) => revision + 1);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    } finally {
      setPendingArchiveChatIds((current) => {
        const next = new Set(current);
        next.delete(chat.id);
        return next;
      });
    }
  };

  const handleChatDeleted = useCallback(
    async (deletedChatId: string, remainingKnownChats: ChatSpec[]) => {
      forgetChatId(selectedAgent, deletedChatId);
      clearDraftChatId(selectedAgent, deletedChatId);
      drafts.clear(`${selectedAgent}:${deletedChatId}`);
      setSessions((items) => items.filter((item) => item.id !== deletedChatId));
      setSessionsTotal((total) => Math.max(0, total - 1));
      if (deletedChatId !== effectiveChatId) return;
      const targetChatId =
        remainingKnownChats.find(
          (chat) => chat.id !== deletedChatId && !chat.archived,
        )?.id ?? resolveChatEntry(selectedAgent);
      const ready = await prepareChatTransition(selectedAgent, targetChatId);
      if (ready) {
        navigateWithPageTransition(
          navigate,
          agentChatPath(selectedAgent, targetChatId),
          { replace: true },
        );
      }
    },
    [effectiveChatId, navigate, selectedAgent],
  );

  useEffect(
    () =>
      subscribeConversationDeleted((address, remaining) => {
        if (
          address.scope !== "agent" ||
          address.agentId !== selectedAgent ||
          address.chatId !== effectiveChatId
        )
          return false;
        void handleChatDeleted(address.chatId, remaining).catch(
          (error: unknown) => {
            toast.error(error instanceof Error ? error.message : String(error));
          },
        );
        return true;
      }),
    [effectiveChatId, handleChatDeleted, selectedAgent],
  );

  const deleteChat = async (chat: ChatSpec) => {
    const name = chatTitle(chat);
    const ok = await Modal.confirm({
      title: t("common.delete"),
      content: t("chat.deleteConfirm", { name }),
      okText: t("common.delete"),
      cancelText: t("common.cancel"),
      danger: true,
    });
    if (!ok) return;
    try {
      await chatApi.deleteChat(chat.id, selectedAgent);
      invalidateConversationNavigation({
        scope: "agent",
        agentId: selectedAgent,
        chatId: chat.id,
      });
      await handleChatDeleted(chat.id, sessions);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  const exportCurrentChat = async () => {
    if (!effectiveChatId) return;
    try {
      const result = await exportConversationAsMarkdown(
        agentConversationTarget(selectedAgent),
        effectiveChatId,
      );
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
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    }
  };

  const controlAcpStatus = acpStatusFromConversationControl(
    currentChatHistory?.control,
    effectiveChatId,
  );
  const currentAcpStatus =
    optimisticAcpStatus ?? controlAcpStatus ?? chatPlacement.acpStatus;
  const acpControlVersion = JSON.stringify(
    currentChatHistory?.control?.acp ?? null,
  );
  useEffect(() => {
    if (
      optimisticAcpStatus &&
      optimisticAcpControlBaselineRef.current !== acpControlVersion
    ) {
      setOptimisticAcpStatus(null);
      optimisticAcpControlBaselineRef.current = acpControlVersion;
    }
  }, [acpControlVersion, optimisticAcpStatus]);
  const acpStatusUnknown = chatPlacement.acpStatusUnknown;
  const acpActive =
    optimisticAcpStatus !== null
      ? optimisticAcpStatus.active
      : currentAcpStatus?.active === true ||
        currentChatHistory?.control?.acp.active === true;
  const acpStatusBlockReason = acpStatusUnknown
    ? t("chat.acpEnter.statusChecking", {
        defaultValue: "正在确认 ACP 状态…",
      })
    : acpActive
    ? t("chat.acpEnter.alreadyActive", {
        defaultValue: "已在 ACP 模式,先退出再切换",
      })
    : "";
  const authorityPlacementLocked =
    acpActive ||
    (controlAuthorityPlacementLocked(currentChatHistory) ??
      chatPlacement.authorityPlacementLocked);
  const authorityPlacementDevices = useMemo(
    () =>
      [...syncDevices].sort((left, right) => {
        if (left.is_self !== right.is_self) return left.is_self ? -1 : 1;
        return deviceDisplayName(left).localeCompare(deviceDisplayName(right));
      }),
    [syncDevices],
  );
  const selectAuthorityPlacement = async (deviceId: string) => {
    if (!effectiveChatId || authorityPlacementMutationInFlightRef.current) {
      return false;
    }
    if (conversationLazy) {
      if (creationStarted) return deviceId === draftChatDevice;
      setDraftDevice(deviceId);
      return true;
    }
    if (authorityPlacementLocked) {
      toast.warning(
        t(
          "chat.authorityPlacement.acpLocked",
          "Current chat is in ACP mode. Exit ACP before changing the authority device.",
        ),
      );
      return false;
    }
    if (deviceId === authorityPlacementDeviceId) return true;
    const showRemoteNotice =
      deviceId !== currentDeviceId && shouldShowRemoteTargetFirstUseNotice();
    const chatIdForUpdate = effectiveChatId;
    const mutationSeq = ++authorityPlacementMutationSeqRef.current;
    authorityPlacementMutationInFlightRef.current = true;
    setAuthorityPlacementPendingDeviceId(deviceId);
    try {
      const updated = await chatApi.setAuthorityPlacement(
        chatIdForUpdate,
        deviceId,
        selectedAgent,
      );
      if (mutationSeq !== authorityPlacementMutationSeqRef.current) return;
      const nextMeta = updated.meta ?? {};
      setCurrentChatHistory((current) =>
        applyConfirmedAuthorityPlacement(current, nextMeta),
      );
      statefulOperationAccepted();
      showDeviceSwitchNotice({
        cacheMessage: isAuthoritySwitchCacheNoticeDisabled()
          ? undefined
          : t(
              "chat.authorityPlacement.cacheInvalidatedNotice",
              "Switching execution devices invalidates the current conversation cache.",
            ),
        remoteMessage: showRemoteNotice
          ? t(
              "chat.authorityPlacement.firstUseNotice",
              "Remote commands are asynchronous messages. Accepted device deliveries cannot be cancelled from this device in this version.",
            )
          : undefined,
        dontRemindLabel: t(
          "chat.authorityPlacement.cacheInvalidatedDontRemind",
          "Do not remind me again",
        ),
      });
      return true;
    } catch (error) {
      if (mutationSeq !== authorityPlacementMutationSeqRef.current) return;
      toast.error(
        error instanceof Error
          ? error.message
          : t("chat.authorityPlacement.saveFailed", {
              defaultValue: "Failed to update authority device",
            }),
      );
      return false;
    } finally {
      if (mutationSeq === authorityPlacementMutationSeqRef.current) {
        authorityPlacementMutationInFlightRef.current = false;
        setAuthorityPlacementPendingDeviceId(null);
      }
    }
  };
  const openAuthorityPlacementModal = () => {
    if (authorityPlacementLocked) {
      toast.warning(
        t(
          "chat.authorityPlacement.acpLocked",
          "Current chat is in ACP mode. Exit ACP before changing the authority device.",
        ),
      );
      return;
    }
    setAuthorityPlacementDraft(
      authorityPlacementDeviceId ||
        currentDeviceId ||
        authorityPlacementDevices[0]?.device_id ||
        "",
    );
    setAuthorityPlacementModalOpen(true);
  };
  const saveAuthorityPlacementSelection = () => {
    if (!authorityPlacementDraft) return;
    void selectAuthorityPlacement(authorityPlacementDraft);
    setAuthorityPlacementModalOpen(false);
  };
  const authorityPlacementDraftUnchanged =
    authorityPlacementDraft === authorityPlacementDeviceId;
  const selectedAuthorityPlacementDevice = authorityPlacementDevices.find(
    (device) => device.device_id === authorityPlacementDeviceId,
  );
  const selectedAuthorityPlacementLabel = authorityPlacementDeviceId
    ? selectedAuthorityPlacementDevice
      ? deviceDisplayName(selectedAuthorityPlacementDevice)
      : authorityPlacementDeviceId
    : t("chat.authorityPlacement.unavailable", "Authority unavailable");
  const setBlockPreference = (
    key: ChatBlockPreferenceKey,
    expanded: boolean,
  ) => {
    setBlockPreferences((current) => {
      const next = { ...current, [key]: expanded };
      writeChatBlockPreferences(next);
      return next;
    });
  };

  const mobileRuntime = androidRuntimeActions();
  const moreMenuItems: MenuProps["items"] = [
    {
      key: "export",
      icon: <Upload />,
      label: t("chat.export.tooltip"),
      disabled: !effectiveChatId,
      onClick: () => void exportCurrentChat(),
    },
    {
      key: "edit-model-context",
      icon: <PencilLine />,
      label: t("chat.contextEditor.menu", { defaultValue: "编辑模型上下文" }),
      disabled: !effectiveChatId || conversationLazy,
      onClick: () => setContextEditorOpen(true),
    },
    {
      key: "authority-placement",
      icon: <Monitor />,
      label: (
        <Tooltip
          title={
            acpActive
              ? t("chat.acpBanner.blockedWhileActive", {
                  defaultValue: "退出 ACP 模式后可用",
                })
              : ""
          }
          placement="left"
        >
          <span className="rua-owned-chat-menu-option">
            <span>
              {t("chat.authorityPlacement.menu", {
                defaultValue: "Execution device",
              })}
            </span>
            <span className="rua-owned-chat-menu-option-status">
              {selectedAuthorityPlacementLabel}
            </span>
          </span>
        </Tooltip>
      ),

      disabled:
        !effectiveChatId ||
        authorityPlacementLocked ||
        authorityPlacementSaving,
      onClick: openAuthorityPlacementModal,
    },
    {
      key: "acp-enter",
      icon: <Zap />,
      label: (
        <Tooltip title={acpStatusBlockReason} placement="left">
          <span>
            {t("chat.acpEnter.menu", {
              defaultValue: "进入 ACP 透传",
            })}
          </span>
        </Tooltip>
      ),

      disabled: !effectiveChatId || acpStatusUnknown || acpActive,
      onClick: () => setACPEnterOpen(true),
    },
    ...(mobileRuntime
      ? [
          {
            key: "mobile-runtime",
            icon: <Wrench />,
            label: t("common.mobileRuntime"),
            onClick: () => mobileRuntime.openEnvironment(),
          },
          {
            key: "mobile-permissions",
            icon: <ShieldCheck />,
            label: t("common.mobilePermissions"),
            onClick: () => mobileRuntime.openPermissions(),
          },
        ]
      : []),
    { type: "divider" },
    {
      key: "refresh",
      icon: <RefreshCw size={14} aria-hidden="true" />,
      label: t("common.refreshPage", "Refresh page"),
      onClick: () => window.location.reload(),
    },
    {
      key: "chat-options",
      icon: <SlidersHorizontal />,
      label: t("chat.options.menu", {
        defaultValue: "Chat options",
      }),
      onClick: () => setChatOptionsOpen(true),
    },
  ];

  return (
    <div className="rua-owned-chat-page" data-rua-owned-chat-page="true">
      <ChatHeader
        actions={[
          {
            key: "new-chat",
            label: t("chat.newChatTooltip"),
            icon: <MessageSquarePlus />,
            onClick: startNewChat,
          },
          {
            key: "conversations",
            label: t("navigation.manageConversations"),
            icon: <MessagesSquare />,
            pressed: sessionDrawerOpen,
            onClick: () =>
              setSessionDrawerPath(
                sessionDrawerOpen ? null : location.pathname,
              ),
          },
          {
            key: "tasks",
            label: t("chat.tasks.title", { defaultValue: "Task activity" }),
            icon: <ListTodo />,
            disabled: !effectiveChatId,
            onClick: () => setTaskDrawerOpen(true),
          },
        ]}
        moreLabel={t("common.more")}
        menuItems={moreMenuItems}
        menuFooter={
          effectiveChatId && selectedAgent ? (
            <LlmDebugCaptureControl
              agentId={selectedAgent}
              chatId={effectiveChatId}
            />
          ) : null
        }
      >
        <AgentBadge
          onAgentChange={(nextAgentId) =>
            navigateWithPageTransition(
              navigate,
              resolveAgentChatPath(nextAgentId),
            )
          }
        />
        <ProjectSelector
          currentProject={null}
          projects={projects}
          projectDeviceLabel={(project) => {
            const device = syncDevices.find(
              (item) => item.device_id === project.resource_device_id,
            );
            return (
              device?.friendly_name ||
              device?.label ||
              project.resource_device_id.slice(0, 12)
            );
          }}
          onSelect={selectProject}
          onSelectIndependent={() => undefined}
          onCreate={() => setCreateProjectOpen(true)}
          onOverview={() => navigate("/projects?overview=1")}
        />
        <div className="rua-owned-chat-title">
          {currentChatTitle || t("chat.untitled")}
        </div>
      </ChatHeader>
      {currentAcpStatus?.active && effectiveChatId && (
        <div className="rua-owned-chat-acp-banner" role="status">
          <span className="rua-owned-chat-acp-banner-icon" aria-hidden="true">
            <Zap size={13} />
          </span>
          <span className="rua-owned-chat-acp-banner-label">ACP</span>
          <span className="rua-owned-chat-acp-banner-divider">·</span>
          <span className="rua-owned-chat-acp-banner-runner">
            {currentAcpStatus.runner}
          </span>
          {currentAcpStatus.cwd && (
            <>
              <span className="rua-owned-chat-acp-banner-divider">·</span>
              <span
                className="rua-owned-chat-acp-banner-cwd"
                title={currentAcpStatus.cwd}
              >
                {currentAcpStatus.cwd}
              </span>
            </>
          )}
          {!currentChatHistory?.meta?.collaboration && (
            <button
              type="button"
              className="rua-owned-chat-acp-banner-exit"
              onClick={() => {
                const chatIdForExit = effectiveChatId;
                if (!chatIdForExit) return;
                void acpApi
                  .exitChat(
                    selectedAgent,
                    chatIdForExit,
                    currentAcpStatus.target_device_id ??
                      authorityPlacementDeviceId,
                  )
                  .then(() => {
                    optimisticAcpControlBaselineRef.current = acpControlVersion;
                    setOptimisticAcpStatus({
                      active: false,
                      runner: null,
                      session_id: null,
                      cwd: null,
                      chat_id: chatIdForExit,
                      workspace_dir: currentAcpStatus.workspace_dir ?? "",
                      target_device_id:
                        currentAcpStatus.target_device_id ?? null,
                    });
                    setCurrentChatHistory(removeAcpBindingFromHistory);
                  });
              }}
            >
              {t("chat.acpBanner.exit", "退出 ACP")}
            </button>
          )}
        </div>
      )}
      <div className="rua-owned-chat-stage">
        <ContentTransitionFrame
          transitionKey={activeChatKey ?? selectedAgent}
          className="rua-chat-conversation-transition"
        >
          <OwnedConversationSurface
            onOpenTasks={() => setTaskDrawerOpen(true)}
            key={
              effectiveChatId
                ? `${selectedAgent}:${effectiveChatId}`
                : selectedAgent
            }
            address={{
              scope: "agent",
              agentId: selectedAgent,
              chatId: effectiveChatId,
            }}
            unmaterialized={conversationLazy}
            deferInitialAttach={lazy}
            onSubmitAccepted={handleSubmitAccepted}
            onHistoryChange={handleConversationHistoryChange}
            authorityPlacementDeviceId={authorityPlacementDeviceId}
            currentDeviceId={currentDeviceId}
            activeAcpRunner={
              currentAcpStatus?.active ? currentAcpStatus.runner : null
            }
            acpHistoryPreview={acpHistoryPreview}
            activityWake={chatActivityWake}
            blockPreferences={blockPreferences}
            contextEditorOpen={contextEditorOpen}
            onContextEditorClose={() => setContextEditorOpen(false)}
            chatModel={
              conversationLazy
                ? draftChatModel
                : currentChatHistory === null
                ? undefined
                : currentChatHistory.model ?? null
            }
            composerLeftTools={
              <ModelSelector
                disabled={creationStarted}
                key={activeChatKey ?? selectedAgent}
                chatId={effectiveChatId}
                chatModel={activeChatModel}
                draftSelection={conversationLazy}
                target={agentConversationTarget(selectedAgent)}
                onChatModelChange={handleChatModelChange}
              />
            }
            composerDeviceChip={
              multiDeviceFeaturesAvailable ? (
                <ChatDeviceChip
                  currentDeviceId={
                    authorityPlacementPendingDeviceId ??
                    authorityPlacementDeviceId ??
                    (conversationLazy ? currentDeviceId : null)
                  }
                  devices={authorityPlacementDevices}
                  onSelect={selectAuthorityPlacement}
                  disabled={
                    creationStarted ||
                    !effectiveChatId ||
                    authorityPlacementLocked ||
                    authorityPlacementSaving
                  }
                  loading={authorityPlacementSaving}
                />
              ) : undefined
            }
          />
        </ContentTransitionFrame>
        <ChatHistoryDrawer
          projectArchive={{
            archived: sessionsArchived,
            disabled: sessionsLoading,
            onToggle: () => {
              setSessionsArchived((value) => !value);
              setSessionSearch("");
              setSessions([]);
              setSessionsTotal(0);
              setSessionsPage(1);
            },
            onChange: changeChatArchive,
          }}
          open={sessionDrawerOpen}
          onClose={closeSessionList}
          title={t("navigation.manageConversations")}
          currentChatId={effectiveChatId}
          conversationView={conversationView}
          onConversationViewChange={(view) => {
            setConversationView(view);
            setSessionSearch("");
            setSessions([]);
            setSessionsTotal(0);
            setSessionsPage(1);
          }}
          collaborationFilters={collaborationFilters}
          onCollaborationFiltersChange={(filters) => {
            setCollaborationFilters(filters);
            setSessions([]);
            setSessionsTotal(0);
            setSessionsPage(1);
          }}
          chats={sessions}
          loading={sessionsLoading}
          loadingMore={sessionsLoadingMore}
          hasMore={hasMoreSessions}
          searchValue={sessionSearch}
          searchLabel={t("chat.chatListSearchPlaceholder")}
          searchPlaceholder={t("chat.chatListSearchPlaceholder")}
          sort={sessionSort}
          tagFilter={sessionTagFilter}
          sortDisabled={isSessionSearchActive}
          emptyLabel={t("chat.noChats")}
          createLabel={t("chat.createNewChat")}
          createDisabled={
            sessionsArchived || conversationView === "collaboration"
          }
          isActionDisabled={(chat) => pendingArchiveChatIds.has(chat.id)}
          loadMoreLabel={t("chat.loadMoreChats", {
            defaultValue: "Load more",
          })}
          loadingMoreLabel={t("chat.loadingMoreChats", {
            defaultValue: "Loading...",
          })}
          searchContent={
            isSessionSearchActive ? (
              <OwnedChatSearchResults
                query={sessionSearch}
                agentId={selectedAgent}
                onResultClick={(id) => switchToChat(id)}
              />
            ) : undefined
          }
          onCreate={startNewChat}
          onSearchChange={setSessionSearch}
          onSortChange={setSessionSort}
          onTagFilterChange={setSessionTagFilter}
          onSelect={(chat) => switchToChat(chat.id, chat.name)}
          onRename={renameChat}
          onTogglePin={togglePinChat}
          onCycleTag={cycleChatTag}
          onDelete={deleteChat}
          onLoadMore={loadMoreSessions}
          onScroll={handleSessionListScroll}
          formatDate={formatChatDate}
          titleForChat={chatTitle}
        />
      </div>
      <CreateProjectDialog
        open={createProjectOpen}
        devices={syncDevices}
        onClose={() => setCreateProjectOpen(false)}
        onCreated={(outcome) => {
          setCreateProjectOpen(false);
          toast.success(t("projects.created"));
          setProjects((current) => [...current, outcome.project]);
          invalidateConversationNavigation();
          void selectProject(outcome.project);
        }}
        onViewDuplicate={(project) => {
          setCreateProjectOpen(false);
          void selectProject(project);
        }}
      />
      <ChatOptionsModal
        open={chatOptionsOpen}
        chatId={effectiveChatId}
        selectedAgent={selectedAgent}
        blockPreferences={blockPreferences}
        onBlockPreferenceChange={setBlockPreference}
        onClose={() => setChatOptionsOpen(false)}
      />
      <Modal
        open={authorityPlacementModalOpen}
        onClose={() => setAuthorityPlacementModalOpen(false)}
        title={t("chat.authorityPlacement.title", "Choose authority device")}
        width={420}
        footer={
          <>
            <Btn
              variant="outline"
              onClick={() => setAuthorityPlacementModalOpen(false)}
            >
              {t("common.cancel")}
            </Btn>
            <Btn
              onClick={saveAuthorityPlacementSelection}
              disabled={
                !effectiveChatId ||
                authorityPlacementLocked ||
                authorityPlacementDraftUnchanged
              }
            >
              {t("common.save")}
            </Btn>
          </>
        }
      >
        <div className="rua-owned-authority-placement-body">
          <div className="rua-owned-authority-placement-current">
            {t("chat.authorityPlacement.current", {
              defaultValue: "Current selection: {{target}}",
              target: selectedAuthorityPlacementLabel,
            })}
          </div>
          <div className="rua-owned-authority-placement-hint">
            {t(
              "chat.authorityPlacement.hint",
              "Queued messages use the selected device when they are sent.",
            )}
          </div>
          <RadioGroup
            value={authorityPlacementDraft}
            onChange={setAuthorityPlacementDraft}
            className="rua-owned-authority-placement-options"
          >
            <div className="rua-owned-authority-placement-list-label">
              {t("chat.authorityPlacement.deviceList", "Devices")}
            </div>
            {authorityPlacementDevices.length === 0 ? (
              <div className="rua-owned-authority-placement-empty">
                {t("chat.authorityPlacement.noDevices", "No paired devices")}
              </div>
            ) : (
              authorityPlacementDevices.map((device) => (
                <Radio
                  key={device.device_id}
                  value={device.device_id}
                  className="rua-owned-authority-placement-option"
                >
                  <span className="rua-owned-authority-placement-option-content">
                    <span className="rua-owned-authority-placement-option-title">
                      {deviceDisplayName(device)}
                      {device.is_self && (
                        <Tag style={{ marginLeft: 8 }}>
                          {t(
                            "chat.authorityPlacement.thisDevice",
                            "This device",
                          )}
                        </Tag>
                      )}
                    </span>
                    <span className="rua-owned-authority-placement-option-desc">
                      {device.device_id}
                    </span>
                  </span>
                </Radio>
              ))
            )}
          </RadioGroup>
        </div>
      </Modal>
      <ACPEnterDialog
        unmaterialized={conversationLazy}
        draftSelection={
          effectiveChatId && conversationLazy
            ? loadDraftConversationAcp(selectedAgent, effectiveChatId)
            : null
        }
        onDraftSelected={(selection) => {
          if (effectiveChatId)
            saveDraftConversationAcp(selectedAgent, effectiveChatId, selection);
        }}
        open={acpEnterOpen}
        onClose={() => setACPEnterOpen(false)}
        agentId={selectedAgent}
        chatId={effectiveChatId}
        targetDeviceId={authorityPlacementDeviceId}
        defaultCwd={currentAcpStatus?.workspace_dir}
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
        backgroundWork={currentChatHistory?.control?.background_work}
        onClose={() => setTaskDrawerOpen(false)}
        agentId={selectedAgent}
        chatId={effectiveChatId}
        currentDeviceId={currentDeviceId}
      />
    </div>
  );
}

export default function ChatPage() {
  const { agentId = "default", chatId = "" } = useParams<{
    agentId: string;
    chatId: string;
  }>();
  return (
    <AgentChatPage
      key={`agent:${agentId}:${chatId}`}
      agentId={agentId}
      chatId={chatId}
    />
  );
}
