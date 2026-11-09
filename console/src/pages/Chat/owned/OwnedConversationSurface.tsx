import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertCircle,
  ArrowDownToLine,
  ArrowUpToLine,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  RefreshCw,
  Send,
  ShieldCheck,
  PencilLine,
  Trash2,
  X,
} from "lucide-react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { navigateWithPageTransition } from "@/layouts/pageTransition";
import {
  type ConversationAddress,
  agentChatPath,
  projectChatPath,
} from "@/pages/Chat/conversationAddress";
import { prepareChatTransition } from "../preparedChatTransition";
import * as drafts from "../drafts";
import { Modal, TextArea, toast } from "@/ui";
import type { ApprovalRequest } from "@/api/modules/console";
import type {
  ChatHistory,
  CollaborationChatRelation,
  ModelSlot,
} from "@/api/types";
import { conversationApi } from "@/api/modules/conversation";
import type { TaskChatAddress } from "@/api/types/task";
import ConversationMessageList from "./ConversationMessageList";
import type { ConversationContentPart } from "./projection";
import BackgroundWorkStatus from "./BackgroundWorkStatus";
import type { ConversationViewportController } from "./useConversationViewport";
import type { ChatBlockCollapsePreferences } from "./blockPreferences";
import OwnedConversationComposer, {
  type ComposerAttachment,
  type RecalledComposerDraft,
} from "./OwnedConversationComposer";
import {
  readChatViewScrollCache,
  useConversationViewRuntime,
  writeChatViewScrollCache,
  type AcpHistoryPreviewPayload,
  type ApproveOptions,
  type QueuedSubmission,
} from "./useConversationViewRuntime";
import type { ConversationActivityWake } from "./useConversationActivityWake";
import "./owned-conversation-surface.css";

function recalledUserDraft(
  content: ConversationContentPart[],
): RecalledComposerDraft {
  let text = "";
  let imageCount = 0;
  let fileCount = 0;
  const attachments: ComposerAttachment[] = [];
  for (const part of content) {
    if (part.type === "text" && typeof part.text === "string") {
      text += part.text;
      continue;
    }
    if (
      (part.type !== "image" &&
        part.type !== "video" &&
        part.type !== "audio" &&
        part.type !== "file") ||
      !part.media_ref?.startsWith("media/")
    )
      continue;
    const storedName = part.media_ref.slice("media/".length);
    if (!storedName) continue;
    const marker =
      part.type === "image"
        ? `[Image #${++imageCount}]`
        : `[File #${++fileCount}]`;
    const name = part.file_name || part.filename || part.name || storedName;
    attachments.push({
      id: `recalled-${attachments.length}`,
      name,
      type: part.type,
      url: part.media_ref,
      storedName,
      marker,
    });
    text += marker;
  }
  return { text, attachments };
}

type OwnedConversationSurfaceProps = {
  onOpenTasks?: () => void;
  address: ConversationAddress;
  unmaterialized?: boolean;
  deferInitialAttach?: boolean;
  projectExecutionAgentId?: string;
  draftKey?: string;
  submitDisabled?: boolean;
  submitDisabledReason?: string;
  attachmentsDisabled?: boolean;
  onSubmitAccepted?: () => void;
  onHistoryChange?: (history: ChatHistory | null) => void;
  authorityPlacementDeviceId?: string | null;
  currentDeviceId?: string | null;
  activeAcpRunner?: string | null;
  acpHistoryPreview?: AcpHistoryPreviewPayload | null;
  activityWake?: ConversationActivityWake | null;
  blockPreferences?: ChatBlockCollapsePreferences;

  composerDeviceChip?: React.ReactNode;

  chatModel?: ModelSlot | null;

  composerLeftTools?: React.ReactNode;
  contextEditorOpen?: boolean;
  onContextEditorClose?: () => void;
};

function queuedText(item: QueuedSubmission): string {
  const text = item.text.trim();
  if (text) return text;
  if (item.attachments.length === 1) return item.attachments[0].name;
  return `${item.attachments.length} attachments`;
}

export function PendingQueue({
  items,
  onSubmitNow,
  onEdit,
  onReclaim,
  onCancel,
}: {
  items: QueuedSubmission[];
  onSubmitNow: (id: string) => Promise<void> | void;
  onEdit: (id: string, nextText: string) => Promise<void> | void;
  onReclaim: (id: string) => void;
  onCancel: (id: string) => void;
}) {
  const { t } = useTranslation();
  const [editingItem, setEditingItem] = useState<QueuedSubmission | null>(null);
  const [editDraft, setEditDraft] = useState("");
  const [savingEdit, setSavingEdit] = useState(false);
  const tx = (key: string, fallback: string) => {
    const translated = t(key);
    return translated === key ? fallback : translated;
  };
  if (items.length === 0) return null;
  const sendNowLabel = tx("chat.queue.sendNow", "立即发送");
  const editLabel = tx("chat.queue.edit", "编辑");
  const deleteLabel = tx("chat.queue.delete", "删除");
  const closeEditor = () => {
    if (savingEdit) return;
    setEditingItem(null);
    setEditDraft("");
  };
  const saveEdit = async () => {
    if (!editingItem || !editDraft.trim() || savingEdit) return;
    setSavingEdit(true);
    try {
      await onEdit(editingItem.id, editDraft);
      setEditingItem(null);
      setEditDraft("");
    } finally {
      setSavingEdit(false);
    }
  };
  return (
    <>
      <div
        className="rua-owned-pending-queue"
        aria-label={tx("chat.queue.label", "Pending messages")}
      >
        {items.map((item, index) => (
          <div className="rua-owned-pending-queue-item" key={item.id}>
            {item.deliveryState === "unknown" && <span>提交结果待确认</span>}
            <span className="rua-owned-pending-queue-grip" />
            <span className="rua-owned-pending-queue-main">
              <span className="rua-owned-pending-queue-text">
                {queuedText(item)}
              </span>
            </span>
            <button
              type="button"
              className="rua-owned-pending-queue-icon"
              aria-label={sendNowLabel}
              title={sendNowLabel}
              disabled={index > 0 || item.source === "transport"}
              onClick={() => void onSubmitNow(item.id)}
            >
              <Send size={15} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="rua-owned-pending-queue-icon"
              aria-label={editLabel}
              title={editLabel}
              disabled={
                item.editable === false || item.deliveryState === "sending"
              }
              onClick={() => {
                if (!item.serverQueued) {
                  onReclaim(item.id);
                  return;
                }
                setEditDraft(item.text);
                setEditingItem(item);
              }}
            >
              <PencilLine size={15} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="rua-owned-pending-queue-icon"
              aria-label={deleteLabel}
              title={deleteLabel}
              disabled={
                item.deletable === false || item.deliveryState === "sending"
              }
              onClick={() => onCancel(item.id)}
            >
              <Trash2 size={15} aria-hidden="true" />
            </button>
          </div>
        ))}
      </div>
      <Modal
        open={editingItem !== null}
        title={tx("chat.queue.editTitle", "编辑消息")}
        width={480}
        onClose={closeEditor}
        onOk={saveEdit}
        okText={t("common.save")}
        cancelText={t("common.cancel")}
        confirmLoading={savingEdit}
        okButtonProps={{ disabled: !editDraft.trim() }}
      >
        <div className="rua-owned-queue-editor">
          <TextArea
            autoFocus
            rows={4}
            aria-label={tx("chat.queue.editInput", "排队消息内容")}
            value={editDraft}
            onChange={(event) => setEditDraft(event.target.value)}
            onKeyDown={(event) => {
              if (
                event.key === "Enter" &&
                !event.shiftKey &&
                !event.nativeEvent.isComposing
              ) {
                event.preventDefault();
                void saveEdit();
              }
            }}
          />
        </div>
      </Modal>
    </>
  );
}

function approvalReason(request: ApprovalRequest): string {
  const trigger =
    request.trigger && typeof request.trigger === "object"
      ? (request.trigger as Record<string, unknown>)
      : {};
  const message = typeof trigger.message === "string" ? trigger.message : "";
  if (message.includes("(Balanced)")) {
    return "Balanced mode asks before shell, write/edit, MCP/plugin, and web tool calls.";
  }
  if (message.includes("(Ask First)")) {
    return "Ask First mode requests authorization before every tool call.";
  }
  return message.trim() || request.tool_call.display || request.tool_name;
}

function approvalSource(request: ApprovalRequest): string {
  const trigger =
    request.trigger && typeof request.trigger === "object"
      ? (request.trigger as Record<string, unknown>)
      : {};
  const source = typeof trigger.source === "string" ? trigger.source : "";
  if (source === "approval_mode") return "Approval Mode";
  if (source === "tool_guard") return "Safety Rule";
  if (source === "file_guard") return "File Guard";
  return source || "Approval";
}

function shortApprovalTaskId(taskId: string): string {
  return taskId.length > 12
    ? `${taskId.slice(0, 8)}...${taskId.slice(-4)}`
    : taskId;
}

function ApprovalTray({
  pending,
  onApprove,
}: {
  pending: ApprovalRequest[];
  onApprove: (
    approvalRequestId: string,
    approve: boolean,
    options?: ApproveOptions,
  ) => Promise<void> | void;
}) {
  const { t } = useTranslation();
  const [activeIndex, setActiveIndex] = useState(0);
  const [submittingId, setSubmittingId] = useState<string | null>(null);

  useEffect(() => {
    setActiveIndex((current) =>
      pending.length === 0 ? 0 : Math.min(current, pending.length - 1),
    );
  }, [pending.length]);

  if (pending.length === 0) return null;

  const request = pending[activeIndex] || pending[0];
  const reuseOptions = request.reuse_options || [];
  const chatReuseOptions = reuseOptions.filter(
    (option) => option.scope === "chat",
  );
  const agentReuseOptions = reuseOptions.filter(
    (option) => option.scope === "agent",
  );
  const disabled = submittingId === request.id;
  const decide = async (approve: boolean, options?: ApproveOptions) => {
    setSubmittingId(request.id);
    try {
      await onApprove(request.id, approve, options);
      setActiveIndex((current) =>
        pending.length <= 1 ? 0 : Math.min(current, pending.length - 2),
      );
    } finally {
      setSubmittingId(null);
    }
  };

  return (
    <div
      className="rua-approval-tray"
      aria-label={t("chat.approval.queueLabel", {
        defaultValue: "Approval queue",
      })}
    >
      <div className="rua-approval-tray-header">
        <span className="rua-approval-tray-icon">
          <ShieldCheck size={15} />
        </span>
        <div className="rua-approval-tray-title">
          <strong>{request.tool_name}</strong>
          <span>
            {request.origin === "acp"
              ? request.acp_task
                ? `ACP · ${
                    request.runner || "runner"
                  } · Task ${shortApprovalTaskId(
                    request.acp_task.task_id,
                  )} · #${request.acp_task.attempt_no}`
                : `ACP · ${request.runner || "runner"}`
              : approvalSource(request)}
          </span>
        </div>
        <div className="rua-approval-tray-count">
          {activeIndex + 1} / {pending.length}
        </div>
        {pending.length > 1 && (
          <div className="rua-approval-tray-nav">
            <button
              type="button"
              aria-label="Previous approval"
              disabled={activeIndex === 0}
              onClick={() =>
                setActiveIndex((current) => Math.max(0, current - 1))
              }
            >
              <ChevronLeft size={14} />
            </button>
            <button
              type="button"
              aria-label="Next approval"
              disabled={activeIndex >= pending.length - 1}
              onClick={() =>
                setActiveIndex((current) =>
                  Math.min(pending.length - 1, current + 1),
                )
              }
            >
              <ChevronRight size={14} />
            </button>
          </div>
        )}
      </div>
      <div className="rua-approval-tray-body">
        <div className="rua-approval-tray-reason">
          {approvalReason(request)}
        </div>
        {request.tool_call.display && (
          <div className="rua-approval-tray-display">
            {request.tool_call.display}
          </div>
        )}
      </div>
      {request.origin === "acp" ? (
        <div className="rua-approval-tray-actions">
          {(request.acp_options || []).map((option) => {
            const isReject = option.kind.startsWith("reject");
            return (
              <button
                key={option.option_id}
                type="button"
                className={`rua-approval-tray-button${
                  isReject ? "" : " rua-approval-tray-button--primary"
                }`}
                disabled={disabled}
                onClick={() =>
                  void decide(!isReject, { acpOptionId: option.option_id })
                }
              >
                {isReject ? <X size={14} /> : <Check size={14} />}
                <span>{option.name}</span>
              </button>
            );
          })}
        </div>
      ) : (
        <>
          <div className="rua-approval-tray-actions">
            <button
              type="button"
              className="rua-approval-tray-button"
              disabled={disabled}
              onClick={() => void decide(false)}
            >
              <X size={14} />
              <span>
                {t("chat.approval.reject", { defaultValue: "Reject" })}
              </span>
            </button>
            <button
              type="button"
              className="rua-approval-tray-button rua-approval-tray-button--primary"
              disabled={disabled}
              onClick={() => void decide(true, { decision: "run_once" })}
            >
              <Check size={14} />
              <span>
                {t("chat.approval.runOnce", { defaultValue: "Run once" })}
              </span>
            </button>
          </div>
          {chatReuseOptions.length > 0 && (
            <div className="rua-approval-tray-option-group">
              <div className="rua-approval-tray-option-heading">
                {t("chat.approval.allowInChat", {
                  defaultValue: "Allow in this chat",
                })}
              </div>
              <div className="rua-approval-tray-option-list">
                {chatReuseOptions.map((option) => (
                  <button
                    key={`${option.scope}-${option.label}`}
                    type="button"
                    className="rua-approval-tray-option-button"
                    disabled={disabled}
                    onClick={() =>
                      void decide(true, {
                        decision: "allow_chat",
                        matcher: option.matcher,
                      })
                    }
                  >
                    <span>{option.label}</span>
                  </button>
                ))}
              </div>
            </div>
          )}
          {agentReuseOptions.length > 0 && (
            <div className="rua-approval-tray-option-group">
              <div className="rua-approval-tray-option-heading">
                {t("chat.approval.allowForAgent", {
                  defaultValue: "Always allow for this agent",
                })}
              </div>
              <div className="rua-approval-tray-option-list">
                {agentReuseOptions.map((option) => (
                  <button
                    key={`${option.scope}-${option.label}`}
                    type="button"
                    className="rua-approval-tray-option-button"
                    disabled={disabled}
                    onClick={() =>
                      void decide(true, {
                        decision: "allow_agent",
                        matcher: option.matcher,
                      })
                    }
                  >
                    <span>{option.label}</span>
                  </button>
                ))}
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}

export default function OwnedConversationSurface({
  onOpenTasks,
  address,
  unmaterialized = false,
  deferInitialAttach = false,
  projectExecutionAgentId,
  draftKey: externalDraftKey,
  submitDisabled = false,
  submitDisabledReason,
  attachmentsDisabled = false,
  onSubmitAccepted,
  onHistoryChange,
  authorityPlacementDeviceId,
  currentDeviceId,
  activeAcpRunner,
  acpHistoryPreview,
  activityWake,
  blockPreferences,
  composerDeviceChip,
  chatModel,
  composerLeftTools,
  contextEditorOpen = false,
  onContextEditorClose,
}: OwnedConversationSurfaceProps) {
  const { t } = useTranslation();
  const tx = (key: string, fallback: string) => {
    const translated = t(key);
    return translated === key ? fallback : translated;
  };
  const inputPlaceholder = (() => {
    const translated = t("chat.inputPlaceholder");
    return translated === "chat.inputPlaceholder" ? "Message" : translated;
  })();
  const runtime = useConversationViewRuntime({
    address,
    authorityPlacementDeviceId,
    currentDeviceId,
    activeAcpRunner,
    acpHistoryPreview,
    activityWake,
    projectExecutionAgentId,
    unmaterialized,
    deferInitialAttach,
  });
  const { identity, executionAgentId: selectedAgent } = runtime;
  const {
    chatId: conversationId,
    agentId,
    projectId,
    target,
    mediaScope,
    ownerKey: runtimeOwnerKey,
    viewKey,
  } = identity;
  const {
    hasMoreBefore,
    loadOlder,
    loadingOlder,
    reclaimQueuedSubmission,
    applyCommittedModel,
    submit: submitMessage,
    rollback,
    branch,
  } = runtime;
  const navigate = useNavigate();
  const messagesRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<ConversationViewportController | null>(null);
  const [excludedMessageIds, setExcludedMessageIds] = useState<Set<string>>(
    new Set(),
  );
  const [draftExcludedMessageIds, setDraftExcludedMessageIds] = useState<
    Set<string>
  >(new Set());
  const [contextEditorSaving, setContextEditorSaving] = useState(false);
  const [showScrollNavigation, setShowScrollNavigation] = useState(false);
  const draftKey = externalDraftKey ?? viewKey;
  const [recalledDraft, setRecalledDraft] =
    useState<RecalledComposerDraft | null>(null);

  useEffect(() => {
    if (chatModel !== undefined) {
      applyCommittedModel(chatModel);
    }
  }, [applyCommittedModel, chatModel]);

  useEffect(() => {
    onHistoryChange?.(runtime.authoritativeHistory);
  }, [onHistoryChange, runtime.authoritativeHistory]);

  useEffect(() => {
    setRecalledDraft(null);
    setExcludedMessageIds(new Set());
    setDraftExcludedMessageIds(new Set());
  }, [draftKey]);

  useEffect(() => {
    if (!contextEditorOpen || !conversationId || unmaterialized) return;
    void conversationApi
      .getProjectionExclusions(target, conversationId)
      .then(({ message_ids }) => {
        const next = new Set(message_ids);
        setExcludedMessageIds(next);
        setDraftExcludedMessageIds(new Set(next));
      })
      .catch((error) => {
        toast.error(error instanceof Error ? error.message : String(error));
        onContextEditorClose?.();
      });
  }, [
    contextEditorOpen,
    conversationId,
    onContextEditorClose,
    target,
    unmaterialized,
  ]);

  const saveContextEditor = useCallback(async () => {
    if (!conversationId) return;
    setContextEditorSaving(true);
    try {
      const result = await conversationApi.updateProjectionExclusions(
        target,
        conversationId,
        Array.from(draftExcludedMessageIds),
      );
      setExcludedMessageIds(new Set(result.message_ids));
      toast.success(tx("chat.contextEditor.saved", "模型上下文已更新"));
      onContextEditorClose?.();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setContextEditorSaving(false);
    }
  }, [conversationId, draftExcludedMessageIds, onContextEditorClose, target]);

  const scrollMessages = useCallback(
    (position: "top" | "up" | "down" | "bottom") => {
      viewportRef.current?.navigate(position);
    },
    [],
  );
  const loadOlderAtTop = useCallback(() => {
    if (!hasMoreBefore || loadingOlder) return;
    viewportRef.current?.beforePrepend();
    void loadOlder();
  }, [hasMoreBefore, loadOlder, loadingOlder]);
  const handleScrollState = useCallback(
    (following: boolean, scrolling: boolean, top: number) => {
      setShowScrollNavigation(scrolling);
      writeChatViewScrollCache(runtimeOwnerKey, conversationId, top, following);
    },
    [runtimeOwnerKey, conversationId],
  );
  useEffect(() => {
    const root = document.documentElement;
    const viewport = window.visualViewport;
    const syncVisualViewport = () => {
      const height = viewport?.height ?? window.innerHeight;
      if (Number.isFinite(height) && height > 0)
        root.style.setProperty(
          "--rua-chat-visual-height",
          `${Math.round(height)}px`,
        );
    };
    syncVisualViewport();
    viewport?.addEventListener("resize", syncVisualViewport);
    window.addEventListener("resize", syncVisualViewport);
    return () => {
      viewport?.removeEventListener("resize", syncVisualViewport);
      window.removeEventListener("resize", syncVisualViewport);
      root.style.removeProperty("--rua-chat-visual-height");
    };
  }, []);

  const submitAndFollowLatest = useCallback(
    async (text: string, attachments?: ComposerAttachment[]) => {
      viewportRef.current?.follow();
      return submitMessage(text, attachments).then((accepted) => {
        if (accepted === true) onSubmitAccepted?.();
        return accepted;
      });
    },
    [onSubmitAccepted, submitMessage],
  );

  const handleStartPrompt = useCallback(
    (text: string) => {
      void submitAndFollowLatest(text);
    },
    [submitAndFollowLatest],
  );
  const handleRollback = useCallback(
    async (beforeTurnId: string, userText: string) => {
      const user = runtime.projection.items.find(
        (item) => item.kind === "user" && item.turn_id === beforeTurnId,
      );
      const draft =
        user?.kind === "user"
          ? recalledUserDraft(user.content)
          : { text: userText, attachments: [] };
      await rollback(beforeTurnId);
      setRecalledDraft({ draftKey, ...draft });
    },
    [rollback, draftKey, runtime.projection.items],
  );
  const handleBranch = useCallback(
    async (beforeTurnId: string, userText: string) => {
      const branchId = await branch(beforeTurnId);
      if (!branchId) return;
      drafts.save(`${runtimeOwnerKey}:${branchId}`, userText);
      try {
        if (projectId) {
          navigateWithPageTransition(
            navigate,
            projectChatPath(projectId, branchId),
          );
        } else {
          const ready = await prepareChatTransition(selectedAgent, branchId);
          if (ready) {
            navigateWithPageTransition(
              navigate,
              agentChatPath(selectedAgent, branchId),
            );
          }
        }
      } catch (err) {
        toast.error(err instanceof Error ? err.message : String(err));
      }
    },
    [branch, runtimeOwnerKey, projectId, selectedAgent, navigate],
  );

  const handleReclaimQueuedSubmission = useCallback(
    (queuedSubmissionId: string) => {
      const submission = reclaimQueuedSubmission(queuedSubmissionId);
      if (!submission) return;
      setRecalledDraft({
        draftKey,
        text: submission.text,
        attachments: submission.attachments,
      });
    },
    [draftKey, reclaimQueuedSubmission],
  );
  const submissionBlocked =
    (submitDisabled || contextEditorOpen) && !activeAcpRunner;
  const turnActive = runtime.projection.turnActivity.kind === "active";
  const backgroundWork = runtime.history?.control?.background_work ?? null;
  const openBackgroundChat = useCallback(
    (chat: TaskChatAddress) => {
      const scope = chat?.scope;
      const chatId = chat?.chat_id;
      if (!scope || typeof chatId !== "string" || !chatId) return;
      navigateWithPageTransition(
        navigate,
        scope.kind === "project"
          ? projectChatPath(scope.project_id, chatId)
          : agentChatPath(scope.agent_id, chatId),
      );
    },
    [navigate],
  );
  const collaboration = (() => {
    const value = runtime.history?.meta?.collaboration;
    return value && typeof value === "object"
      ? (value as CollaborationChatRelation)
      : undefined;
  })();
  const collaborationNeedsAttention =
    collaboration?.source_available === false ||
    (collaboration !== undefined &&
      collaboration.configuration.status !== "ready");
  const sourceScope = collaboration?.source.scope as
    | { kind: "agent"; agent_id: string }
    | { kind: "project"; project_id: string }
    | undefined;
  const sourceChatId = collaboration?.source.chat_id;
  const sourcePath =
    typeof sourceChatId === "string" && sourceScope
      ? sourceScope.kind === "project"
        ? projectChatPath(sourceScope.project_id, sourceChatId)
        : agentChatPath(sourceScope.agent_id, sourceChatId)
      : undefined;

  return (
    <section
      className={[
        "rua-owned-surface",
        collaboration && "rua-owned-surface--collaboration-alert",
      ]
        .filter(Boolean)
        .join(" ")}
      data-rua-owned-chat="runtime"
    >
      {collaboration ? (
        <aside
          className={`rua-owned-collaboration-relationship${
            collaborationNeedsAttention ? " is-warning" : ""
          }`}
          aria-label={tx(
            "chat.collaborationRelationship",
            "Collaboration relationship",
          )}
        >
          <strong>
            {collaboration.executor.kind === "acp"
              ? t("chat.collaborationExecutorAcp", {
                  runner: collaboration.executor.runner,
                })
              : t("chat.collaborationExecutorAgent", {
                  agent: collaboration.executor_agent_id,
                })}
          </strong>
          {sourcePath && collaboration.source_available !== false ? (
            <button
              type="button"
              onClick={() => navigateWithPageTransition(navigate, sourcePath)}
            >
              {tx(
                "chat.collaborationBackToSource",
                "Back to source conversation",
              )}
            </button>
          ) : null}
          {collaboration.source_available === false ? (
            <span>{tx("chat.sourceUnavailable", "Source unavailable")}</span>
          ) : null}
          {collaboration.configuration.status !== "ready" ? (
            <span>
              {tx("chat.collaborationConfiguration", "Inherited configuration")}
              : {collaboration.configuration.status}
            </span>
          ) : null}
        </aside>
      ) : null}
      <div className="rua-owned-surface-messages" ref={messagesRef}>
        {runtime.loading && runtime.projection.items.length === 0 ? (
          <div className="rua-owned-surface-loading" />
        ) : (
          <ConversationMessageList
            key={viewKey}
            viewKey={viewKey}
            scrollRef={messagesRef}
            viewportRef={viewportRef}
            onScrollState={handleScrollState}
            onTop={loadOlderAtTop}
            onLocateTurn={runtime.locateTurn}
            historyGap={runtime.historyGap}
            onLoadHistoryGap={runtime.loadHistoryGap}
            initialScroll={readChatViewScrollCache(
              runtimeOwnerKey,
              conversationId,
            )}
            projection={runtime.projection}
            agentId={agentId}
            mediaScope={mediaScope}
            approvalSnapshot={runtime.approvalSnapshot}
            onSelectVariant={runtime.selectVariant}
            onRegenerate={runtime.regenerate}
            blockPreferences={blockPreferences}
            activeAcpRunner={activeAcpRunner}
            contextEditorOpen={contextEditorOpen}
            excludedMessageIds={excludedMessageIds}
            draftExcludedMessageIds={draftExcludedMessageIds}
            onDraftExcludedMessageIdsChange={setDraftExcludedMessageIds}
            onStartPrompt={submitDisabled ? undefined : handleStartPrompt}
            onRollback={handleRollback}
            onBranch={handleBranch}
          />
        )}
      </div>
      {contextEditorOpen && (
        <div
          className="rua-owned-context-editor"
          role="region"
          aria-label={tx("chat.contextEditor.title", "编辑模型上下文")}
        >
          <div>
            <strong>{tx("chat.contextEditor.title", "编辑模型上下文")}</strong>
            <span>
              {tx(
                "chat.contextEditor.help",
                "勾选要从后续模型调用中排除的 Agent 输出。原始对话记录不会删除。",
              )}
            </span>
          </div>
          <div className="rua-owned-context-editor-actions">
            <button
              type="button"
              onClick={onContextEditorClose}
              disabled={contextEditorSaving}
            >
              {tx("common.cancel", "取消")}
            </button>
            <button
              type="button"
              onClick={() => void saveContextEditor()}
              disabled={contextEditorSaving}
            >
              {contextEditorSaving
                ? tx("common.saving", "保存中…")
                : tx("common.save", "保存")}
            </button>
          </div>
        </div>
      )}
      {runtime.contextCompactionStatus?.active && (
        <div className="rua-owned-surface-compact-status" role="status">
          <RefreshCw size={15} aria-hidden="true" />
          <span>
            {runtime.contextCompactionStatus.message ||
              tx("chat.contextCompaction.blocking", "正在压缩上下文...")}
          </span>
        </div>
      )}
      <BackgroundWorkStatus
        view={backgroundWork}
        turnActive={turnActive}
        currentChatId={conversationId ?? null}
        onOpenChat={openBackgroundChat}
        onOpenTasks={onOpenTasks}
      />
      {runtime.error && (
        <div className="rua-owned-surface-error" role="alert">
          <AlertCircle size={16} aria-hidden="true" />
          <span>{runtime.error}</span>
          <button
            type="button"
            className="rua-owned-surface-error-close"
            aria-label={tx("chat.error.dismiss", "Dismiss error")}
            title={tx("chat.error.dismiss", "Dismiss error")}
            onClick={runtime.clearError}
          >
            <X size={14} aria-hidden="true" />
          </button>
        </div>
      )}
      <div className="rua-owned-surface-composer">
        {showScrollNavigation && (
          <div
            className="rua-owned-scroll-controls"
            role="toolbar"
            aria-label="聊天滚动导航"
          >
            <button
              type="button"
              className="rua-owned-scroll-control"
              aria-label={tx("chat.nav.first", "到顶部")}
              title={tx("chat.nav.first", "到顶部")}
              onClick={() => scrollMessages("top")}
            >
              <ArrowUpToLine size={16} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="rua-owned-scroll-control"
              aria-label={tx("chat.nav.prev", "到上一条")}
              title={tx("chat.nav.prev", "到上一条")}
              onClick={() => scrollMessages("up")}
            >
              <ChevronUp size={16} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="rua-owned-scroll-control"
              aria-label={tx("chat.nav.next", "到下一条")}
              title={tx("chat.nav.next", "到下一条")}
              onClick={() => scrollMessages("down")}
            >
              <ChevronDown size={16} aria-hidden="true" />
            </button>
            <button
              type="button"
              className="rua-owned-scroll-control"
              aria-label={tx("chat.nav.last", "到最下")}
              title={tx("chat.nav.last", "到最下")}
              onClick={() => scrollMessages("bottom")}
            >
              <ArrowDownToLine size={16} aria-hidden="true" />
            </button>
          </div>
        )}
        <PendingQueue
          items={runtime.queuedSubmissions}
          onSubmitNow={runtime.submitQueuedNow}
          onEdit={runtime.editQueuedSubmission}
          onReclaim={handleReclaimQueuedSubmission}
          onCancel={runtime.cancelQueuedSubmission}
        />
        <ApprovalTray
          pending={runtime.approvalSnapshot.pending}
          onApprove={runtime.approveTool}
        />
        <OwnedConversationComposer
          chatId={conversationId!}
          mediaScope={mediaScope}
          uploadAgentId={selectedAgent}
          chatModel={chatModel}
          currentAuthorityEpoch={
            runtime.history?.control?.placement?.authority_epoch ?? null
          }
          draftKey={draftKey}
          placeholder={inputPlaceholder}
          recalledDraft={recalledDraft}
          turnActive={turnActive}
          disabled={!conversationId}
          submitDisabled={submissionBlocked}
          submitDisabledReason={
            submissionBlocked ? submitDisabledReason : undefined
          }
          attachmentsDisabled={attachmentsDisabled}
          onSubmit={submitAndFollowLatest}
          onStop={runtime.stop}
          deviceChip={composerDeviceChip}
          leftTools={composerLeftTools}
        />
      </div>
    </section>
  );
}
