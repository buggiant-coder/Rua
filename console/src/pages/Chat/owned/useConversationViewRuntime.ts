import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type ApprovalDecisionRequest,
  type ApprovalRequest,
  type ApprovalReuseMatcher,
  type ApprovalSnapshot,
  type ApprovalStatus,
} from "@/api/modules/console";
import {
  conversationApi,
  type ConversationApiTarget,
} from "@/api/modules/conversation";
import type { ConversationAddress } from "../conversationAddress";
import {
  useConversationIdentity,
  type ConversationIdentity,
} from "./useConversationIdentity";
import { getBrowserViewerId } from "@/deviceNetwork/viewerIdentity";
import { isDesktopShell } from "@/desktop/runtime";
import { isMobile } from "@/utils/isMobile";
import { useRouteViewerVisible } from "@/routes/RouteActivity";
import type {
  ACPPermissionRequest,
  ChatHistory,
  ConversationControlView,
  Message,
  ModelSlot,
} from "@/api/types";
import { resolveChatPlacement } from "../chatPlacementState";
import { normalizeContentUrls } from "../utils";
import {
  deviceDeliveryIdFromMessage,
  submissionIdFromMessage,
  useConversationLiveActivityViewerLifecycle,
} from "./conversationLiveActivityViewer";
import type { ComposerAttachment } from "./OwnedConversationComposer";
import {
  clearPendingSelectedVariant,
  getPendingSelectedVariant,
  setPendingSelectedVariant,
  type SelectedAssistantVariant,
} from "./api";
import {
  finishConversationStream,
  foldConversationStreamEvent,
  initialConversationStreamFold,
  parseConversationStreamChunk,
  type ConversationStreamEvent,
  type ConversationStreamFold,
} from "./stream";
import { readSseChunks, SseReadIdleTimeoutError } from "./sse";
import { HistoryReconciliationWorker } from "./historyReconciliationWorker";
import {
  assistantResponseText,
  createConversationProjector,
  type AssistantResponseVariant,
  type ConversationAssistantResponse,
  type ConversationOutputMessage,
  type ConversationProjection,
} from "./projection";
import {
  liveRunMessages,
  liveMessageCoveredByHistory,
  mergeLiveMessagesIntoHistory,
  remotePendingDeliveryCoveredByHistory,
  type RemotePendingDelivery,
} from "./renderState";
import {
  ConversationConvergence,
  activeRunCoveredByHistory,
  type ConversationActivityCursor,
  type ActivityUpdate,
} from "./conversationConvergence";
import { reconcileStableConversationHistory as mergeStableHistory } from "./conversationStateConvergence";
import type { ConversationActivityWake } from "./useConversationActivityWake";
import {
  joinHistoryWindow,
  historyWindowGap,
  preserveLoadedHistory,
} from "./locatedHistory";
import { triggerTargetedSyncCycle } from "./conversationLiveActivityViewer";

type RuntimeStatus =
  | "idle"
  | "loading"
  | "preparing"
  | "streaming"
  | "stopping";
export type ContextCompactionStatus = {
  active: boolean;
  message?: string;
};
export const INITIAL_CHAT_TURN_PAGE_SIZE = 20;
const CHAT_VIEW_RUNTIME_POLICY = {
  maxRetainedConversations: 8,
  mobileMaxBytes: 128 * 1024 * 1024,
  desktopMaxBytes: 256 * 1024 * 1024,
} as const;
const RETAINED_COMPLETED_LIVE_RUN_LIMIT = INITIAL_CHAT_TURN_PAGE_SIZE;
const STREAM_RENDER_INTERVAL_MS = 16;
const CONVERSATION_LIVE_READ_IDLE_TIMEOUT_MS = 45_000;
const CONVERSATION_LIVE_RECONNECT_DELAY_MS = 1_000;

type ActiveRegeneration = {
  turnId: string;
  variantId: string;
  baselineVariantIds: string[];
  fold: ConversationStreamFold;
};

type ReconnectRegenerationTarget = {
  turnId: string;
  variantId: string;
  baselineVariantIds: string[];
};

export type QueuedSubmission = {
  id: string;
  submissionId?: string;
  text: string;
  attachments: ComposerAttachment[];
  messageId?: string;
  source?: "transport" | "draft";
  deliveryState?: "sending" | "failed" | "unknown";
  serverQueued?: boolean;
  deviceDeliveryId?: string;
  targetDeviceId?: string;
  deviceDeliverySequence?: number;
  acceptedAt?: string;
  editable?: boolean;
  deletable?: boolean;
};

export type AcpHistoryPreviewPayload = {
  id: number;
  text: string;
};

type RemotePendingSubmission = {
  device_delivery_id: string;
  target_device_id: string;
  device_delivery_sequence: number;
  accepted_at?: string;
  submission_id?: string;
  input_id?: string;
  turn_id?: string;
  message: Message;
};

type RetainedCompletedLiveRun = {
  key: string;
  user: Message;
  fold: ConversationStreamFold;
  acpOriginRunner: string | null;
};

function remotePendingDeliveryFromSubmission(
  submission: RemotePendingSubmission,
): RemotePendingDelivery {
  return {
    deviceDeliveryId: submission.device_delivery_id,
    targetDeviceId: submission.target_device_id,
    deviceDeliverySequence: submission.device_delivery_sequence,
    acceptedAt: submission.accepted_at,
    submissionId: submission.submission_id,
    inputId: submission.input_id,
    turnId: submission.turn_id,
    message: submission.message,
  };
}

type SubmitChatResult = {
  accepted?: boolean;
  submission_id?: string;
  mode?: "started" | "queued" | "rejected" | "unconfirmed";
  chat_id?: string;
  input_id?: string;
  device_delivery_id?: string;
  target_device_id?: string;
  device_delivery_sequence?: number;
  accepted_at?: string;
  detail?: string;
  failure_kind?:
    | "business"
    | "device_offline"
    | "delivery_unknown"
    | "version_incompatible"
    | "protocol_error"
    | "invalid_request";
  control?: ConversationControlView | null;
};

function streamFoldSawTerminal(fold: ConversationStreamFold): boolean {
  return fold.sawTerminal;
}

function streamEventTerminatesRun(event: ConversationStreamEvent): boolean {
  return event.kind === "turn_terminal" || event.kind === "parse_error";
}

function activeRegenerationSawTerminal(active: ActiveRegeneration): boolean {
  return active.fold.sawTerminal;
}

function activeRegenerationCoveredByHistory(
  history: ChatHistory | null,
  active: ActiveRegeneration,
): boolean {
  if (!active.fold.sawTerminal) return false;
  const stableVariantIds = new Set<string>();
  for (const message of history?.messages || []) {
    if (message.role === "user" || message.turn_id !== active.turnId) continue;
    if (typeof message.selected_variant_id === "string") {
      stableVariantIds.add(message.selected_variant_id);
    }
    const variants = Array.isArray(message.assistant_response_variants)
      ? (message.assistant_response_variants as AssistantResponseVariant[])
      : [];
    for (const variant of variants) {
      stableVariantIds.add(variant.variant_id);
    }
  }
  if (stableVariantIds.has(active.variantId)) return true;
  const baselineVariantIds = new Set(active.baselineVariantIds);
  return [...stableVariantIds].some(
    (variantId) => !baselineVariantIds.has(variantId),
  );
}

function useCoalescedPublisher<T>(
  publishValue: (value: T) => void,
  shouldPublishImmediately: (value: T) => boolean,
) {
  const pendingRef = useRef<T | null>(null);
  const timerRef = useRef<number | null>(null);
  const lastFlushAtRef = useRef(0);

  const clear = useCallback(() => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    pendingRef.current = null;
  }, []);

  const take = useCallback(() => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    const pending = pendingRef.current;
    pendingRef.current = null;
    return pending;
  }, []);

  const flush = useCallback(() => {
    timerRef.current = null;
    const pending = pendingRef.current;
    if (!pending) return;
    pendingRef.current = null;
    lastFlushAtRef.current = Date.now();
    publishValue(pending);
  }, [publishValue]);

  const publish = useCallback(
    (value: T, immediate = false) => {
      pendingRef.current = value;
      if (immediate || shouldPublishImmediately(value)) {
        clear();
        lastFlushAtRef.current = Date.now();
        publishValue(value);
        return;
      }

      const elapsed = Date.now() - lastFlushAtRef.current;
      if (elapsed >= STREAM_RENDER_INTERVAL_MS) {
        clear();
        lastFlushAtRef.current = Date.now();
        publishValue(value);
        return;
      }

      if (timerRef.current === null) {
        timerRef.current = window.setTimeout(
          flush,
          STREAM_RENDER_INTERVAL_MS - elapsed,
        );
      }
    },
    [clear, flush, publishValue, shouldPublishImmediately],
  );

  return { clear, publish, take };
}

export type ApproveOptions = {
  decision?: "run_once" | "allow_chat" | "allow_agent" | "reject";
  matcher?: ApprovalReuseMatcher;
  rejectionFeedback?: string;
  acpOptionId?: string;
};

export type ConversationViewRuntime = {
  identity: ConversationIdentity;
  executionAgentId: string;
  history: ChatHistory | null;
  authoritativeHistory: ChatHistory | null;
  projection: ConversationProjection;
  approvalSnapshot: ApprovalSnapshot;
  loading: boolean;
  contextCompactionStatus: ContextCompactionStatus | null;
  loadingOlder: boolean;
  hasMoreBefore: boolean;
  error: string | null;
  authorityPlacementDeviceId: string | null;
  authorityPlacementLocked: boolean;
  clearError: () => void;
  submit: (
    text: string,
    attachments?: ComposerAttachment[],
  ) => Promise<boolean | "unknown">;
  stop: () => Promise<void>;
  queuedSubmissions: QueuedSubmission[];
  submitQueuedNow: (queuedSubmissionId: string) => Promise<void>;
  reclaimQueuedSubmission: (
    queuedSubmissionId: string,
  ) => QueuedSubmission | null;
  cancelQueuedSubmission: (queuedSubmissionId: string) => void;
  editQueuedSubmission: (
    queuedSubmissionId: string,
    nextText: string,
  ) => Promise<void>;
  loadOlder: () => Promise<void>;
  locateTurn: (turnId: string) => Promise<void>;
  loadHistoryGap: () => Promise<void>;
  historyGap: { beforeMessageId: string; count: number } | null;
  reload: () => void;
  applyCommittedModel: (model: ModelSlot | null) => void;
  selectVariant: (turnId: string, variantId: string) => Promise<void>;
  regenerate: (turnId: string, userText: string) => Promise<void>;
  rollback: (beforeTurnId: string) => Promise<void>;
  branch: (beforeTurnId: string) => Promise<string>;
  approveTool: (
    approvalRequestId: string,
    approve: boolean,
    options?: ApproveOptions,
  ) => Promise<void>;
};

const DEFAULT_USER_ID = "default";
const DEFAULT_CHANNEL = "console";
export const CHAT_SUBMITTED_EVENT = "rua-chat-submitted";

export type ChatSubmittedEventDetail = {
  agentId: string;
  chatId: string;
  title: string;
};

type ChatViewCacheEntry = {
  estimatedBytes: number;
  history: ChatHistory;
  hasDurableHistory: boolean;
  locatedHistory: ChatHistory | null;
  approvalSnapshot: ApprovalSnapshot;
  retainedCompletedLiveRuns: RetainedCompletedLiveRun[];
  activeUser: Message | null;
  streamFold: ConversationStreamFold | null;
  activityCursor: ConversationActivityCursor | null;
  updatedAt: number;
  scrollTop?: number;
  followLatest?: boolean;
};

const chatViewCache = new Map<string, ChatViewCacheEntry>();
let chatViewCacheBytes = 0;
let chatViewObjectBytes = new WeakMap<object, number>();
let chatViewCacheByteLimit: number | null = null;

export function getChatViewCacheByteLimit(): number {
  if (chatViewCacheByteLimit !== null) return chatViewCacheByteLimit;
  const browser = typeof navigator === "undefined" ? undefined : navigator;
  const userAgent = browser?.userAgent ?? "";
  const mobile =
    isMobile() ||
    /Android|iPhone|iPad|iPod|Mobile/i.test(userAgent) ||
    (browser?.platform === "MacIntel" && browser.maxTouchPoints > 1);
  const desktop =
    !mobile &&
    (isDesktopShell() || /Windows NT|Macintosh|X11|CrOS/.test(userAgent));
  // Resolve once per page: viewport size is not a memory signal. Unknown hosts
  // use the mobile budget; these weights exclude DOM and decoded media memory.
  chatViewCacheByteLimit = desktop
    ? CHAT_VIEW_RUNTIME_POLICY.desktopMaxBytes
    : CHAT_VIEW_RUNTIME_POLICY.mobileMaxBytes;
  return chatViewCacheByteLimit;
}

// Runtime snapshots are immutable. Reuse unchanged subtrees and count string
// lengths without allocating serialized copies. This estimates retained data,
// not total browser memory (DOM, decoded media, and engine overhead differ).
function estimateChatViewBytes(value: unknown): number {
  if (typeof value === "string") return value.length * 2;
  if (typeof value === "number") return 8;
  if (typeof value === "boolean") return 4;
  if (!value || typeof value !== "object") return 0;

  const cached = chatViewObjectBytes.get(value);
  if (cached !== undefined) return cached;

  let bytes = 32;
  if (Array.isArray(value)) {
    for (const item of value) bytes += 8 + estimateChatViewBytes(item);
  } else {
    for (const key of Object.keys(value)) {
      bytes +=
        key.length * 2 +
        8 +
        estimateChatViewBytes((value as Record<string, unknown>)[key]);
    }
  }
  chatViewObjectBytes.set(value, bytes);
  return bytes;
}

function approvalRequestFromAcp(
  request: ACPPermissionRequest,
): ApprovalRequest {
  const input = JSON.stringify(request.tool_input);
  return {
    id: request.id,
    agent_id: request.agent_id,
    chat_id: request.chat_id,
    turn_id: "",
    tool_call_id: request.tool_call_id,
    tool_name: request.tool_name,
    status: "pending",
    trigger: {
      source: "acp_permission",
      message: `ACP runner ${request.runner} requests permission`,
    },
    tool_call: {
      display: input ? `${request.tool_name} ${input}` : request.tool_name,
      args: request.tool_input,
    },
    created_at: "",
    reuse_options: [],
    origin: "acp",
    runner: request.runner,
    acp_session_id: request.session_id,
    acp_options: request.options,
    acp_task: request.task,
  };
}

async function loadApprovalSnapshot(
  target: ConversationApiTarget,
  chatId: string,
  acpActive: boolean,
): Promise<ApprovalSnapshot> {
  const [ruaSnapshot, acpSnapshot] = await Promise.all([
    conversationApi.getApprovalSnapshot(target, chatId),
    acpActive
      ? conversationApi.getAcpPermissionSnapshot(target, chatId)
      : Promise.resolve({ pending: [] }),
  ]);
  return {
    pending: [
      ...ruaSnapshot.pending,
      ...acpSnapshot.pending.map(approvalRequestFromAcp),
    ],
    resolved: ruaSnapshot.resolved,
  };
}

function chatViewCacheKey(agentId: string, chatId: string): string {
  return `${agentId}:${chatId}`;
}

function cacheableChatHistory(history: ChatHistory): ChatHistory {
  const {
    active_turn: _activeTurn,
    control: _control,
    status: _status,
    ...stableView
  } = history;
  return {
    ...stableView,
    status: "idle",
    active_turn: null,
  };
}

function readChatViewCache(
  agentId: string,
  chatId: string | undefined,
): ChatViewCacheEntry | null {
  if (!chatId) return null;
  const key = chatViewCacheKey(agentId, chatId);
  const cached = chatViewCache.get(key);
  if (!cached) return null;
  chatViewCache.delete(key);
  chatViewCache.set(key, cached);
  return cached;
}

function writeChatViewCache(
  agentId: string,
  chatId: string | undefined,
  history: ChatHistory | null,
  approvalSnapshot: ApprovalSnapshot,
  retainedCompletedLiveRuns?: RetainedCompletedLiveRun[],
  runtime?: Pick<
    ChatViewCacheEntry,
    "activeUser" | "streamFold" | "activityCursor"
  >,
  locatedHistory?: ChatHistory | null,
) {
  if (!chatId) return;
  const key = chatViewCacheKey(agentId, chatId);
  const previous = chatViewCache.get(key);
  const hasRuntimeState = Boolean(
    runtime?.activeUser || runtime?.streamFold || runtime?.activityCursor,
  );
  if (!history && !previous?.hasDurableHistory && !hasRuntimeState) {
    return;
  }
  const durableHistory = history ?? previous?.history ?? { messages: [] };
  const entry = {
    history: cacheableChatHistory(durableHistory),
    locatedHistory:
      locatedHistory === undefined
        ? previous?.locatedHistory ?? null
        : locatedHistory,
    hasDurableHistory: history !== null || previous?.hasDurableHistory === true,
    approvalSnapshot,
    retainedCompletedLiveRuns:
      retainedCompletedLiveRuns ?? previous?.retainedCompletedLiveRuns ?? [],
    activeUser: runtime ? runtime.activeUser : previous?.activeUser ?? null,
    streamFold: runtime ? runtime.streamFold : previous?.streamFold ?? null,
    activityCursor: runtime
      ? runtime.activityCursor
      : previous?.activityCursor ?? null,
    updatedAt: Date.now(),
    scrollTop: previous?.scrollTop,
    followLatest: previous?.followLatest,
  };
  const estimatedBytes = estimateChatViewBytes(entry);
  chatViewCacheBytes += estimatedBytes - (previous?.estimatedBytes ?? 0);
  chatViewCache.delete(key);
  chatViewCache.set(key, { ...entry, estimatedBytes });

  const byteLimit = getChatViewCacheByteLimit();
  while (
    chatViewCache.size > CHAT_VIEW_RUNTIME_POLICY.maxRetainedConversations ||
    chatViewCacheBytes > byteLimit
  ) {
    const oldest = [...chatViewCache.entries()].find(
      ([, entry]) => !entry.streamFold || entry.streamFold.sawTerminal,
    );
    if (!oldest) break;
    chatViewCacheBytes -= oldest[1].estimatedBytes;
    chatViewCache.delete(oldest[0]);
  }
  if (
    chatViewCache.size > CHAT_VIEW_RUNTIME_POLICY.maxRetainedConversations ||
    chatViewCacheBytes > byteLimit
  ) {
    console.warn("conversation_view_runtime_capacity_unavailable", {
      retainedConversations: chatViewCache.size,
      estimatedBytes: chatViewCacheBytes,
    });
  }
}

export function primeChatViewCache(
  agentId: string,
  chatId: string,
  history: ChatHistory,
) {
  const previous = chatViewCache.get(chatViewCacheKey(agentId, chatId));
  writeChatViewCache(
    agentId,
    chatId,
    history,
    previous?.approvalSnapshot ?? {
      pending: [],
      resolved: [],
    },
  );
}

export function __resetChatViewCacheForTests() {
  chatViewCache.clear();
  chatViewCacheBytes = 0;
  chatViewObjectBytes = new WeakMap();
  chatViewCacheByteLimit = null;
}

export function readChatViewTitleCache(
  agentId: string,
  chatId: string | undefined,
): string | null {
  if (!chatId) return null;
  const cached = chatViewCache.get(chatViewCacheKey(agentId, chatId));
  const title =
    cached?.hasDurableHistory && typeof cached.history.title === "string"
      ? cached.history.title.trim()
      : "";
  return title || null;
}

export function readChatViewScrollCache(
  agentId: string,
  chatId: string | undefined,
): Pick<ChatViewCacheEntry, "scrollTop" | "followLatest"> | null {
  if (!chatId) return null;
  const cached = chatViewCache.get(chatViewCacheKey(agentId, chatId));
  if (!cached) return null;
  return {
    scrollTop: cached.scrollTop,
    followLatest: cached.followLatest,
  };
}

export function writeChatViewScrollCache(
  agentId: string,
  chatId: string | undefined,
  scrollTop: number,
  followLatest: boolean,
) {
  if (!chatId) return;
  const key = chatViewCacheKey(agentId, chatId);
  const cached = chatViewCache.get(key);
  if (!cached) return;
  const { estimatedBytes: previousBytes, ...entry } = cached;
  const next = { ...entry, scrollTop, followLatest, updatedAt: Date.now() };
  const estimatedBytes = estimateChatViewBytes(next);
  chatViewCacheBytes += estimatedBytes - previousBytes;
  chatViewCache.set(key, { ...next, estimatedBytes });
}

function attachmentContent(
  attachment: ComposerAttachment,
): Record<string, unknown> {
  if (!attachment.storedName) {
    throw new Error("Managed attachment is missing its media reference");
  }
  return {
    type: attachment.type,
    media_ref: `media/${attachment.storedName}`,
  };
}

const ATTACHMENT_MARKER_RE =
  /!?\[[^\]\n]*\]\(rua-attachment:\/\/([^)]+)\)|\[Image #(\d+)\]|\[File #(\d+)\]/g;

export function buildUserContent(
  text: string,
  attachments: ComposerAttachment[] = [],
): Record<string, unknown>[] {
  const attachmentsById = new Map(
    attachments.map((attachment) => [attachment.id, attachment]),
  );
  const imageAttachments = attachments.filter(
    (attachment) => attachment.type === "image",
  );
  const fileAttachments = attachments.filter(
    (attachment) => attachment.type === "file" && attachment.marker,
  );
  const used = new Set<string>();
  const content: Record<string, unknown>[] = [];
  let cursor = 0;
  const markerRe = new RegExp(ATTACHMENT_MARKER_RE);
  let match: RegExpExecArray | null;

  while ((match = markerRe.exec(text)) !== null) {
    const index = match.index;
    if (index > cursor) {
      content.push({ type: "text", text: text.slice(cursor, index) });
    }
    const attachmentId = match[1];
    const imageIndex = match[2] ? Number.parseInt(match[2], 10) - 1 : -1;
    const fileIndex = match[3] ? Number.parseInt(match[3], 10) - 1 : -1;
    const attachment = attachmentId
      ? attachmentsById.get(attachmentId)
      : imageIndex >= 0
      ? imageAttachments[imageIndex]
      : fileAttachments[fileIndex];
    if (attachment) {
      content.push(attachmentContent(attachment));
      used.add(attachment.id);
    } else {
      content.push({ type: "text", text: match[0] });
    }
    cursor = index + match[0].length;
  }

  if (cursor < text.length) {
    content.push({ type: "text", text: text.slice(cursor) });
  }

  for (const attachment of attachments) {
    if (!used.has(attachment.id)) {
      content.push(attachmentContent(attachment));
    }
  }

  return content.filter((part) => {
    if (part.type !== "text") return true;
    return typeof part.text === "string" && part.text.length > 0;
  });
}

function userMessage(
  text: string,
  attachments: ComposerAttachment[] = [],
  remoteDeliveryState?: "sending",
  messageId?: string,
  submissionId?: string,
): Message {
  const content = buildUserContent(text, attachments);
  return {
    id: messageId || `user-${crypto.randomUUID()}`,
    role: "user",
    content,
    ...(submissionId ? { metadata: { submission_id: submissionId } } : {}),
    ...(remoteDeliveryState
      ? { remote_delivery_state: remoteDeliveryState }
      : {}),
  };
}

function outputMessages(
  fold: ConversationStreamFold,
): ConversationOutputMessage[] {
  return fold.output.map((msg) => ({
    ...msg,
    role: msg.role,
    type: msg.type,
    content: msg.content,
  }));
}

function assistantVariantOutputFromMessages(
  messages: Message[],
  start: number,
  end: number,
): ConversationOutputMessage[] {
  return messages.slice(start, end).map((msg, offset) => {
    const rest = { ...msg };
    delete rest.assistant_response_variants;
    return {
      ...(rest as Message),
      id:
        typeof rest.id === "string" && rest.id
          ? rest.id
          : `assistant-variant-${start + offset}`,
      role: String(rest.role || "assistant"),
      type: typeof rest.type === "string" ? rest.type : undefined,
      content: Array.isArray(rest.content)
        ? (rest.content as ConversationOutputMessage["content"])
        : rest.content == null
        ? []
        : [{ type: "text", text: String(rest.content) }],
    } as ConversationOutputMessage;
  });
}

function dispatchChatSubmitted(
  agentId: string,
  chatId: string,
  title: string,
): void {
  window.dispatchEvent(
    new CustomEvent<ChatSubmittedEventDetail>(CHAT_SUBMITTED_EVENT, {
      detail: {
        agentId,
        chatId,
        title,
      },
    }),
  );
}

function messageStringId(message: Message): string | null {
  return typeof message.id === "string" && message.id ? message.id : null;
}

function messageTurnId(message: Message | null): string | null {
  return typeof message?.turn_id === "string" && message.turn_id
    ? message.turn_id
    : null;
}

function queuedSubmissionIdentityMatchesUserMessage(
  queued: QueuedSubmission,
  message: Message,
): boolean {
  const submissionId = submissionIdFromMessage(message);
  if (submissionId && queued.submissionId === submissionId) return true;
  const messageId = messageStringId(message);
  if (messageId && queued.messageId === messageId) return true;
  const deviceDeliveryId = deviceDeliveryIdFromMessage(message);
  return Boolean(
    deviceDeliveryId && queued.deviceDeliveryId === deviceDeliveryId,
  );
}

function removeQueuedSubmissionForUserMessage(
  current: QueuedSubmission[],
  message: Message,
): QueuedSubmission[] {
  const identityMatchIndex = current.findIndex((item) =>
    queuedSubmissionIdentityMatchesUserMessage(item, message),
  );
  if (identityMatchIndex >= 0) {
    return current.filter((_, index) => index !== identityMatchIndex);
  }
  return current;
}

function queuedSubmissionsFromControl(
  control: ConversationControlView,
): QueuedSubmission[] {
  return (control.queue?.entries || []).map((entry) => ({
    id: entry.input_id,
    submissionId: entry.submission_id,
    text: entry.text,
    attachments: [],
    serverQueued: true,
    deviceDeliveryId: entry.device_delivery_id ?? undefined,
    editable: entry.editable,
    deletable: entry.deletable,
  }));
}

function mergeControlQueueSubmissions(
  current: QueuedSubmission[],
  control: ConversationControlView,
): QueuedSubmission[] {
  const authorityQueue = queuedSubmissionsFromControl(control);
  if (authorityQueue.length === 0) {
    return current.filter((item) => !item.serverQueued);
  }
  const authoritySubmissionIds = new Set(
    authorityQueue
      .map((item) => item.submissionId)
      .filter((id): id is string => Boolean(id)),
  );
  const optimistic = current.filter(
    (item) =>
      !item.serverQueued &&
      (!item.submissionId || !authoritySubmissionIds.has(item.submissionId)),
  );
  return [...authorityQueue, ...optimistic];
}

function authorityPlacementDeviceIdFromControl(
  control: ConversationControlView | null | undefined,
): string | null | undefined {
  if (!control) return undefined;
  const deviceId = control?.placement?.authority_device_id;
  return typeof deviceId === "string" && deviceId ? deviceId : null;
}

function shouldRetainCompletedActiveRunAfterRefresh(
  nextHistory: ChatHistory | null,
  activeUser: Message | null,
  fold: ConversationStreamFold | null,
): boolean {
  if (!fold || !fold.sawTerminal) return false;
  if (fold.output.length === 0) return false;
  return !activeRunCoveredByHistory(nextHistory, activeUser, fold);
}

function stableHistoryKeepsActiveRunOpen(
  history: ChatHistory | null,
  activeUser: Message | null,
  fold: ConversationStreamFold | null,
): boolean {
  if (history?.status !== "running") return false;
  const activeTurnId = history.active_turn?.turn_id;
  const streamedTurnId = activeUser
    ? messageTurnId(activeUser)
    : fold?.output.map(messageTurnId).find(Boolean) || null;
  return !activeTurnId || !streamedTurnId || activeTurnId === streamedTurnId;
}

function remoteDeliveryErrorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (error && typeof error === "object") {
    const detail = (error as Record<string, unknown>).detail;
    if (typeof detail === "string" && detail.trim()) return detail;
    const message = (error as Record<string, unknown>).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return "Remote device did not acknowledge this message";
}

function mergeOlderHistory(
  current: ChatHistory | null,
  older: ChatHistory,
): ChatHistory {
  if (!current) return older;
  return {
    ...current,
    messages: [...(older.messages || []), ...(current.messages || [])],
    oldest_turn_ordinal:
      older.oldest_turn_ordinal ?? current.oldest_turn_ordinal,
    newest_turn_ordinal:
      current.newest_turn_ordinal ?? older.newest_turn_ordinal,
    has_more_before: Boolean(older.has_more_before),
  };
}

function withCompletedLiveRun(
  base: ChatHistory | null,
  user: Message | null,
  fold: ConversationStreamFold,
  acpOriginRunner?: string | null,
): ChatHistory | null {
  if (!user || !fold.sawTerminal) return base;
  const liveMessages = liveRunMessages(user, fold, acpOriginRunner);
  if (liveMessages.length === 0) return base;
  const appended = liveMessages.filter((message) => {
    return !liveMessageCoveredByHistory(message, base?.messages || []);
  });
  if (appended.length === 0) return base;
  return {
    ...(base || {}),
    status: "idle",
    messages: mergeLiveMessagesIntoHistory(base?.messages || [], appended),
  };
}

function completedLiveRunKey(
  user: Message,
  fold: ConversationStreamFold,
): string | null {
  const submissionId = submissionIdFromMessage(user);
  if (submissionId) return `submission:${submissionId}`;
  const deviceDeliveryId = deviceDeliveryIdFromMessage(user);
  if (deviceDeliveryId) return `delivery:${deviceDeliveryId}`;
  const id = messageStringId(user);
  if (id) return `message:${id}`;
  const turnId = messageTurnId(user) ?? fold.terminalTurnId;
  return turnId ? `turn:${turnId}` : null;
}

function validChatHistory(value: unknown): value is ChatHistory {
  return Boolean(
    value &&
      typeof value === "object" &&
      Array.isArray((value as ChatHistory).messages),
  );
}

function withActiveRegeneration(
  base: ChatHistory | null,
  active: ActiveRegeneration | null,
): ChatHistory | null {
  if (!active) return base;
  const messages = [...(base?.messages || [])];
  const targetIndex = messages.findIndex(
    (msg) => msg.role !== "user" && msg.turn_id === active.turnId,
  );
  if (targetIndex < 0) return base;

  const target = messages[targetIndex];
  let responseEnd = targetIndex + 1;
  while (
    responseEnd < messages.length &&
    messages[responseEnd].role !== "user"
  ) {
    responseEnd += 1;
  }
  const existingVariants = Array.isArray(target.assistant_response_variants)
    ? ([...target.assistant_response_variants] as AssistantResponseVariant[])
    : [
        {
          variant_id:
            typeof target.selected_variant_id === "string"
              ? target.selected_variant_id
              : `${active.turnId}-variant-1`,
          output: assistantVariantOutputFromMessages(
            messages,
            targetIndex,
            responseEnd,
          ),
        },
      ];
  const pendingVariant: AssistantResponseVariant = {
    variant_id: active.variantId,
    output: outputMessages(active.fold),
  };
  const nextVariants = [
    ...existingVariants.filter(
      (variant) => variant.variant_id !== active.variantId,
    ),
    pendingVariant,
  ];

  const replacement = {
    ...target,
    selected_variant_id: active.variantId,
    assistant_response_variants: nextVariants,
  };
  messages.splice(targetIndex, responseEnd - targetIndex, replacement);

  return {
    ...(base || {}),
    messages,
  };
}

import { loadDraftConversationAcp } from "../conversationLifecycle";
import {
  saveSubmitted,
  loadSubmitted,
  loadSubmittedAll,
  clearSubmitted,
} from "../drafts";

type SubmitRequest = {
  project_working_directory_id?: string;
  initial_acp?: import("../conversationLifecycle").DraftAcpSelection;
  input: Message[];
  submission_id: string;
  chat_id: string;
  user_id: string;
  channel: string;
  stream: true;
  model?: ModelSlot;
  turn_id?: string;
  selected_variant_id?: string;
  regenerate_turn_id?: string;
  expected_authority_epoch?: string;
};

function remotePendingStorageKey(agentId: string, chatId: string) {
  return `rua:remote-pending-submissions:${agentId}:${chatId}`;
}

function isRemotePendingSubmission(
  value: unknown,
): value is RemotePendingSubmission {
  if (!value || typeof value !== "object") return false;
  const item = value as Partial<RemotePendingSubmission>;
  return (
    typeof item.device_delivery_id === "string" &&
    item.device_delivery_id.length > 0 &&
    typeof item.target_device_id === "string" &&
    typeof item.device_delivery_sequence === "number" &&
    !!item.message &&
    typeof item.message === "object"
  );
}

function readRemotePendingSubmissions(agentId: string, chatId: string) {
  try {
    const raw = window.localStorage.getItem(
      remotePendingStorageKey(agentId, chatId),
    );
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter(isRemotePendingSubmission)
      : [];
  } catch {
    return [];
  }
}

function writeRemotePendingSubmissions(
  agentId: string,
  chatId: string,
  submissions: RemotePendingSubmission[],
) {
  try {
    const key = remotePendingStorageKey(agentId, chatId);
    if (submissions.length === 0) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, JSON.stringify(submissions));
  } catch {
    return;
  }
}

function buildSubmitRequest(
  conversationId: string,
  message: Message,
  submissionId: string,
  selectedVariant?: SelectedAssistantVariant | null,
  model?: ModelSlot | null,
  expectedAuthorityEpoch?: string,
): SubmitRequest {
  const content = Array.isArray(message.content)
    ? message.content.map(normalizeContentUrls)
    : message.content;
  const submittedMessage: Message = { ...message, content };
  delete submittedMessage.remote_delivery_status;
  delete submittedMessage.remote_delivery_state;
  delete submittedMessage.remote_pending;
  return {
    input: [submittedMessage],
    submission_id: submissionId,
    chat_id: conversationId,
    user_id: DEFAULT_USER_ID,
    channel: DEFAULT_CHANNEL,
    stream: true,
    ...(model ? { model } : {}),
    ...(selectedVariant || {}),
    ...(expectedAuthorityEpoch
      ? { expected_authority_epoch: expectedAuthorityEpoch }
      : {}),
  };
}

function shouldRefreshApprovalSnapshot(
  event: ConversationStreamEvent,
): boolean {
  return (
    event.kind === "message_updated" &&
    event.message.type === "tool_approval_request"
  );
}

function markApprovalDecision(
  history: ChatHistory | null,
  approvalRequestId: string,
  approve: boolean,
  options?: ApproveOptions,
): ChatHistory | null {
  if (!history) return history;
  const nextStatus = approve ? "approved" : "rejected";

  const decidedBy = "webui";
  const decidedAt = new Date().toISOString();
  const rejectionFeedback = !approve ? options?.rejectionFeedback : undefined;
  let changed = false;
  const messages = (history.messages || []).map((message) => {
    const content = Array.isArray(message.content) ? message.content : [];
    let messageChanged = false;
    const nextContent = content.map((part) => {
      if (!part || typeof part !== "object") return part;
      const record = part as Record<string, unknown>;
      const data = record.data;
      if (!data || typeof data !== "object") return part;
      const dataRecord = data as Record<string, unknown>;
      const id =
        dataRecord.approval_request_id ||
        dataRecord.request_id ||
        dataRecord.id;
      if (id === approvalRequestId) {
        messageChanged = true;
        return {
          ...record,
          data: {
            ...dataRecord,
            status: nextStatus,
            decided_by: decidedBy,
            decided_at: decidedAt,
            ...(rejectionFeedback
              ? { rejection_feedback: rejectionFeedback }
              : {}),
          },
        };
      }
      const approval = dataRecord.approval;
      if (approval && typeof approval === "object") {
        const approvalRecord = approval as Record<string, unknown>;
        const approvalId =
          approvalRecord.approval_request_id ||
          approvalRecord.request_id ||
          approvalRecord.id;
        if (approvalId === approvalRequestId) {
          messageChanged = true;
          return {
            ...record,
            data: {
              ...dataRecord,
              approval: {
                ...approvalRecord,
                status: nextStatus,
                decided_by: decidedBy,
                decided_at: decidedAt,
                ...(rejectionFeedback
                  ? { rejection_feedback: rejectionFeedback }
                  : {}),
              },
            },
          };
        }
      }
      return part;
    });
    if (!messageChanged) return message;
    changed = true;
    return {
      ...message,
      status: nextStatus,
      content: nextContent,
    };
  });
  return changed ? { ...history, messages } : history;
}

function textFromUserBeforeTurn(
  projection: ConversationProjection,
  turnId: string,
): string {
  let previousUser = "";
  for (const item of projection.items) {
    if (item.kind === "user") {
      previousUser = item.content
        .filter((part) => part.type === "text" && typeof part.text === "string")
        .map((part) => part.text || "")
        .filter(Boolean)
        .join("\n");
      continue;
    }
    if (item.turn_id === turnId) return previousUser.trim();
  }
  return "";
}

function rolledBackHistory(
  history: ChatHistory | null,
  beforeTurnId: string,
): ChatHistory | null {
  if (!history) return history;
  const firstRemovedIndex = history.messages.findIndex(
    (message) => message.turn_id === beforeTurnId,
  );
  return {
    ...history,
    durable_coverage: undefined,
    active_turn: null,
    control: undefined,
    status: "idle",
    messages:
      firstRemovedIndex < 0 ? [] : history.messages.slice(0, firstRemovedIndex),
  };
}

export interface ConversationViewRuntimeOptions {
  address: ConversationAddress;
  authorityPlacementDeviceId?: string | null;
  currentDeviceId?: string | null;
  activeAcpRunner?: string | null;
  acpHistoryPreview?: AcpHistoryPreviewPayload | null;
  activityWake?: ConversationActivityWake | number | null;
  projectExecutionAgentId?: string;
  unmaterialized?: boolean;
  deferInitialAttach?: boolean;
}

export function useConversationViewRuntime({
  address,
  authorityPlacementDeviceId: externalAuthorityPlacementDeviceId,
  currentDeviceId,
  activeAcpRunner,
  acpHistoryPreview,
  activityWake = null,
  projectExecutionAgentId,
  unmaterialized = false,
  deferInitialAttach = unmaterialized,
}: ConversationViewRuntimeOptions): ConversationViewRuntime {
  const identity = useConversationIdentity(address);
  const {
    chatId: conversationId,
    projectId,
    target: conversationTarget,
    ownerKey: runtimeOwnerKey,
  } = identity;
  const identityRef = useRef(identity);
  identityRef.current = identity;
  const submitDraftModel = unmaterialized;
  const selectedAgent =
    identity.agentId ?? projectExecutionAgentId ?? "default";
  const conversationTargetRef = useRef(conversationTarget);
  conversationTargetRef.current = conversationTarget;
  const restoredSubmissionRef = useRef(
    Boolean(
      conversationId &&
        loadSubmittedAll(`${runtimeOwnerKey}:${conversationId}`).length,
    ),
  );
  if (restoredSubmissionRef.current) {
    deferInitialAttach = false;
  }
  const isCurrentConversation = useCallback(
    () => identityRef.current.viewKey === identity.viewKey,
    [identity.viewKey],
  );
  const browserViewerId = useMemo(getBrowserViewerId, []);
  const viewerVisible = useRouteViewerVisible();
  const initialCacheLoadedRef = useRef(false);
  const initialCachedViewRef = useRef<ChatViewCacheEntry | null>(null);
  if (!initialCacheLoadedRef.current) {
    initialCachedViewRef.current = deferInitialAttach
      ? null
      : readChatViewCache(runtimeOwnerKey, conversationId);
    initialCacheLoadedRef.current = true;
  }
  const [history, setHistory] = useState<ChatHistory | null>(() =>
    initialCachedViewRef.current?.hasDurableHistory
      ? initialCachedViewRef.current.history
      : null,
  );
  const [locatedHistory, setLocatedHistory] = useState<ChatHistory | null>(
    initialCachedViewRef.current?.locatedHistory ?? null,
  );
  const locateVersion = useRef(0);
  const locatedReadBusy = useRef(false);
  useEffect(() => {
    locateVersion.current += 1;
    setLocatedHistory(
      readChatViewCache(runtimeOwnerKey, conversationId)?.locatedHistory ??
        null,
    );
    return () => {
      locateVersion.current += 1;
    };
  }, [runtimeOwnerKey, conversationId]);
  const committedModelRef = useRef<ModelSlot | null | undefined>(
    initialCachedViewRef.current?.hasDurableHistory
      ? initialCachedViewRef.current.history.model
      : undefined,
  );
  const [approvalSnapshot, setApprovalSnapshot] = useState<ApprovalSnapshot>(
    () =>
      initialCachedViewRef.current?.approvalSnapshot ?? {
        pending: [],
        resolved: [],
      },
  );
  const [activeUser, setActiveUser] = useState<Message | null>(
    () => initialCachedViewRef.current?.activeUser ?? null,
  );
  const [streamFold, setStreamFold] = useState<ConversationStreamFold | null>(
    () => initialCachedViewRef.current?.streamFold ?? null,
  );
  const [activeRegeneration, setActiveRegeneration] =
    useState<ActiveRegeneration | null>(null);
  const [status, setStatus] = useState<RuntimeStatus>("idle");
  const [contextCompactionStatus, setContextCompactionStatus] =
    useState<ContextCompactionStatus | null>(null);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [selectedVariantOverrides, setSelectedVariantOverrides] = useState<
    Record<string, string>
  >({});
  const [queuedSubmissions, setQueuedSubmissions] = useState<
    QueuedSubmission[]
  >([]);
  const [remotePendingSubmissions, setRemotePendingSubmissions] = useState<
    RemotePendingSubmission[]
  >([]);
  const activityWakeSeq =
    typeof activityWake === "number"
      ? activityWake
      : activityWake?.sequence ?? 0;
  const recoveryScope =
    typeof activityWake === "object" ? activityWake?.address.scope : null;
  const activityRecoverySeq =
    typeof activityWake === "object" &&
    activityWake?.chat_id === conversationId &&
    (recoveryScope?.kind === "agent"
      ? recoveryScope.agent_id === identity.agentId
      : recoveryScope?.kind === "project" &&
        recoveryScope.project_id === projectId)
      ? activityWake.recoverySequence ?? 0
      : 0;
  const lastActivityRecoveryRef = useRef<{
    viewKey: string;
    sequence: number;
  } | null>(null);
  const [liveActivityWakeSeq, setLiveActivityWakeSeq] = useState(0);
  const conversationIdRef = useRef(conversationId);
  const currentDeviceIdRef = useRef(currentDeviceId);
  currentDeviceIdRef.current = currentDeviceId;
  conversationIdRef.current = conversationId;
  const abortRef = useRef<AbortController | null>(null);
  const submitAbortRef = useRef<AbortController | null>(null);
  const reconnectInFlightRef = useRef<AbortController | null>(null);
  const liveActivityRetryTimerRef = useRef<number | null>(null);
  const activeUserRef = useRef<Message | null>(
    initialCachedViewRef.current?.activeUser ?? null,
  );
  const pendingSubmittedUserRef = useRef<Message | null>(null);
  const [sendingSubmission, setSendingSubmission] = useState<{
    owner: string;
    chatId: string;
    message: Message;
  } | null>(null);
  const streamFoldRef = useRef<ConversationStreamFold | null>(
    initialCachedViewRef.current?.streamFold ?? null,
  );
  const retainedCompletedLiveRunsRef = useRef<RetainedCompletedLiveRun[]>(
    initialCachedViewRef.current?.retainedCompletedLiveRuns ?? [],
  );
  const activeRegenerationRef = useRef<ActiveRegeneration | null>(null);
  const queuedSubmissionsRef = useRef<QueuedSubmission[]>([]);
  const remotePendingSubmissionsRef = useRef<RemotePendingSubmission[]>([]);
  const lastAcpHistoryPreviewIdRef = useRef<number | null>(null);
  const lastActivityWakeSeqRef = useRef(activityWakeSeq);
  const acpHistoryPreviewPreparingRef = useRef(false);
  const convergence = useMemo(
    () => new ConversationConvergence(identity.viewKey),
    [identity.viewKey],
  );
  const activityCursorRef = useRef<ConversationActivityCursor | null>(
    initialCachedViewRef.current?.activityCursor ?? null,
  );
  const loadingOlderRef = useRef(false);
  const conversationResetInitializedRef = useRef(false);
  const {
    clear: clearPendingStreamFoldFlush,
    publish: publishStreamFold,
    take: takePendingStreamFold,
  } = useCoalescedPublisher<ConversationStreamFold>(
    setStreamFold,
    streamFoldSawTerminal,
  );
  const {
    clear: clearPendingActiveRegenerationFlush,
    publish: publishActiveRegeneration,
    take: takePendingActiveRegeneration,
  } = useCoalescedPublisher<ActiveRegeneration>(
    setActiveRegeneration,
    activeRegenerationSawTerminal,
  );

  useEffect(() => {
    activeUserRef.current = activeUser;
  }, [activeUser]);

  useEffect(() => {
    if (submitDraftModel) return;
    committedModelRef.current = history?.model;
  }, [history?.model, submitDraftModel]);

  useEffect(() => {
    streamFoldRef.current = streamFold;
  }, [streamFold]);

  useEffect(() => {
    activeRegenerationRef.current = activeRegeneration;
  }, [activeRegeneration]);

  useEffect(() => {
    return () => {
      abortRef.current?.abort("viewer disconnect");
      submitAbortRef.current?.abort("viewer disconnect");
      if (reconnectInFlightRef.current !== abortRef.current) {
        reconnectInFlightRef.current?.abort("viewer disconnect");
      }
      abortRef.current = null;
      submitAbortRef.current = null;
      reconnectInFlightRef.current = null;
      if (liveActivityRetryTimerRef.current !== null) {
        window.clearTimeout(liveActivityRetryTimerRef.current);
        liveActivityRetryTimerRef.current = null;
      }
      pendingSubmittedUserRef.current = null;
      clearPendingStreamFoldFlush();
      clearPendingActiveRegenerationFlush();
    };
  }, [clearPendingActiveRegenerationFlush, clearPendingStreamFoldFlush]);

  const updateQueuedSubmissions = useCallback(
    (
      updater:
        | QueuedSubmission[]
        | ((current: QueuedSubmission[]) => QueuedSubmission[]),
    ) => {
      setQueuedSubmissions((current) => {
        const next = typeof updater === "function" ? updater(current) : updater;
        queuedSubmissionsRef.current = next;
        return next;
      });
    },
    [],
  );

  const updateRemotePendingSubmissions = useCallback(
    (
      updater:
        | RemotePendingSubmission[]
        | ((current: RemotePendingSubmission[]) => RemotePendingSubmission[]),
    ) => {
      setRemotePendingSubmissions((current) => {
        const next = typeof updater === "function" ? updater(current) : updater;
        remotePendingSubmissionsRef.current = next;
        if (conversationId) {
          writeRemotePendingSubmissions(runtimeOwnerKey, conversationId, next);
        }
        return next;
      });
    },
    [conversationId, runtimeOwnerKey],
  );

  const applyControlProjection = useCallback(
    (control: ConversationControlView) => {
      if (!convergence.acceptControl(control)) return;
      setHistory((current) => ({
        ...(current || { messages: [] }),
        status: control.active_turn ? "running" : "idle",
        active_turn: control.active_turn ?? null,
        ...(control.model !== undefined ? { model: control.model } : {}),
        control,
      }));
      updateQueuedSubmissions((current) =>
        mergeControlQueueSubmissions(current, control),
      );
    },
    [convergence, updateQueuedSubmissions],
  );

  useEffect(() => {
    if (!history?.control) return;
    if (!convergence.acceptControl(history.control)) return;
    updateQueuedSubmissions((current) =>
      mergeControlQueueSubmissions(current, history.control!),
    );
  }, [convergence, history?.control, updateQueuedSubmissions]);

  const chatPlacement = useMemo(
    () =>
      resolveChatPlacement({
        chatId: conversationId,
        meta: history?.meta,
        requireAcpStatus: false,
      }),
    [conversationId, history?.meta],
  );
  const controlAuthorityPlacementDeviceId =
    authorityPlacementDeviceIdFromControl(history?.control);
  const authorityPlacementDeviceId =
    externalAuthorityPlacementDeviceId !== undefined
      ? externalAuthorityPlacementDeviceId
      : controlAuthorityPlacementDeviceId !== undefined
      ? controlAuthorityPlacementDeviceId
      : chatPlacement.authorityPlacementDeviceId;
  const authorityPlacementLocked =
    history?.control?.allowed_controls?.switch_authority !== undefined
      ? !history.control.allowed_controls.switch_authority
      : chatPlacement.authorityPlacementLocked;
  const acpPassthroughActive =
    activeAcpRunner !== undefined && activeAcpRunner !== null;
  const acpPermissionActive =
    history?.control?.acp?.active ??
    (chatPlacement.acpActive || acpPassthroughActive);
  const acpHistoryPreviewId = acpHistoryPreview?.id ?? null;
  const acpHistoryPreviewText = acpHistoryPreview?.text ?? "";

  useEffect(() => {
    const next = conversationId
      ? readRemotePendingSubmissions(runtimeOwnerKey, conversationId)
      : [];
    remotePendingSubmissionsRef.current = next;
    setRemotePendingSubmissions(next);
  }, [conversationId, runtimeOwnerKey]);

  useEffect(() => {
    if (!conversationId) return;
    const pending = loadSubmittedAll(`${runtimeOwnerKey}:${conversationId}`);
    if (!pending.length) return;
    updateQueuedSubmissions((current) => [
      ...current.filter(
        (item) => !pending.some((saved) => saved.submissionId === item.id),
      ),
      ...pending.map(
        (saved): QueuedSubmission => ({
          id: saved.submissionId,
          submissionId: saved.submissionId,
          text: saved.text,
          attachments: saved.attachments,
          source: "transport",
          deliveryState: "unknown",
          editable: false,
          deletable: false,
        }),
      ),
    ]);
    setError("Submission result pending confirmation");
  }, [conversationId, runtimeOwnerKey, updateQueuedSubmissions]);

  useEffect(() => {
    if (!conversationId) return;
    const key = `${runtimeOwnerKey}:${conversationId}`;
    const confirmed = loadSubmittedAll(key).filter(
      (pending) =>
        history?.messages?.some(
          (message) =>
            submissionIdFromMessage(message) === pending.submissionId,
        ) ||
        history?.control?.queue?.entries?.some(
          (entry) => entry.submission_id === pending.submissionId,
        ),
    );
    if (!confirmed.length) return;
    for (const pending of confirmed) clearSubmitted(key, pending.submissionId);
    updateQueuedSubmissions((current) =>
      current.filter(
        (item) =>
          !confirmed.some((pending) => pending.submissionId === item.id),
      ),
    );
    setError(null);
  }, [conversationId, runtimeOwnerKey, history, updateQueuedSubmissions]);

  useEffect(() => {
    if (!conversationId || remotePendingSubmissions.length === 0) return;
    const next = remotePendingSubmissions.filter(
      (submission) =>
        !remotePendingDeliveryCoveredByHistory(
          history,
          remotePendingDeliveryFromSubmission(submission),
        ),
    );
    if (next.length === remotePendingSubmissions.length) return;
    updateRemotePendingSubmissions(next);
  }, [
    conversationId,
    history,
    remotePendingSubmissions,
    updateRemotePendingSubmissions,
  ]);

  useEffect(() => {
    if (!conversationId) return;
    let cancelled = false;
    const target = conversationTarget;
    const pending = remotePendingSubmissions.filter(
      (submission) => submission.turn_id,
    );
    void (async () => {
      for (const submission of pending) {
        if (cancelled) return;
        try {
          const around = await conversationApi.getHistory(
            target,
            conversationId,
            {
              aroundTurnId: submission.turn_id,
              turnLimit: 1,
            },
          );
          if (cancelled) return;
          const confirmed = around.messages?.some(
            (message) =>
              message.role === "user" &&
              ((submission.submission_id &&
                submissionIdFromMessage(message) ===
                  submission.submission_id) ||
                deviceDeliveryIdFromMessage(message) ===
                  submission.device_delivery_id),
          );
          if (confirmed) {
            updateRemotePendingSubmissions((current) =>
              current.filter(
                (item) =>
                  item.device_delivery_id !== submission.device_delivery_id,
              ),
            );
          }
        } catch {
          // Without authoritative identity evidence, retain the pending delivery.
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [
    conversationId,
    conversationTarget,
    runtimeOwnerKey,
    remotePendingSubmissions,
    updateRemotePendingSubmissions,
  ]);

  const refreshApprovalSnapshot = useCallback(
    async (chatId: string, current: () => boolean) => {
      try {
        const nextApprovalSnapshot = await loadApprovalSnapshot(
          conversationTarget,
          chatId,
          acpPermissionActive,
        );
        if (!current()) return;
        if (!isCurrentConversation()) return;
        setApprovalSnapshot(nextApprovalSnapshot);
      } catch (approvalErr) {
        setError(
          approvalErr instanceof Error
            ? approvalErr.message
            : String(approvalErr),
        );
      }
    },
    [isCurrentConversation, conversationTarget, acpPermissionActive],
  );

  const loadStableHistory = useCallback(
    async (chatId: string) => {
      const mutationVersion = convergence.beginHistoryRead();
      const targetAtCall = conversationTarget;
      const nextHistory = await conversationApi.getHistory(
        targetAtCall,
        chatId,
        { turnLimit: INITIAL_CHAT_TURN_PAGE_SIZE },
      );
      if (!isCurrentConversation()) return null;
      if (!validChatHistory(nextHistory)) return null;
      if (!convergence.acceptHistory(mutationVersion, nextHistory)) return null;
      return nextHistory;
    },
    [convergence, isCurrentConversation, conversationTarget],
  );

  const applyStableHistorySnapshot = useCallback((nextHistory: ChatHistory) => {
    const retainedRuns = retainedCompletedLiveRunsRef.current.filter(
      (run) => !activeRunCoveredByHistory(nextHistory, run.user, run.fold),
    );
    retainedCompletedLiveRunsRef.current = retainedRuns;
    setHistory((current) =>
      retainedRuns.reduce<ChatHistory | null>(
        (merged, run) =>
          withCompletedLiveRun(merged, run.user, run.fold, run.acpOriginRunner),
        mergeStableHistory(
          current,
          preserveLoadedHistory(current, nextHistory),
        ),
      ),
    );
  }, []);

  const applyHistoryMutation = useCallback(
    (event: Extract<ConversationStreamEvent, { kind: "history_mutated" }>) => {
      if (!conversationId) return;
      if (event.action === "variant_selected") {
        setPendingSelectedVariant(identity.viewKey, {
          turn_id: event.turnId,
          selected_variant_id: event.selectedVariantId,
        });
        setSelectedVariantOverrides((current) => ({
          ...current,
          [event.turnId]: event.selectedVariantId,
        }));
        return;
      }

      clearPendingSelectedVariant(identity.viewKey);
      setSelectedVariantOverrides(() => {
        if (!event.selectedTurnId || !event.selectedVariantId) {
          return {};
        }
        return { [event.selectedTurnId]: event.selectedVariantId };
      });
      clearPendingActiveRegenerationFlush();
      clearPendingStreamFoldFlush();
      convergence.invalidateHistory();
      chatViewCache.delete(chatViewCacheKey(runtimeOwnerKey, conversationId));
      retainedCompletedLiveRunsRef.current = [];
      pendingSubmittedUserRef.current = null;
      setSendingSubmission(null);
      activeUserRef.current = null;
      streamFoldRef.current = null;
      activeRegenerationRef.current = null;
      setActiveUser(null);
      setStreamFold(null);
      setActiveRegeneration(null);
      setContextCompactionStatus(null);
      setStatus("idle");
      setLocatedHistory(null);
      locateVersion.current += 1;
      setHistory((current) => rolledBackHistory(current, event.beforeTurnId));
    },
    [
      convergence,
      clearPendingActiveRegenerationFlush,
      clearPendingStreamFoldFlush,
      conversationId,
      runtimeOwnerKey,
      identity.viewKey,
    ],
  );

  const retainCompletedLiveRun = useCallback(
    (
      user: Message,
      fold: ConversationStreamFold,
      acpOriginRunner: string | null,
    ) => {
      const key = completedLiveRunKey(user, fold);
      if (key) {
        const retainedRun = { key, user, fold, acpOriginRunner };
        retainedCompletedLiveRunsRef.current = [
          ...retainedCompletedLiveRunsRef.current.filter(
            (run) => run.key !== key,
          ),
          retainedRun,
        ].slice(-RETAINED_COMPLETED_LIVE_RUN_LIMIT);
      }
      setHistory((current) =>
        withCompletedLiveRun(current, user, fold, acpOriginRunner),
      );
    },
    [],
  );

  const refreshChatHistory = useCallback(
    async (chatId: string, current?: () => boolean) => {
      const nextHistory = await loadStableHistory(chatId);
      if (current && !current()) return null;
      if (!nextHistory) return null;
      applyStableHistorySnapshot(nextHistory);
      return nextHistory;
    },
    [applyStableHistorySnapshot, loadStableHistory],
  );

  useEffect(() => {
    const text = acpHistoryPreviewText.trim();
    if (!conversationId || acpHistoryPreviewId === null || !text) return;
    if (lastAcpHistoryPreviewIdRef.current === acpHistoryPreviewId) return;
    lastAcpHistoryPreviewIdRef.current = acpHistoryPreviewId;
    let cancelled = false;

    acpHistoryPreviewPreparingRef.current = true;
    setStatus((prev) =>
      prev === "idle" || prev === "loading" || prev === "preparing"
        ? "preparing"
        : prev,
    );
    setError(null);

    void (async () => {
      try {
        await conversationApi.persistAcpHistoryPreview(
          conversationTargetRef.current,
          conversationId,
          {
            preview_id: acpHistoryPreviewId,
            text,
            ...(activeAcpRunner ? { runner: activeAcpRunner } : {}),
          },
        );
        if (cancelled) return;
        await refreshChatHistory(conversationId);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (!cancelled) {
          acpHistoryPreviewPreparingRef.current = false;
          setStatus((prev) => (prev === "preparing" ? "idle" : prev));
        }
      }
    })();

    return () => {
      cancelled = true;
      acpHistoryPreviewPreparingRef.current = false;
    };
  }, [
    acpHistoryPreviewId,
    acpHistoryPreviewText,
    activeAcpRunner,
    conversationId,
    refreshChatHistory,
  ]);

  const clearCoveredCompletedStreamOverlay = useCallback(
    (nextHistory: ChatHistory | null) => {
      const activeUserSnapshot = activeUserRef.current;
      const streamFoldSnapshot = streamFoldRef.current;
      if (
        stableHistoryKeepsActiveRunOpen(
          nextHistory,
          activeUserSnapshot,
          streamFoldSnapshot,
        )
      ) {
        return;
      }
      const stableHistoryCoversStreamOverlay = Boolean(
        streamFoldSnapshot &&
          activeRunCoveredByHistory(
            nextHistory,
            activeUserSnapshot,
            streamFoldSnapshot,
          ),
      );
      if (!stableHistoryCoversStreamOverlay) {
        return;
      }
      clearPendingStreamFoldFlush();
      activeUserRef.current = null;
      streamFoldRef.current = null;
      abortRef.current = null;
      setActiveUser(null);
      setStreamFold(null);
      setContextCompactionStatus(null);
      setStatus((current) =>
        current === "streaming" || current === "stopping" ? "idle" : current,
      );
    },
    [clearPendingStreamFoldFlush],
  );

  const clearActiveRegenerationOverlay = useCallback(
    (active: ActiveRegeneration) => {
      clearPendingActiveRegenerationFlush();
      activeRegenerationRef.current = null;
      setActiveRegeneration(null);
      setSelectedVariantOverrides((current) => {
        if (current[active.turnId] !== active.variantId) return current;
        const next = { ...current };
        delete next[active.turnId];
        return next;
      });
    },
    [clearPendingActiveRegenerationFlush],
  );

  const startReconnectStream = useCallback(
    (options?: {
      passive?: boolean;
      regeneration?: ReconnectRegenerationTarget;
    }) => {
      if (
        !conversationId ||
        !viewerVisible ||
        abortRef.current ||
        reconnectInFlightRef.current
      ) {
        return false;
      }
      const passive = options?.passive === true;
      const regeneration = options?.regeneration ?? null;
      const chatAtCall = conversationId;
      const targetAtCall = conversationTarget;
      const controller = new AbortController();
      const reconnectCursor = activityCursorRef.current;
      const reconnectFold = streamFoldRef.current;
      const resumesExistingProjection = Boolean(
        reconnectCursor?.generation && reconnectFold !== null,
      );
      reconnectInFlightRef.current = controller;
      let activated = !passive;
      if (!passive) {
        abortRef.current = controller;
        clearPendingStreamFoldFlush();
        if (regeneration) {
          clearPendingActiveRegenerationFlush();
          setActiveUser(null);
          setStreamFold(null);
          const nextActiveRegeneration = {
            ...regeneration,
            fold: initialConversationStreamFold,
          };
          activeRegenerationRef.current = nextActiveRegeneration;
          setActiveRegeneration(nextActiveRegeneration);
        } else if (!resumesExistingProjection) {
          setStreamFold(initialConversationStreamFold);
        }
        setContextCompactionStatus(null);
        setStatus("streaming");
      }

      const activate = () => {
        if (activated) return true;
        if (
          conversationIdRef.current !== chatAtCall ||
          !isCurrentConversation() ||
          abortRef.current
        ) {
          return false;
        }
        activated = true;
        abortRef.current = controller;
        clearPendingStreamFoldFlush();
        if (regeneration) {
          clearPendingActiveRegenerationFlush();
          setActiveUser(null);
          setStreamFold(null);
          const nextActiveRegeneration = {
            ...regeneration,
            fold: initialConversationStreamFold,
          };
          activeRegenerationRef.current = nextActiveRegeneration;
          setActiveRegeneration(nextActiveRegeneration);
        } else if (!resumesExistingProjection) {
          setStreamFold(initialConversationStreamFold);
        }
        setContextCompactionStatus(null);
        setStatus("streaming");
        return true;
      };

      void (async () => {
        let fold =
          resumesExistingProjection && reconnectFold
            ? reconnectFold
            : initialConversationStreamFold;
        let sawApprovalRequest = false;
        const activity = convergence.openActivity(
          targetAtCall,
          chatAtCall,
          reconnectCursor,
        );
        let reconciliationWorker: HistoryReconciliationWorker | null = null;
        try {
          const resumePosition = activityCursorRef.current;
          const response = await conversationApi.openLive(
            targetAtCall,
            chatAtCall,
            {
              chat_id: conversationId,
              user_id: DEFAULT_USER_ID,
              viewer_id: browserViewerId,
              ...(resumePosition?.generation
                ? {
                    activity_generation: resumePosition.generation,
                    next_activity_sequence: resumePosition.sequence + 1,
                  }
                : {}),
              channel: DEFAULT_CHANNEL,
              stream: true,
            },
            controller.signal,
          );
          if (!response.ok) {
            const body = await response.text().catch(() => "");
            throw new Error(
              `Chat reconnect failed: ${response.status}${
                body ? ` ${body}` : ""
              }`,
            );
          }

          const streamIsCurrent = () =>
            !controller.signal.aborted &&
            conversationIdRef.current === chatAtCall &&
            isCurrentConversation();
          const worker = new HistoryReconciliationWorker({
            refreshHistory: async () => {
              await refreshChatHistory(conversationId, streamIsCurrent);
            },
            refreshApproval: () =>
              refreshApprovalSnapshot(conversationId, streamIsCurrent),
            onError: (_kind, error) => {
              if (streamIsCurrent()) {
                setError(
                  error instanceof Error ? error.message : String(error),
                );
              }
            },
          });
          reconciliationWorker = worker;

          const requestReconciliation = (decision: ActivityUpdate) => {
            if (decision.refreshHistory) {
              if (decision.hydrationDevices.size) {
                void Promise.allSettled(
                  [...decision.hydrationDevices].map((deviceId) =>
                    triggerTargetedSyncCycle(deviceId),
                  ),
                ).then(() => worker.requestHistory());
              } else worker.requestHistory();
            }
            if (decision.refreshApproval) {
              sawApprovalRequest = true;
              worker.requestApproval();
            }
          };

          for await (const chunk of readSseChunks(response.body, {
            idleTimeoutMs: CONVERSATION_LIVE_READ_IDLE_TIMEOUT_MS,
          })) {
            if (!streamIsCurrent()) return;
            const update = activity.consume(
              parseConversationStreamChunk(chunk.data),
              {
                fold: activeRegenerationRef.current?.fold ?? fold,
                user: activeUserRef.current ?? pendingSubmittedUserRef.current,
                hasOverlay:
                  remotePendingSubmissionsRef.current.length > 0 ||
                  activeUserRef.current !== null ||
                  streamFoldRef.current !== null ||
                  activeRegenerationRef.current !== null,
                currentDeviceId: currentDeviceIdRef.current,
              },
            );
            if (!update) continue;
            activityCursorRef.current = activity.cursor;
            for (const transition of update.transitions) {
              if (transition.kind === "conversation_control") {
                applyControlProjection(transition.control);
              } else {
                applyHistoryMutation(transition);
                if (transition.action === "rolled_back")
                  fold = initialConversationStreamFold;
              }
            }
            if (update.completedDeliveries.length) {
              const completed = new Set(update.completedDeliveries);
              updateRemotePendingSubmissions((pending) =>
                pending.filter(
                  (submission) => !completed.has(submission.device_delivery_id),
                ),
              );
              updateQueuedSubmissions((queued) =>
                queued.filter(
                  (submission) =>
                    !submission.deviceDeliveryId ||
                    !completed.has(submission.deviceDeliveryId),
                ),
              );
            }
            if (update.replay) {
              const {
                fold: snapshotFold,
                user: replayUser,
                compaction,
              } = update.replay;
              const replayActivates =
                Boolean(replayUser) ||
                snapshotFold.output.length > 0 ||
                snapshotFold.status !== "idle";
              if (replayActivates && !activate()) return;
              if (activated && compaction !== undefined)
                setContextCompactionStatus(compaction);
              fold = snapshotFold;
              const activeRegenerationTarget = activeRegenerationRef.current;
              if (activeRegenerationTarget) {
                const nextActiveRegeneration = {
                  ...activeRegenerationTarget,
                  fold,
                };
                activeRegenerationRef.current = nextActiveRegeneration;
                publishActiveRegeneration(nextActiveRegeneration, true);
              } else if (activated) {
                if (replayUser) {
                  pendingSubmittedUserRef.current = null;
                  activeUserRef.current = replayUser;
                  setActiveUser(replayUser);
                  updateQueuedSubmissions((current) =>
                    removeQueuedSubmissionForUserMessage(current, replayUser),
                  );
                }
                publishStreamFold(fold, true);
              }
              if (activated && fold.sawTerminal) {
                setContextCompactionStatus(null);
                setStatus("idle");
              }
              requestReconciliation(update);
              continue;
            }
            requestReconciliation(update);
            const event = update.event;
            if (!event) continue;
            const activeRegenerationTarget = activeRegenerationRef.current;
            if (activeRegenerationTarget) {
              if (event.kind === "user_message") continue;
              const activatesRegeneration =
                event.kind !== "response_started" &&
                event.kind !== "context_compaction" &&
                event.kind !== "device_delivery_accepted" &&
                !shouldRefreshApprovalSnapshot(event) &&
                !(event.kind === "completed" && event.output.length === 0);
              if (activatesRegeneration && !activate()) return;
              const nextCompactionStatus = update.compaction;
              if (activated && nextCompactionStatus !== undefined) {
                setContextCompactionStatus(nextCompactionStatus);
              }
              fold = update.fold!;
              if (activated) {
                const nextActiveRegeneration = {
                  ...activeRegenerationTarget,
                  fold,
                };
                activeRegenerationRef.current = nextActiveRegeneration;
                publishActiveRegeneration(nextActiveRegeneration);
              }
              if (activated && streamEventTerminatesRun(event)) {
                setContextCompactionStatus(null);
                setStatus("idle");
              }
              continue;
            }
            if (event.kind === "user_message") {
              const latestFold = takePendingStreamFold() ?? fold;
              if (!activate()) return;
              if (abortRef.current !== controller) {
                abortRef.current = controller;
                setContextCompactionStatus(null);
                setStatus("streaming");
              }
              if (latestFold.output.length > 0 || latestFold.sawTerminal) {
                const completedUser = activeUserRef.current;
                if (latestFold.sawTerminal && completedUser) {
                  retainCompletedLiveRun(
                    completedUser,
                    latestFold,
                    acpPassthroughActive ? activeAcpRunner : null,
                  );
                }
                publishStreamFold(latestFold, true);
                worker.requestHistory();
                if (!isCurrentConversation()) {
                  return;
                }
              }
              clearPendingStreamFoldFlush();
              pendingSubmittedUserRef.current = null;
              activeUserRef.current = event.message;
              setActiveUser(event.message);
              setError(null);
              fold = initialConversationStreamFold;
              publishStreamFold(fold, true);
              updateQueuedSubmissions((current) =>
                removeQueuedSubmissionForUserMessage(current, event.message),
              );
              continue;
            }
            const pendingSubmittedUser = pendingSubmittedUserRef.current;
            if (
              !activeUserRef.current &&
              pendingSubmittedUser &&
              !(
                event.kind === "completed" &&
                event.output.length === 0 &&
                fold.output.length === 0
              )
            ) {
              pendingSubmittedUserRef.current = null;
              activeUserRef.current = pendingSubmittedUser;
              setActiveUser(pendingSubmittedUser);
            }
            if (
              !activeUserRef.current &&
              fold.output.length === 0 &&
              shouldRefreshApprovalSnapshot(event)
            ) {
              continue;
            }
            const activatesStream =
              event.kind !== "response_started" &&
              event.kind !== "context_compaction" &&
              event.kind !== "device_delivery_accepted" &&
              !shouldRefreshApprovalSnapshot(event) &&
              !(
                event.kind === "completed" &&
                event.output.length === 0 &&
                !activeUserRef.current &&
                !streamFoldRef.current
              );
            if (activatesStream && !activate()) return;
            const nextCompactionStatus = update.compaction;
            if (activated && nextCompactionStatus !== undefined) {
              setContextCompactionStatus(nextCompactionStatus);
            }
            const previousFold = fold;
            fold = update.fold!;
            const foldChanged = fold !== previousFold;
            if (activated) {
              if (foldChanged) {
                publishStreamFold(fold, streamEventTerminatesRun(event));
              }
              if (streamEventTerminatesRun(event)) {
                setContextCompactionStatus(null);
                setStatus("idle");
              }
            } else if (streamEventTerminatesRun(event)) {
              pendingSubmittedUserRef.current = null;
              setContextCompactionStatus(null);
              setStatus((prev) => (prev === "streaming" ? "idle" : prev));
            }
            if (event.kind === "turn_terminal" && event.status === "failed") {
              const detail = event.raw.detail;
              const message = event.raw.message;
              if (typeof detail === "string" && detail.trim()) setError(detail);
              else if (typeof message === "string" && message.trim())
                setError(message);
            }
          }
          if (!activated && passive && sawApprovalRequest) {
            worker.requestHistory();
            return;
          }
          fold = finishConversationStream(fold);
          if (!streamIsCurrent()) return;
          if (activated) {
            const activeRegenerationTarget = activeRegenerationRef.current;
            if (activeRegenerationTarget) {
              const nextActiveRegeneration = {
                ...activeRegenerationTarget,
                fold,
              };
              activeRegenerationRef.current = nextActiveRegeneration;
              publishActiveRegeneration(nextActiveRegeneration, true);
            } else if (
              fold.output.length === 0 &&
              !fold.sawTerminal &&
              !activeUserRef.current
            ) {
              clearPendingStreamFoldFlush();
              activeUserRef.current = null;
              streamFoldRef.current = null;
              setActiveUser(null);
              setStreamFold(null);
              if (sawApprovalRequest) {
                worker.requestHistory();
              }
            } else {
              publishStreamFold(fold, true);
            }
          }
        } catch (err) {
          if (
            conversationIdRef.current !== chatAtCall ||
            !isCurrentConversation()
          ) {
            return;
          }
          // Fetch/body readers may reject with the raw abort reason instead of AbortError.
          if (
            controller.signal.aborted ||
            (err as DOMException)?.name === "AbortError"
          ) {
            if (activated) {
              const activeRegenerationTarget = activeRegenerationRef.current;
              if (activeRegenerationTarget) {
                const latest = takePendingActiveRegeneration()?.fold ?? fold;
                const nextActiveRegeneration = {
                  ...activeRegenerationTarget,
                  fold: finishConversationStream(latest),
                };
                activeRegenerationRef.current = nextActiveRegeneration;
                publishActiveRegeneration(nextActiveRegeneration, true);
              } else {
                const latest = takePendingStreamFold() ?? fold;
                publishStreamFold(finishConversationStream(latest), true);
              }
            }
          } else if (!(err instanceof SseReadIdleTimeoutError)) {
            if (!passive || activated) {
              setError(err instanceof Error ? err.message : String(err));
            }
            if (activated) {
              const failedEvent: ConversationStreamEvent = {
                kind: "parse_error",
                error: err instanceof Error ? err : new Error(String(err)),
                raw: "",
              };
              const activeRegenerationTarget = activeRegenerationRef.current;
              if (activeRegenerationTarget) {
                const latest = takePendingActiveRegeneration()?.fold ?? fold;
                const nextActiveRegeneration = {
                  ...activeRegenerationTarget,
                  fold: foldConversationStreamEvent(latest, failedEvent),
                };
                activeRegenerationRef.current = nextActiveRegeneration;
                publishActiveRegeneration(nextActiveRegeneration, true);
              } else {
                const latest = takePendingStreamFold() ?? fold;
                publishStreamFold(
                  foldConversationStreamEvent(latest, failedEvent),
                  true,
                );
              }
            }
          }
        } finally {
          reconciliationWorker?.cancel();
          if (abortRef.current === controller) {
            abortRef.current = null;
          }
          if (reconnectInFlightRef.current === controller) {
            reconnectInFlightRef.current = null;
          }
          if (
            !controller.signal.aborted &&
            conversationIdRef.current === chatAtCall &&
            isCurrentConversation()
          ) {
            if (activated) {
              setContextCompactionStatus(null);
              setStatus("idle");
            }
            if (activated && fold.status === "completed") {
              setReloadKey((key) => key + 1);
            }
            if (liveActivityRetryTimerRef.current === null) {
              liveActivityRetryTimerRef.current = window.setTimeout(() => {
                liveActivityRetryTimerRef.current = null;
                if (
                  conversationIdRef.current === chatAtCall &&
                  isCurrentConversation()
                ) {
                  setLiveActivityWakeSeq((seq) => seq + 1);
                }
              }, CONVERSATION_LIVE_RECONNECT_DELAY_MS);
            }
          }
        }
      })();

      return true;
    },
    [
      convergence,
      clearPendingStreamFoldFlush,
      clearPendingActiveRegenerationFlush,
      conversationId,
      browserViewerId,
      activeAcpRunner,
      acpPassthroughActive,
      authorityPlacementDeviceId,
      publishActiveRegeneration,
      publishStreamFold,
      applyControlProjection,
      applyHistoryMutation,
      refreshApprovalSnapshot,
      refreshChatHistory,
      isCurrentConversation,
      retainCompletedLiveRun,
      takePendingActiveRegeneration,
      takePendingStreamFold,
      updateQueuedSubmissions,
      updateRemotePendingSubmissions,
      viewerVisible,
      conversationTarget,
    ],
  );

  useEffect(() => {
    if (viewerVisible) return;
    reconnectInFlightRef.current?.abort("viewer hidden");
    abortRef.current?.abort("viewer hidden");
  }, [viewerVisible]);

  const loadStableHistoryForViewer = useCallback(async () => {
    if (!conversationId) return null;
    return loadStableHistory(conversationId);
  }, [conversationId, loadStableHistory]);

  const applyStableHistoryForViewer = useCallback(
    (nextHistory: ChatHistory) => {
      applyStableHistorySnapshot(nextHistory);
    },
    [applyStableHistorySnapshot],
  );

  const handleRemotePendingStableHistory = useCallback(
    (nextHistory: ChatHistory) => {
      updateRemotePendingSubmissions((pending) =>
        pending.filter(
          (submission) =>
            !remotePendingDeliveryCoveredByHistory(
              nextHistory,
              remotePendingDeliveryFromSubmission(submission),
            ),
        ),
      );
      const canonicalUsers = (nextHistory.messages || []).filter(
        (message) => message.role === "user",
      );
      updateQueuedSubmissions((queued) =>
        queued.filter(
          (submission) =>
            !canonicalUsers.some((message) =>
              queuedSubmissionIdentityMatchesUserMessage(submission, message),
            ),
        ),
      );
    },
    [updateQueuedSubmissions, updateRemotePendingSubmissions],
  );

  useEffect(() => {
    if (!conversationId) return;
    if (lastActivityWakeSeqRef.current === activityWakeSeq) return;
    lastActivityWakeSeqRef.current = activityWakeSeq;

    if (!abortRef.current && !reconnectInFlightRef.current) {
      startReconnectStream({ passive: true });
    }
  }, [activityWakeSeq, conversationId, startReconnectStream]);

  useEffect(() => {
    if (!viewerVisible || !activityRecoverySeq) return;
    const previous = lastActivityRecoveryRef.current;
    if (
      previous?.viewKey === identity.viewKey &&
      previous.sequence === activityRecoverySeq
    )
      return;
    lastActivityRecoveryRef.current = {
      viewKey: identity.viewKey,
      sequence: activityRecoverySeq,
    };
    // Reuse scoped history/control reconciliation even if Live is still attached.
    // Reconnecting the notification stream alone cannot recover missed events.
    setReloadKey((key) => key + 1);
  }, [activityRecoverySeq, identity.viewKey, viewerVisible]);

  const isWatchingLiveActivity = useCallback(() => {
    return Boolean(abortRef.current || reconnectInFlightRef.current);
  }, []);

  useConversationLiveActivityViewerLifecycle({
    viewKey: deferInitialAttach ? undefined : identity.viewKey,
    history,
    currentDeviceId,
    remotePendingSubmissions,
    isWatchingLiveActivity,
    startLiveActivityViewer: startReconnectStream,
    loadStableHistory: loadStableHistoryForViewer,
    applyStableHistory: applyStableHistoryForViewer,
    onRemotePendingStableHistory: handleRemotePendingStableHistory,
    viewerVisible,
    wakeSignal: liveActivityWakeSeq,
  });

  useEffect(() => {
    if (!conversationId) {
      setHistory({ status: "idle", messages: [] });
      setApprovalSnapshot({ pending: [], resolved: [] });
      setStatus("idle");
      setError(null);
      setSelectedVariantOverrides({});
      updateQueuedSubmissions([]);
      return;
    }
    if (deferInitialAttach) {
      return;
    }

    let cancelled = false;

    const mutationVersion = convergence.beginHistoryRead();
    const cached = readChatViewCache(runtimeOwnerKey, conversationId);
    if (cached?.hasDurableHistory) {
      setHistory(cached.history);
      setApprovalSnapshot(cached.approvalSnapshot);
    }
    setStatus((prev) =>
      prev === "streaming" || prev === "preparing" || cached ? prev : "loading",
    );
    setError(null);

    const historyRequest = conversationApi.getHistory(
      conversationTarget,
      conversationId,
      { turnLimit: INITIAL_CHAT_TURN_PAGE_SIZE },
    );

    historyRequest
      .then((nextHistory) => {
        if (cancelled) return;
        if (!isCurrentConversation()) return;
        if (!validChatHistory(nextHistory)) return;
        if (!convergence.acceptHistory(mutationVersion, nextHistory)) return;
        applyStableHistorySnapshot(nextHistory);
        const pending = getPendingSelectedVariant(identity.viewKey);
        setSelectedVariantOverrides(
          pending ? { [pending.turn_id]: pending.selected_variant_id } : {},
        );
        if (!abortRef.current && !submitAbortRef.current) {
          const activeUserSnapshot = activeUserRef.current;
          const streamFoldSnapshot = streamFoldRef.current;
          const hasNonTerminalActiveRun = Boolean(
            activeUserSnapshot &&
              (!streamFoldSnapshot || !streamFoldSnapshot.sawTerminal),
          );
          const retainCompletedStream =
            shouldRetainCompletedActiveRunAfterRefresh(
              nextHistory,
              activeUserSnapshot,
              streamFoldSnapshot,
            );
          clearPendingActiveRegenerationFlush();
          activeRegenerationRef.current = null;
          setActiveRegeneration(null);
          if (!hasNonTerminalActiveRun && !retainCompletedStream) {
            clearPendingStreamFoldFlush();
            activeUserRef.current = null;
            streamFoldRef.current = null;
            setActiveUser(null);
            setStreamFold(null);
          }
        }
      })
      .catch((err) => {
        if (cancelled) return;
        if (!cached) {
          setHistory({ status: "idle", messages: [] });
          setApprovalSnapshot({ pending: [], resolved: [] });
        }
        setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) {
          setStatus((prev) => (prev === "loading" ? "idle" : prev));
        }
      });

    return () => {
      cancelled = true;
    };
  }, [
    convergence,
    conversationId,
    runtimeOwnerKey,
    projectId,
    conversationTarget,
    reloadKey,
    applyStableHistorySnapshot,
    clearPendingActiveRegenerationFlush,
    clearPendingStreamFoldFlush,
    isCurrentConversation,
    updateQueuedSubmissions,
    deferInitialAttach,
    identity.viewKey,
  ]);

  useEffect(() => {
    if (!conversationId || deferInitialAttach) return;
    let cancelled = false;
    void loadApprovalSnapshot(
      conversationTarget,
      conversationId,
      acpPermissionActive,
    )
      .then((nextApprovalSnapshot) => {
        if (cancelled || !isCurrentConversation()) return;
        setApprovalSnapshot(nextApprovalSnapshot);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [
    conversationId,
    conversationTarget,
    acpPermissionActive,
    reloadKey,
    deferInitialAttach,
    isCurrentConversation,
  ]);

  useEffect(() => {
    clearCoveredCompletedStreamOverlay(history);
  }, [clearCoveredCompletedStreamOverlay, history]);

  useEffect(() => {
    if (
      !activeRegeneration ||
      !activeRegenerationCoveredByHistory(history, activeRegeneration)
    ) {
      return;
    }
    clearActiveRegenerationOverlay(activeRegeneration);
  }, [activeRegeneration, clearActiveRegenerationOverlay, history]);

  const locateTurn = useCallback(
    async (turnId: string) => {
      if (!conversationId) return;
      const version = ++locateVersion.current;
      setError(null);
      const target = conversationTarget;
      try {
        const page = await conversationApi.getHistory(target, conversationId, {
          aroundTurnId: turnId,
          turnLimit: INITIAL_CHAT_TURN_PAGE_SIZE,
        });
        if (version !== locateVersion.current) return;
        setLocatedHistory(page);
      } catch (err) {
        if (version === locateVersion.current)
          setError(err instanceof Error ? err.message : String(err));
      }
    },
    [conversationId, conversationTarget],
  );

  const loadHistoryGap = useCallback(async () => {
    const gap = historyWindowGap(history, locatedHistory);
    if (!conversationId || !gap || locatedReadBusy.current) return;
    locatedReadBusy.current = true;
    const version = locateVersion.current;
    try {
      const target = conversationTarget;
      const older = await conversationApi.getHistory(target, conversationId, {
        beforeTurnOrdinal: gap.beforeTurnOrdinal,
        turnLimit: INITIAL_CHAT_TURN_PAGE_SIZE,
      });
      if (version !== locateVersion.current) return;
      if (gap.beforeMessageId === history?.messages[0]?.id) {
        setHistory((current) =>
          current ? joinHistoryWindow(current, older) : older,
        );
      } else {
        setLocatedHistory((current) =>
          current ? joinHistoryWindow(current, older) : older,
        );
      }
    } catch (err) {
      if (version === locateVersion.current)
        setError(err instanceof Error ? err.message : String(err));
    } finally {
      locatedReadBusy.current = false;
    }
  }, [conversationId, history, locatedHistory, conversationTarget]);

  useEffect(() => {
    if (
      !locatedHistory ||
      !history ||
      historyWindowGap(history, locatedHistory)
    )
      return;
    setHistory((current) =>
      current ? joinHistoryWindow(current, locatedHistory) : locatedHistory,
    );
    setLocatedHistory(null);
  }, [history, locatedHistory]);

  const loadOlder = useCallback(async () => {
    if (!conversationId || loadingOlderRef.current) return;
    const loadingLocated = Boolean(
      locatedHistory &&
        (locatedHistory.oldest_turn_ordinal ?? 0) <
          (history?.oldest_turn_ordinal ?? Infinity),
    );
    const oldest = loadingLocated ? locatedHistory : history;
    const beforeTurnOrdinal = oldest?.oldest_turn_ordinal;
    if (!oldest?.has_more_before || !beforeTurnOrdinal) return;

    loadingOlderRef.current = true;
    setLoadingOlder(true);

    const version = locateVersion.current;
    try {
      const options = {
        turnLimit: INITIAL_CHAT_TURN_PAGE_SIZE,
        beforeTurnOrdinal,
      };
      const targetAtCall = conversationTarget;
      const older = await conversationApi.getHistory(
        targetAtCall,
        conversationId,
        options,
      );
      if (!isCurrentConversation() || version !== locateVersion.current) return;
      if (loadingLocated)
        setLocatedHistory((current) =>
          current ? joinHistoryWindow(current, older) : older,
        );
      else setHistory((current) => mergeOlderHistory(current, older));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      loadingOlderRef.current = false;
      setLoadingOlder(false);
    }
  }, [
    conversationId,
    history,
    locatedHistory,
    isCurrentConversation,
    conversationTarget,
  ]);

  useEffect(() => {
    if (!conversationResetInitializedRef.current) {
      conversationResetInitializedRef.current = true;
      return;
    }
    const cached = readChatViewCache(runtimeOwnerKey, conversationId);

    abortRef.current?.abort("conversation changed");
    submitAbortRef.current?.abort("conversation changed");
    if (
      reconnectInFlightRef.current &&
      reconnectInFlightRef.current !== abortRef.current
    ) {
      reconnectInFlightRef.current.abort("conversation changed");
    }
    abortRef.current = null;
    submitAbortRef.current = null;
    reconnectInFlightRef.current = null;
    clearPendingStreamFoldFlush();
    clearPendingActiveRegenerationFlush();
    pendingSubmittedUserRef.current = null;
    setActiveUser(cached?.activeUser ?? null);
    activeUserRef.current = cached?.activeUser ?? null;
    setStreamFold(cached?.streamFold ?? null);
    streamFoldRef.current = cached?.streamFold ?? null;
    activeRegenerationRef.current = null;
    setActiveRegeneration(null);
    setApprovalSnapshot({ pending: [], resolved: [] });
    setContextCompactionStatus(null);
    setStatus("idle");
    setLoadingOlder(false);
    loadingOlderRef.current = false;
    acpHistoryPreviewPreparingRef.current = false;
    lastAcpHistoryPreviewIdRef.current = null;
    activityCursorRef.current = cached?.activityCursor ?? null;
    setSelectedVariantOverrides({});
    updateQueuedSubmissions([]);
  }, [
    conversationId,
    runtimeOwnerKey,
    clearPendingActiveRegenerationFlush,
    clearPendingStreamFoldFlush,
    updateQueuedSubmissions,
  ]);

  const reload = useCallback(() => {
    setReloadKey((key) => key + 1);
  }, []);

  const stop = useCallback(async () => {
    if (!conversationId) return;
    const activeTurn = history?.active_turn;
    if (!activeTurn) {
      setReloadKey((key) => key + 1);
      return;
    }
    const authorityEpoch = activeTurn.authority_epoch;
    if (!authorityEpoch) return;
    const targetAtCall = conversationTargetRef.current;
    const current = () =>
      conversationIdRef.current === conversationId &&
      identityRef.current.viewKey === identity.viewKey;
    setStatus("stopping");
    try {
      const result = await conversationApi.stopChat(
        targetAtCall,
        conversationId,
        {
          turn_id: activeTurn.turn_id,
          authority_epoch: authorityEpoch,
        },
      );
      if (!current()) return;
      if (result && !result.stopped) {
        if (result.detail) setError(result.detail);
        setStatus((value) => (value === "stopping" ? status : value));
      }
    } catch (err) {
      if (!current()) return;
      setError(err instanceof Error ? err.message : String(err));
      setStatus((value) => (value === "stopping" ? status : value));
    }
  }, [
    conversationId,
    status,
    history?.active_turn,
    history?.control,
    identity.viewKey,
  ]);

  const submit = useCallback(
    async (
      text: string,
      attachments: ComposerAttachment[] = [],
      messageId?: string,
      existingSubmissionId?: string,
    ) => {
      const trimmed = text.trim();
      if ((!trimmed && attachments.length === 0) || !conversationId)
        return false;

      const agentAtCall = selectedAgent;
      const chatAtCall = conversationId;
      const targetAtCall = conversationTarget;
      const generatedActionId = crypto.randomUUID();
      const submissionId =
        existingSubmissionId || `submission-${generatedActionId}`;
      const isCurrentView = () =>
        isCurrentConversation() && conversationIdRef.current === chatAtCall;

      if (acpHistoryPreviewPreparingRef.current) {
        updateQueuedSubmissions((current) => [
          ...current,
          {
            id: submissionId,
            submissionId,
            text: trimmed,
            attachments,
            messageId: `user-${submissionId}`,
            source: "draft",
          },
        ]);
        return true;
      }

      const targetAtSubmit = authorityPlacementDeviceId || null;
      const attachmentEpochs = Array.from(
        new Set(
          attachments
            .map((attachment) => attachment.authorityEpoch)
            .filter((epoch): epoch is string => Boolean(epoch)),
        ),
      );
      if (attachmentEpochs.length > 1) {
        setError("Pending attachments belong to different Authority epochs");
        return false;
      }
      const message = userMessage(
        trimmed,
        attachments,
        undefined,
        messageId || `user-${generatedActionId}`,
        submissionId,
      );
      const selectedVariant = getPendingSelectedVariant(identity.viewKey);
      if (!pendingSubmittedUserRef.current) {
        pendingSubmittedUserRef.current = message;
      }
      setContextCompactionStatus(null);
      setError(null);
      const localDeviceAtSubmit = currentDeviceIdRef.current;
      const deliveringToRemoteDevice =
        targetAtSubmit &&
        localDeviceAtSubmit &&
        targetAtSubmit !== localDeviceAtSubmit;
      const optimisticUser = deliveringToRemoteDevice
        ? { ...message, remote_delivery_state: "sending" as const }
        : message;
      setSendingSubmission({
        owner: runtimeOwnerKey,
        chatId: chatAtCall,
        message: optimisticUser,
      });
      const clearSendingSubmission = () => {
        setSendingSubmission((current) =>
          current?.message === optimisticUser ? null : current,
        );
      };
      const clearSubmittedVariant = () => {
        if (!selectedVariant) return;
        clearPendingSelectedVariant(identity.viewKey);
        setSelectedVariantOverrides({});
      };
      let submittedToTransport = false;
      const submittedDraftKey = `${runtimeOwnerKey}:${conversationId}`;
      try {
        if (!isCurrentView()) return false;
        setStatus("streaming");
        const submitRequest = buildSubmitRequest(
          conversationId,
          message,
          submissionId,
          selectedVariant,
          submitDraftModel ? committedModelRef.current : undefined,
          attachmentEpochs[0],
        );
        if (unmaterialized) {
          submitRequest.initial_acp =
            loadDraftConversationAcp(
              projectId ? `project:${projectId}` : agentAtCall,
              conversationId,
            ) ?? undefined;
        }
        saveSubmitted(submittedDraftKey, {
          submissionId,
          text: trimmed,
          attachments,
          request: submitRequest,
          targetDeviceId: targetAtSubmit,
        });
        submittedToTransport = true;
        const response = await conversationApi.submit(
          targetAtCall,
          chatAtCall,
          submitRequest,
          projectId ? agentAtCall : undefined,
        );

        if (!response.ok) {
          if (response.status < 500)
            clearSubmitted(submittedDraftKey, submissionId);
          const body = await response.text().catch(() => "");
          throw new Error(
            `Chat submit failed: ${response.status}${body ? ` ${body}` : ""}`,
          );
        }

        const result = (await response
          .json()
          .catch(() => null)) as SubmitChatResult | null;
        const confirmedAcceptance = result?.accepted === true;
        const confirmedRejection =
          result?.accepted === false &&
          result.mode !== "unconfirmed" &&
          result.failure_kind !== "device_offline" &&
          result.failure_kind !== "delivery_unknown" &&
          result.failure_kind !== "protocol_error";
        if (confirmedAcceptance || confirmedRejection)
          clearSubmitted(submittedDraftKey, submissionId);
        if (!isCurrentView()) {
          return confirmedAcceptance
            ? true
            : confirmedRejection
            ? false
            : ("unknown" as const);
        }
        if (!confirmedAcceptance) {
          if (!confirmedRejection) {
            throw new Error(
              result?.detail || "Submission result could not be confirmed",
            );
          }
          clearSubmitted(submittedDraftKey, submissionId);
          clearSendingSubmission();
          if (targetAtSubmit) {
            const reason = remoteDeliveryErrorText(result);
            if (
              pendingSubmittedUserRef.current &&
              messageStringId(pendingSubmittedUserRef.current) ===
                messageStringId(message)
            ) {
              pendingSubmittedUserRef.current = null;
            }
            updateQueuedSubmissions((current) => [
              ...current.filter((item) => item.id !== submissionId),
              {
                id: submissionId,
                submissionId,
                text: trimmed,
                attachments,
                messageId: messageStringId(message) || undefined,
                source: "transport",
                deliveryState: "failed",
                editable: true,
                deletable: true,
              },
            ]);
            setError(reason);
            return false;
          }
          throw new Error(result?.detail || "Chat submit was not accepted");
        }

        clearSubmitted(submittedDraftKey, submissionId);
        if (targetAtCall.scope === "agent") {
          dispatchChatSubmitted(agentAtCall, conversationId, trimmed);
        }
        clearSubmittedVariant();
        setContextCompactionStatus(null);
        clearSendingSubmission();
        updateQueuedSubmissions((current) =>
          current.filter((item) => item.id !== submissionId),
        );
        if (result.control) {
          applyControlProjection(result.control);
        }

        const deviceDeliveryAccepted =
          result.device_delivery_id &&
          result.target_device_id &&
          typeof result.device_delivery_sequence === "number"
            ? {
                device_delivery_id: result.device_delivery_id,
                target_device_id: result.target_device_id,
                device_delivery_sequence: result.device_delivery_sequence,
                accepted_at: result.accepted_at,
              }
            : null;
        if (deviceDeliveryAccepted) {
          const submission: RemotePendingSubmission = {
            ...deviceDeliveryAccepted,
            ...(result.submission_id
              ? { submission_id: result.submission_id }
              : {}),
            ...(result.input_id ? { input_id: result.input_id } : {}),
            ...(result.control?.active_turn?.turn_id
              ? { turn_id: result.control.active_turn.turn_id }
              : {}),
            message,
          };
          if (
            pendingSubmittedUserRef.current &&
            messageStringId(pendingSubmittedUserRef.current) ===
              messageStringId(message)
          ) {
            pendingSubmittedUserRef.current = null;
          }
          updateRemotePendingSubmissions((current) => [
            ...current.filter(
              (item) =>
                item.device_delivery_id !== submission.device_delivery_id &&
                !(
                  item.target_device_id === submission.target_device_id &&
                  item.device_delivery_sequence <
                    submission.device_delivery_sequence
                ),
            ),
            submission,
          ]);
          setStatus("idle");
          setLiveActivityWakeSeq((seq) => seq + 1);
        }

        if (result.mode === "queued") {
          if (
            pendingSubmittedUserRef.current &&
            messageStringId(pendingSubmittedUserRef.current) ===
              messageStringId(message)
          ) {
            pendingSubmittedUserRef.current = null;
          }
          setStatus("idle");
        } else if (!deviceDeliveryAccepted) {
          if (!abortRef.current && !reconnectInFlightRef.current) {
            startReconnectStream({ passive: true });
          }
        }
        return true;
      } catch (err) {
        if (!isCurrentView()) return false;
        clearSendingSubmission();
        if (
          submittedToTransport &&
          loadSubmitted(submittedDraftKey, submissionId)
        ) {
          updateQueuedSubmissions((current) => [
            ...current.filter((item) => item.id !== submissionId),
            {
              id: submissionId,
              submissionId,
              text: trimmed,
              attachments,
              source: "transport",
              deliveryState: "unknown",
              editable: false,
              deletable: false,
            },
          ]);
          setError("Submission result pending confirmation");
          setStatus("idle");
          return "unknown" as const;
        }
        if (
          pendingSubmittedUserRef.current &&
          messageStringId(pendingSubmittedUserRef.current) ===
            messageStringId(message)
        ) {
          pendingSubmittedUserRef.current = null;
        }
        updateQueuedSubmissions((current) => [
          ...current.filter((item) => item.id !== submissionId),
          {
            id: submissionId,
            submissionId,
            text: trimmed,
            attachments,
            messageId: messageStringId(message) || undefined,
            source: "transport",
            deliveryState: "failed",
            editable: true,
            deletable: true,
          },
        ]);
        if ((err as DOMException)?.name !== "AbortError") {
          setError(err instanceof Error ? err.message : String(err));
        }
        return false;
      } finally {
        if (isCurrentView()) {
          setContextCompactionStatus(null);
          if (selectedVariant) {
            clearPendingSelectedVariant(identity.viewKey);
            setSelectedVariantOverrides({});
          }
        }
      }
    },
    [
      conversationId,
      authorityPlacementDeviceId,
      projectId,
      selectedAgent,
      startReconnectStream,
      applyControlProjection,
      updateRemotePendingSubmissions,
      updateQueuedSubmissions,
      isCurrentConversation,
      submitDraftModel,
      runtimeOwnerKey,
      conversationTarget,
      identity.viewKey,
    ],
  );

  const clearError = useCallback(() => {
    setError(null);
  }, []);

  const submitQueuedNow = useCallback(
    async (queuedSubmissionId: string) => {
      const target = queuedSubmissionsRef.current.find(
        (item) => item.id === queuedSubmissionId,
      );
      if (!target || !conversationId) return;
      if (queuedSubmissionsRef.current[0]?.id !== queuedSubmissionId) {
        return;
      }
      if (target.source === "transport") return;
      if (target.serverQueued) {
        const authorityEpoch = history?.control?.placement?.authority_epoch;
        if (typeof authorityEpoch !== "string") {
          setError("Conversation authority is unavailable");
          return;
        }
        try {
          const targetAtCall = conversationTarget;
          const response = await conversationApi.sendNowQueuedInput(
            targetAtCall,
            conversationId,
            target.id,
            {
              command_id: `send-now-${crypto.randomUUID()}`,
              authority_epoch: authorityEpoch,
            },
          );
          applyControlProjection(response.control);
        } catch (err) {
          setError(err instanceof Error ? err.message : String(err));
        }
        return;
      }
      updateQueuedSubmissions((current) =>
        current.filter((item) => item.id !== queuedSubmissionId),
      );
      await submit(
        target.text,
        target.attachments,
        target.messageId,
        target.submissionId,
      );
    },
    [
      applyControlProjection,
      conversationId,
      history?.control?.placement?.authority_epoch,
      submit,
      updateQueuedSubmissions,
      conversationTarget,
    ],
  );

  const reclaimQueuedSubmission = useCallback(
    (queuedSubmissionId: string) => {
      const target = queuedSubmissionsRef.current.find(
        (item) => item.id === queuedSubmissionId,
      );
      if (!target) return null;
      if (target.deliveryState === "unknown") return null;
      if (target.serverQueued) return null;
      updateQueuedSubmissions((current) =>
        current.filter((item) => item.id !== queuedSubmissionId),
      );
      return target;
    },
    [updateQueuedSubmissions],
  );

  const cancelQueuedSubmission = useCallback(
    (queuedSubmissionId: string) => {
      const target = queuedSubmissionsRef.current.find(
        (item) => item.id === queuedSubmissionId,
      );
      if (target?.deliveryState === "unknown") return;
      if (target?.serverQueued) {
        if (!conversationId) return;

        const targetAtCall = conversationTarget;
        void conversationApi
          .deleteQueuedInput(targetAtCall, conversationId, target.id)
          .then((response) => {
            if (!isCurrentConversation()) return;
            setHistory((current) => ({
              ...(current || { messages: [] }),
              status: response.control.active_turn ? "running" : "idle",
              active_turn: response.control.active_turn ?? null,
              control: response.control,
            }));
          })
          .catch((err) => {
            setError(err instanceof Error ? err.message : String(err));
          });
        return;
      }
      updateQueuedSubmissions((current) =>
        current.filter((item) => item.id !== queuedSubmissionId),
      );
    },
    [
      conversationId,
      isCurrentConversation,
      updateQueuedSubmissions,
      conversationTarget,
    ],
  );

  const editQueuedSubmission = useCallback(
    async (queuedSubmissionId: string, nextText: string) => {
      if (!conversationId) return;
      const target = queuedSubmissionsRef.current.find(
        (item) => item.id === queuedSubmissionId,
      );
      if (!target?.serverQueued) return;

      const targetAtCall = conversationTarget;
      try {
        const response = await conversationApi.editQueuedInput(
          targetAtCall,
          conversationId,
          target.id,
          nextText,
        );
        if (!isCurrentConversation()) return;
        setHistory((current) => ({
          ...(current || { messages: [] }),
          status: response.control.active_turn ? "running" : "idle",
          active_turn: response.control.active_turn ?? null,
          control: response.control,
        }));
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [conversationId, isCurrentConversation, conversationTarget],
  );

  const approveTool = useCallback(
    async (
      approvalRequestId: string,
      approve: boolean,
      options?: ApproveOptions,
    ) => {
      if (!conversationId || !approvalRequestId) return;

      const targetAtCall = conversationTarget;
      setHistory((current) =>
        markApprovalDecision(current, approvalRequestId, approve, options),
      );
      setError(null);

      try {
        if (options?.acpOptionId) {
          await conversationApi.decideAcpPermission(
            targetAtCall,
            conversationId,
            approvalRequestId,
            options.acpOptionId,
          );
          setApprovalSnapshot((current) => ({
            pending: current.pending.filter(
              (request) => request.id !== approvalRequestId,
            ),
            resolved: current.resolved,
          }));
          return;
        }

        const decision = approve ? options?.decision ?? "run_once" : "reject";
        let body: ApprovalDecisionRequest;
        if (decision === "allow_chat" || decision === "allow_agent") {
          if (!options?.matcher) {
            throw new Error("Approval reuse matcher is required");
          }
          body = {
            decision,
            decided_by: "webui",
            matcher: options.matcher,
          };
        } else {
          body = {
            decision,
            decided_by: "webui",
          };
        }
        await conversationApi.decideApproval(
          targetAtCall,
          conversationId,
          approvalRequestId,
          body,
        );
        setApprovalSnapshot((current) => {
          const pending = current.pending.filter(
            (request) => request.id !== approvalRequestId,
          );
          const found = current.pending.find(
            (request) => request.id === approvalRequestId,
          );
          const nextStatus: ApprovalStatus = approve ? "approved" : "rejected";
          const resolved = found
            ? [
                {
                  ...found,
                  status: nextStatus,
                  decided_by: "webui",
                  decided_at: new Date().toISOString(),
                },
                ...current.resolved.filter(
                  (request) => request.id !== approvalRequestId,
                ),
              ]
            : current.resolved;
          return { pending, resolved };
        });
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [conversationId, conversationTarget],
  );

  const displayHistory = useMemo(
    () =>
      history && locatedHistory
        ? joinHistoryWindow(history, locatedHistory)
        : history,
    [history, locatedHistory],
  );
  const effectiveHistory = useMemo(() => {
    const projectionState = convergence.project({
      serverHistory: displayHistory,
      pendingDeliveries: remotePendingSubmissions.map(
        remotePendingDeliveryFromSubmission,
      ),
      activeRun: {
        user: activeUser,
        fold: streamFold,
        acpOriginRunner: activeAcpRunner,
      },
    });
    const rendered = withActiveRegeneration(
      projectionState.history,
      activeRegeneration,
    );
    if (
      sendingSubmission?.owner === runtimeOwnerKey &&
      sendingSubmission.chatId === conversationId
    ) {
      return {
        ...(rendered ?? { status: "idle" as const }),
        messages: mergeLiveMessagesIntoHistory(rendered?.messages ?? [], [
          sendingSubmission.message,
        ]),
      };
    }
    return rendered;
  }, [
    convergence,
    displayHistory,
    remotePendingSubmissions,
    activeUser,
    streamFold,
    activeAcpRunner,
    activeRegeneration,
    sendingSubmission,
    runtimeOwnerKey,
    conversationId,
  ]);
  // A projector belongs to the complete scoped address, never just the Chat id.
  const projectConversation = useMemo(createConversationProjector, [
    runtimeOwnerKey,
    conversationId,
  ]);
  const projection = useMemo(
    () =>
      projectConversation(
        conversationId || "_new",
        effectiveHistory || undefined,
        {
          selectedVariantOverrides,
          liveActive: status === "streaming",
          hasLiveProjection: Boolean(activeUser || streamFold),
        },
      ),
    [
      projectConversation,
      activeUser,
      conversationId,
      effectiveHistory,
      selectedVariantOverrides,
      status,
      streamFold,
    ],
  );

  useEffect(() => {
    if (deferInitialAttach) return;
    writeChatViewCache(
      runtimeOwnerKey,
      conversationId,
      history,
      approvalSnapshot,
      retainedCompletedLiveRunsRef.current,
      {
        activeUser,
        streamFold,
        activityCursor: activityCursorRef.current,
      },
      locatedHistory,
    );
  }, [
    activeUser,
    approvalSnapshot,
    conversationId,
    history,
    locatedHistory,
    runtimeOwnerKey,
    streamFold,
    deferInitialAttach,
  ]);

  const selectVariant = useCallback(
    async (turnId: string, variantId: string) => {
      if (!conversationId || !turnId || !variantId) return;

      const targetAtCall = conversationTarget;
      await conversationApi.selectVariant(
        targetAtCall,
        conversationId,
        turnId,
        variantId,
      );
      if (
        isCurrentConversation() &&
        conversationIdRef.current === conversationId
      ) {
        applyHistoryMutation({
          kind: "history_mutated",
          action: "variant_selected",
          turnId,
          selectedVariantId: variantId,
          raw: {},
        });
      }
    },
    [
      applyHistoryMutation,
      conversationId,
      isCurrentConversation,
      conversationTarget,
    ],
  );

  const regenerate = useCallback(
    async (turnId: string, userText: string) => {
      const text = userText.trim();
      const item = projection.items.find(
        (candidate) =>
          candidate.kind === "assistant" && candidate.turn_id === turnId,
      ) as ConversationAssistantResponse | undefined;
      const regenerateText = text || (item ? assistantResponseText(item) : "");
      if (!regenerateText || !conversationId) return;

      const agentAtCall = selectedAgent;
      const chatAtCall = conversationId;
      const targetAtCall = conversationTarget;
      const baselineVariantIds = Array.from(
        new Set([
          ...(item?.variants.map((variant) => variant.variant_id) || []),
          ...(item?.selected_variant_id ? [item.selected_variant_id] : []),
        ]),
      );
      const regenerationId = crypto.randomUUID();
      const message = userMessage(
        regenerateText,
        [],
        undefined,
        undefined,
        `submission-${regenerationId}`,
      );
      const controller = new AbortController();
      const variantId = `${turnId}-pending-${regenerationId}`;
      submitAbortRef.current = controller;
      clearPendingStreamFoldFlush();
      clearPendingActiveRegenerationFlush();
      setActiveUser(null);
      setStreamFold(null);
      const nextActiveRegeneration = {
        turnId,
        variantId,
        baselineVariantIds,
        fold: initialConversationStreamFold,
      };
      activeRegenerationRef.current = nextActiveRegeneration;
      setActiveRegeneration(nextActiveRegeneration);
      setContextCompactionStatus(null);
      setStatus("streaming");
      setError(null);

      try {
        const submitRequest = {
          ...buildSubmitRequest(
            conversationId,
            message,
            `submission-${regenerationId}`,
            null,
          ),
          regenerate_turn_id: turnId,
        };
        const response = await conversationApi.submit(
          targetAtCall,
          chatAtCall,
          submitRequest,
          projectId ? agentAtCall : undefined,
          controller.signal,
        );

        if (!response.ok) {
          const body = await response.text().catch(() => "");
          throw new Error(
            `Chat submit failed: ${response.status}${body ? ` ${body}` : ""}`,
          );
        }

        const result = (await response
          .json()
          .catch(() => null)) as SubmitChatResult | null;
        if (
          submitAbortRef.current !== controller ||
          !isCurrentConversation() ||
          conversationIdRef.current !== chatAtCall
        ) {
          return;
        }
        if (!result?.accepted) {
          throw new Error(result?.detail || "Chat submit was not accepted");
        }
        if (result.control) {
          applyControlProjection(result.control);
        }
        if (submitAbortRef.current === controller) {
          submitAbortRef.current = null;
        }
        startReconnectStream({
          regeneration: { turnId, variantId, baselineVariantIds },
        });
      } catch (err) {
        if (
          submitAbortRef.current !== controller ||
          !isCurrentConversation() ||
          conversationIdRef.current !== chatAtCall
        ) {
          return;
        }
        if ((err as DOMException)?.name === "AbortError") {
          const latest = takePendingActiveRegeneration();
          const fold = finishConversationStream(
            latest?.fold ?? initialConversationStreamFold,
          );
          publishActiveRegeneration(
            { turnId, variantId, baselineVariantIds, fold },
            true,
          );
        } else {
          setError(err instanceof Error ? err.message : String(err));
          const latest = takePendingActiveRegeneration();
          const fold = foldConversationStreamEvent(
            latest?.fold ?? initialConversationStreamFold,
            {
              kind: "parse_error",
              error: err instanceof Error ? err : new Error(String(err)),
              raw: "",
            },
          );
          publishActiveRegeneration(
            { turnId, variantId, baselineVariantIds, fold },
            true,
          );
        }
      } finally {
        if (submitAbortRef.current === controller) {
          submitAbortRef.current = null;
          setContextCompactionStatus(null);
          if (
            abortRef.current === null &&
            reconnectInFlightRef.current === null
          ) {
            setStatus("idle");
          }
        }
      }
    },
    [
      conversationId,
      clearPendingActiveRegenerationFlush,
      clearPendingStreamFoldFlush,
      applyControlProjection,
      isCurrentConversation,
      projectId,
      publishActiveRegeneration,
      projection.items,
      selectedAgent,
      startReconnectStream,
      takePendingActiveRegeneration,
      conversationTarget,
    ],
  );

  const rollback = useCallback(
    async (beforeTurnId: string) => {
      if (!conversationId || !beforeTurnId) return;
      setLocatedHistory(null);
      locateVersion.current += 1;

      const targetAtCall = conversationTarget;
      await conversationApi.rollback(targetAtCall, conversationId, {
        before_turn_id: beforeTurnId,
      });
      if (
        isCurrentConversation() &&
        conversationIdRef.current === conversationId
      ) {
        applyHistoryMutation({
          kind: "history_mutated",
          action: "rolled_back",
          beforeTurnId,
          raw: {},
        });
        reload();
      }
    },
    [
      applyHistoryMutation,
      conversationId,
      isCurrentConversation,
      reload,
      conversationTarget,
    ],
  );

  const branch = useCallback(
    async (beforeTurnId: string) => {
      if (!conversationId || !beforeTurnId) return conversationId || "";

      const targetAtCall = conversationTarget;
      const result = await conversationApi.branch(
        targetAtCall,
        conversationId,
        { before_turn_id: beforeTurnId },
      );
      return result.chat.id;
    },
    [conversationId, conversationTarget],
  );

  const loading =
    status === "loading" || Boolean(conversationId && history === null);
  const applyCommittedModel = useCallback((model: ModelSlot | null) => {
    committedModelRef.current = model;
    setHistory((current) => {
      if (!current) {
        return model ? { status: "idle", messages: [], model } : current;
      }
      if (model === current.model) return current;
      return { ...current, model };
    });
  }, []);

  return {
    identity,
    executionAgentId: selectedAgent,
    history: effectiveHistory,
    authoritativeHistory: history,
    projection,
    approvalSnapshot,
    loading,
    contextCompactionStatus,
    loadingOlder,
    hasMoreBefore: Boolean(
      locatedHistory &&
        (locatedHistory.oldest_turn_ordinal ?? 0) <
          (history?.oldest_turn_ordinal ?? Infinity)
        ? locatedHistory.has_more_before
        : history?.has_more_before,
    ),
    error,
    authorityPlacementDeviceId,
    authorityPlacementLocked,
    clearError,
    submit,
    stop,
    queuedSubmissions,
    submitQueuedNow,
    reclaimQueuedSubmission,
    cancelQueuedSubmission,
    editQueuedSubmission,
    loadOlder,
    locateTurn,
    loadHistoryGap,
    historyGap: historyWindowGap(history, locatedHistory),
    reload,
    applyCommittedModel,
    selectVariant,
    regenerate: async (turnId: string, userText: string) => {
      const fallback = userText || textFromUserBeforeTurn(projection, turnId);
      await regenerate(turnId, fallback);
    },
    rollback,
    branch,
    approveTool,
  };
}
