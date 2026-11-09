import type { ChatHistory, Message, RuntimeNote } from "@/api/types";
import {
  deriveTurnActivity,
  type TurnActivity,
} from "./conversationStateConvergence";

export type ConversationContentPart = {
  type?: string;
  text?: string;
  image_url?: string;
  video_url?: string;
  video_poster?: string;
  audio_url?: string;
  data?: unknown;
  file_url?: string;
  file_id?: string;
  file_name?: string;
  filename?: string;
  url?: string;
  name?: string;
  alt?: string;
  file_size?: number;
  media_ref?: string;
  [key: string]: unknown;
};

export type ConversationRuntimeNote = RuntimeNote;

export function runtimeNoteField(
  note: ConversationRuntimeNote,
  field: string,
): unknown {
  const payload =
    note.payload && typeof note.payload === "object" ? note.payload : undefined;
  return payload?.[field];
}

export function runtimeNoteTerminalStatus(
  note: ConversationRuntimeNote,
): string | undefined {
  const value = runtimeNoteField(note, "terminal_status");
  return typeof value === "string" && value ? value : undefined;
}

export type TokenUsage = {
  prompt_tokens?: number;
  completion_tokens?: number;
  total_tokens?: number;
  cache_read_tokens?: number;
  cache_write_tokens?: number;
  estimated?: boolean;
};

export type TokenUsageSnapshot = {
  usage: TokenUsage | null;
  last_call_usage: TokenUsage | null;
  max_input_length: number | null;
};

export type ConversationOutputMessage = Omit<Message, "content"> & {
  id: string;
  role: string;
  type?: string;
  content: ConversationContentPart[];
  sequence_number?: number;
  turn_id?: string;
  selected_variant_id?: string;
  assistant_response_variants?: AssistantResponseVariant[];
  runtime_note?: ConversationRuntimeNote;
  reasoning_only_fallback?: boolean;
  token_usage?: TokenUsageSnapshot;
};

export type AssistantResponseVariant = {
  variant_id: string;
  output: ConversationOutputMessage[];
};

export type RemoteInputDeliveryState = "sending" | "pending";

export type ConversationUserMessage = {
  kind: "user";
  id: string;
  role: "user";
  content: ConversationContentPart[];
  turn_id?: string;

  created_at?: string;

  acp_origin_runner?: string;
  remote_delivery_state?: RemoteInputDeliveryState;
};

export type ConversationAssistantResponse = {
  kind: "assistant";
  id: string;
  role: "assistant";
  status: "completed" | "generating" | "failed" | "interrupted";
  output: ConversationOutputMessage[];
  turn_id?: string;
  selected_variant_id?: string;
  started_at?: string;
  finished_at?: string;

  acp_origin_runner?: string;
  variants: AssistantResponseVariant[];
};

export type ConversationAcpHistoryPreview = {
  kind: "acp_history_preview";
  id: string;
  role: "ui";
  content: ConversationContentPart[];
  created_at?: string;
  turn_id?: string;
  acp_origin_runner?: string;
};

export type ConversationProjectionItem =
  | ConversationUserMessage
  | ConversationAssistantResponse
  | ConversationAcpHistoryPreview;

export type ConversationProjection = {
  conversationId: string;
  turnActivity: TurnActivity;
  items: ConversationProjectionItem[];
};

export type ConversationProjectionOptions = {
  selectedVariantOverrides?: Record<string, string>;
  liveActive?: boolean;
  hasLiveProjection?: boolean;
};

const ROLE_USER = "user";
const TYPE_PLUGIN_CALL_OUTPUT = "plugin_call_output";
const HIDDEN_RUNTIME_NOTE_KINDS = new Set([
  "memory_signal",
  "collaboration_result",
]);
const HIDDEN_MODEL_VISIBLE_MARKERS = new Set(["memory_signal"]);
const TURN_ABORTED_MARKER = "turn_aborted";
const TURN_INTERJECTED_MARKER = "turn_interjected";
const RUA_TOKEN_USAGE_META_KEY = "rua_token_usage";

function canonicalMessageId(message: unknown, kind: string): string {
  const id = objectRecord(message)?.id;
  if (typeof id === "string" && id) return id;
  throw new Error(`${kind} is missing canonical message_id`);
}

export function normalizeContentParts(
  content: unknown,
): ConversationContentPart[] {
  if (typeof content === "string") {
    return content ? [{ type: "text", text: content }] : [];
  }
  if (content == null) return [];
  if (
    !Array.isArray(content) &&
    (typeof content !== "object" ||
      typeof (content as Record<string, unknown>).type !== "string")
  ) {
    return [{ type: "text", text: String(content) }];
  }
  const parts = Array.isArray(content) ? content : [content];
  return (parts as unknown[]).map((part) => {
    if (!part || typeof part !== "object") {
      return { type: "text", text: String(part ?? "") };
    }
    return part as ConversationContentPart;
  });
}

function resolveRuntimeNote(msg: Message): ConversationRuntimeNote | undefined {
  const direct = (msg as { runtime_note?: ConversationRuntimeNote })
    .runtime_note;
  return direct;
}

function objectRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object"
    ? (value as Record<string, unknown>)
    : null;
}

function numberValue(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function parseTokenUsagePayload(raw: unknown): TokenUsageSnapshot | null {
  const obj = objectRecord(raw);
  if (!obj) return null;
  const usageRaw = objectRecord(obj.usage);
  const lastCallRaw = objectRecord(obj.last_call_usage);
  const usageTotal = usageRaw
    ? numberValue(usageRaw.total_tokens) ||
      numberValue(usageRaw.prompt_tokens) +
        numberValue(usageRaw.completion_tokens)
    : 0;
  const lastCallTotal = lastCallRaw
    ? numberValue(lastCallRaw.total_tokens) ||
      numberValue(lastCallRaw.prompt_tokens) +
        numberValue(lastCallRaw.completion_tokens)
    : 0;
  if (usageTotal <= 0 && lastCallTotal <= 0) return null;
  return {
    usage: usageTotal > 0 ? (usageRaw as TokenUsage) : null,
    last_call_usage: lastCallTotal > 0 ? (lastCallRaw as TokenUsage) : null,
    max_input_length:
      numberValue(obj.max_input_length) > 0
        ? numberValue(obj.max_input_length)
        : null,
  };
}

export function readTokenUsageSnapshot(
  raw: unknown,
): TokenUsageSnapshot | null {
  const msg = objectRecord(raw);
  if (!msg) return null;
  const metadata = objectRecord(msg.metadata);
  if (!metadata) return null;
  const direct = parseTokenUsagePayload(metadata[RUA_TOKEN_USAGE_META_KEY]);
  if (direct) return direct;
  const nestedMetadata = objectRecord(metadata.metadata);
  return parseTokenUsagePayload(nestedMetadata?.[RUA_TOKEN_USAGE_META_KEY]);
}

function turnMarkerType(
  msg: Message,
): "turn_aborted" | "turn_interjected" | null {
  const metadata = objectRecord(msg.metadata);
  const nestedMetadata = objectRecord(metadata?.metadata);
  const marker =
    typeof metadata?.model_visible_marker === "string"
      ? metadata.model_visible_marker
      : typeof nestedMetadata?.model_visible_marker === "string"
      ? nestedMetadata.model_visible_marker
      : null;
  if (marker === TURN_ABORTED_MARKER || marker === TURN_INTERJECTED_MARKER) {
    return marker;
  }
  const text = normalizeContentParts(msg.content)
    .map((part) => (typeof part.text === "string" ? part.text : ""))
    .join("\n");
  if (text.includes("<turn_aborted>")) return TURN_ABORTED_MARKER;
  if (text.includes("<turn_interjected>")) return TURN_INTERJECTED_MARKER;
  return null;
}

function modelVisibleMarker(msg: Message): string | null {
  const metadata = objectRecord(msg.metadata);
  const nestedMetadata = objectRecord(metadata?.metadata);
  return typeof metadata?.model_visible_marker === "string"
    ? metadata.model_visible_marker
    : typeof nestedMetadata?.model_visible_marker === "string"
    ? nestedMetadata.model_visible_marker
    : null;
}

function hiddenModelMarkerType(msg: Message): string | null {
  const marker = modelVisibleMarker(msg);
  if (marker && HIDDEN_MODEL_VISIBLE_MARKERS.has(marker)) {
    return marker;
  }
  const text = normalizeContentParts(msg.content)
    .map((part) => (typeof part.text === "string" ? part.text : ""))
    .join("\n");
  if (text.includes("<memory_signal>")) return "memory_signal";
  return null;
}

function messagesWithTerminalMarkersInTheirTurns(
  messages: Message[],
): Message[] {
  const markersByTurn = new Map<string, Message[]>();
  const messagesWithoutMarkers: Message[] = [];

  messages.forEach((msg) => {
    const turnId = typeof msg.turn_id === "string" ? msg.turn_id : undefined;
    if (turnId && turnMarkerType(msg)) {
      const markers = markersByTurn.get(turnId) ?? [];
      markers.push(msg);
      markersByTurn.set(turnId, markers);
      return;
    }
    messagesWithoutMarkers.push(msg);
  });

  if (markersByTurn.size === 0) return messages;

  const normalized: Message[] = [];
  messagesWithoutMarkers.forEach((msg, index) => {
    normalized.push(msg);
    const turnId = typeof msg.turn_id === "string" ? msg.turn_id : undefined;
    if (!turnId) return;
    const nextMessageTurnId = messagesWithoutMarkers[index + 1]?.turn_id;
    const nextTurnId =
      typeof nextMessageTurnId === "string" ? nextMessageTurnId : undefined;
    if (nextTurnId === turnId) return;
    const markers = markersByTurn.get(turnId);
    if (markers) {
      normalized.push(...markers);
      markersByTurn.delete(turnId);
    }
  });

  for (const markers of markersByTurn.values()) normalized.push(...markers);
  return normalized;
}

function isAcpHistoryPreviewMessage(msg: Message): boolean {
  if (msg.type === "acp_history_preview") return true;
  const metadata = objectRecord(msg.metadata);
  const nestedMetadata = objectRecord(metadata?.metadata);
  return (
    metadata?.message_type === "acp_history_preview" ||
    nestedMetadata?.message_type === "acp_history_preview" ||
    metadata?.acp_history_preview === true ||
    nestedMetadata?.acp_history_preview === true
  );
}

function acpOriginRunner(msg: Message): string | undefined {
  const metadata = objectRecord(msg.metadata);
  const nestedMetadata = objectRecord(metadata?.metadata);
  const origin =
    objectRecord(metadata?.origin) ?? objectRecord(nestedMetadata?.origin);
  if (!origin || origin.kind !== "acp") return undefined;
  return typeof origin.runner === "string" ? origin.runner : "";
}

function acpRunnerFromActiveTurn(history: ChatHistory | null | undefined) {
  if (history?.active_turn?.driver !== "acp") return undefined;
  const runner = history.active_turn.driver_detail?.runner;
  return typeof runner === "string" ? runner : "";
}

function isTurnMarkerType(type: unknown): boolean {
  return type === TURN_ABORTED_MARKER || type === TURN_INTERJECTED_MARKER;
}

export function isVisibleRuntimeNote(
  note: ConversationRuntimeNote | undefined,
) {
  return !note || !HIDDEN_RUNTIME_NOTE_KINDS.has(note.kind);
}

export function toConversationOutputMessage(
  msg: Message,
  _index: number,
): ConversationOutputMessage {
  const runtimeNote = resolveRuntimeNote(msg);
  const turnMarker = turnMarkerType(msg);
  const turnUsage = readTokenUsageSnapshot(msg);
  return {
    ...msg,
    id: canonicalMessageId(msg, "assistant output"),
    type: runtimeNote
      ? "runtime_note"
      : turnMarker
      ? turnMarker
      : (msg.type as string | undefined),
    status: turnMarker ? "interrupted" : msg.status,
    role:
      msg.type === TYPE_PLUGIN_CALL_OUTPUT && msg.role === "system"
        ? "tool"
        : String(msg.role || "assistant"),
    content: normalizeContentParts(msg.content),
    runtime_note: runtimeNote,
    token_usage: turnUsage ?? undefined,
  };
}

function normalizeVariantOutput(
  output: ConversationOutputMessage[] | undefined,
): ConversationOutputMessage[] {
  return (output || []).map((msg) => ({
    ...msg,
    id: canonicalMessageId(msg, "assistant variant output"),
    content: normalizeContentParts(msg.content),
    token_usage: readTokenUsageSnapshot(msg) ?? msg.token_usage,
  }));
}

function normalizeReasoningOnlyOutput(
  output: ConversationOutputMessage[],
): ConversationOutputMessage[] {
  const visible = output.filter((msg) => !msg.runtime_note);
  if (
    visible.length === 0 ||
    !visible.every((msg) => msg.type === "reasoning")
  ) {
    return output;
  }
  return output.map((msg) =>
    msg.type === "reasoning"
      ? {
          ...msg,
          type: "message",
          reasoning_only_fallback: true,
        }
      : msg,
  );
}

function buildAssistantResponse(
  conversationId: string,
  output: ConversationOutputMessage[],
  responseKey: string,
  status: ConversationAssistantResponse["status"] = "completed",
  selectedVariantOverrides: Record<string, string> = {},
  acpOriginRunner?: string,
): ConversationAssistantResponse {
  const variantSource = output.find((msg) =>
    Array.isArray(msg.assistant_response_variants),
  );
  const metadataSource =
    variantSource ||
    output.find(
      (msg) => msg.turn_id || msg.selected_variant_id || msg.variant_id,
    );
  const variants = (variantSource?.assistant_response_variants || []).map(
    (variant) => ({
      variant_id: variant.variant_id,
      output: normalizeVariantOutput(variant.output),
    }),
  );

  const turnId = metadataSource?.turn_id;
  const selectedVariantId =
    (turnId ? selectedVariantOverrides[turnId] : undefined) ||
    variantSource?.selected_variant_id ||
    metadataSource?.selected_variant_id ||
    (typeof metadataSource?.variant_id === "string"
      ? metadataSource.variant_id
      : undefined);
  const selectedVariant = variants.find(
    (variant) => variant.variant_id === selectedVariantId,
  );
  const selectedOutput = selectedVariant
    ? [
        ...selectedVariant.output,
        ...output.filter(
          (msg) =>
            msg.runtime_note &&
            !selectedVariant.output.some(
              (variantMsg) => variantMsg.id === msg.id,
            ),
        ),
      ]
    : output;
  const normalizedOutput =
    status === "generating"
      ? selectedOutput
      : normalizeReasoningOnlyOutput(selectedOutput);
  const startedAt =
    typeof metadataSource?.started_at === "string"
      ? metadataSource.started_at
      : undefined;
  const finishedAt =
    typeof metadataSource?.finished_at === "string"
      ? metadataSource.finished_at
      : undefined;

  return {
    kind: "assistant",
    id: `${conversationId}:assistant:${responseKey}`,
    role: "assistant",
    status,
    output: normalizedOutput,
    turn_id: turnId,
    selected_variant_id: selectedVariantId,
    started_at: startedAt,
    finished_at: finishedAt,
    acp_origin_runner: acpOriginRunner,
    variants,
  };
}

function responseStatusFromOutput(
  output: ConversationOutputMessage[],
  fallback: ConversationAssistantResponse["status"],
): ConversationAssistantResponse["status"] {
  if (output.some((msg) => msg.status === "interrupted")) {
    return "interrupted";
  }
  if (
    output.some(
      (msg) =>
        msg.runtime_note &&
        runtimeNoteTerminalStatus(msg.runtime_note) === "interrupted",
    )
  ) {
    return "interrupted";
  }
  if (output.some((msg) => msg.status === "failed")) {
    return "failed";
  }
  if (
    output.some(
      (msg) =>
        msg.runtime_note &&
        runtimeNoteTerminalStatus(msg.runtime_note) === "failed",
    )
  ) {
    return "failed";
  }
  return fallback;
}

type ProjectionItemMemo = (
  key: string,
  inputs: readonly unknown[],
  build: () => ConversationProjectionItem,
) => ConversationProjectionItem;

// Each View Runtime owns only its latest item cache. Source Messages are
// immutable; replacement, variant selection, and control changes invalidate it.
export function createConversationProjector(): typeof projectConversation {
  let cachedConversationId: string | undefined;
  let previous = new Map<
    string,
    { inputs: readonly unknown[]; item: ConversationProjectionItem }
  >();
  return (conversationId, history, options = {}) => {
    if (cachedConversationId !== conversationId) previous.clear();
    cachedConversationId = conversationId;
    const next = new Map<
      string,
      { inputs: readonly unknown[]; item: ConversationProjectionItem }
    >();
    const projection = projectConversationWithMemo(
      conversationId,
      history,
      options,
      (key, inputs, build) => {
        const cached = previous.get(key);
        const item =
          cached &&
          cached.inputs.length === inputs.length &&
          inputs.every((input, index) => Object.is(input, cached.inputs[index]))
            ? cached.item
            : build();
        next.set(key, { inputs, item });
        return item;
      },
    );
    previous = next;
    return projection;
  };
}

export function projectConversation(
  conversationId: string,
  history: ChatHistory | null | undefined,
  options: ConversationProjectionOptions = {},
): ConversationProjection {
  return projectConversationWithMemo(conversationId, history, options);
}

function projectConversationWithMemo(
  conversationId: string,
  history: ChatHistory | null | undefined,
  options: ConversationProjectionOptions,
  memo: ProjectionItemMemo = (_key, _inputs, build) => build(),
): ConversationProjection {
  const baseTurnActivity = deriveTurnActivity(history, options.liveActive);
  const activeTurnId =
    baseTurnActivity.kind === "active" ? baseTurnActivity.turnId : null;
  const activeTurnAcpOriginRunner = acpRunnerFromActiveTurn(history);
  const sourceMessages = messagesWithTerminalMarkersInTheirTurns([
    ...(history?.messages || []),
  ])
    .filter((msg) => isVisibleRuntimeNote(resolveRuntimeNote(msg)))
    .filter((msg) => !hiddenModelMarkerType(msg));

  const items: ConversationProjectionItem[] = [];
  let i = 0;
  let precedingUserId: string | undefined;
  let pendingAcpOriginRunner: string | undefined;
  let pendingUserBelongsToActiveTurn = false;

  while (i < sourceMessages.length) {
    const msg = sourceMessages[i];
    if (isAcpHistoryPreviewMessage(msg)) {
      precedingUserId = undefined;
      const id = canonicalMessageId(msg, "ACP history preview");
      items.push(
        memo(id, [msg], () => ({
          kind: "acp_history_preview",
          id,
          role: "ui",
          content: normalizeContentParts(msg.content),
          created_at:
            typeof msg.created_at === "string" && msg.created_at
              ? msg.created_at
              : undefined,
          turn_id:
            typeof msg.turn_id === "string" && msg.turn_id
              ? msg.turn_id
              : undefined,
          acp_origin_runner: acpOriginRunner(msg),
        })),
      );
      i += 1;
      continue;
    }

    if (msg.role === ROLE_USER) {
      let userTurnId =
        typeof msg.turn_id === "string" && msg.turn_id
          ? msg.turn_id
          : undefined;
      if (!userTurnId) {
        for (let cursor = i + 1; cursor < sourceMessages.length; cursor += 1) {
          const candidate = sourceMessages[cursor];
          if (candidate.role === ROLE_USER) break;
          if (typeof candidate.turn_id === "string" && candidate.turn_id) {
            userTurnId = candidate.turn_id;
            break;
          }
        }
      }
      const userCreatedAt =
        typeof msg.created_at === "string" && msg.created_at
          ? msg.created_at
          : typeof msg.timestamp === "string" && msg.timestamp
          ? msg.timestamp
          : undefined;
      const acpRunner: string | undefined = (() => {
        const meta = msg.metadata;
        if (!meta || typeof meta !== "object") return undefined;
        const origin = (meta as Record<string, unknown>).origin;
        if (!origin || typeof origin !== "object") return undefined;
        const kind = (origin as Record<string, unknown>).kind;
        if (kind !== "acp") return undefined;
        const runner = (origin as Record<string, unknown>).runner;
        return typeof runner === "string" ? runner : "";
      })();
      const remoteDeliveryState =
        msg.remote_delivery_state === "sending" ||
        msg.remote_delivery_state === "pending"
          ? msg.remote_delivery_state
          : msg.remote_pending === true
          ? "pending"
          : msg.remote_delivery_status === "delivering"
          ? "sending"
          : undefined;
      const id = canonicalMessageId(msg, "user message");
      precedingUserId = id;
      items.push(
        memo(id, [msg, userTurnId], () => ({
          kind: "user",
          id,
          role: "user",
          content: normalizeContentParts(msg.content),
          turn_id:
            typeof userTurnId === "string" && userTurnId
              ? userTurnId
              : undefined,
          created_at: userCreatedAt,
          acp_origin_runner: acpRunner,
          remote_delivery_state: remoteDeliveryState,
        })),
      );
      pendingUserBelongsToActiveTurn = activeTurnId
        ? userTurnId === activeTurnId ||
          msg.id ===
            (baseTurnActivity.kind === "active"
              ? baseTurnActivity.userMessageId
              : undefined)
        : false;
      pendingAcpOriginRunner = acpRunner;
      i += 1;
      continue;
    }

    const sourceOutput: Message[] = [];
    while (
      i < sourceMessages.length &&
      sourceMessages[i].role !== ROLE_USER &&
      !isAcpHistoryPreviewMessage(sourceMessages[i])
    ) {
      sourceOutput.push(sourceMessages[i]);
      i += 1;
    }
    if (sourceOutput.length) {
      const generating =
        baseTurnActivity.kind === "active" &&
        i >= sourceMessages.length &&
        pendingUserBelongsToActiveTurn;
      const responseKey =
        precedingUserId ??
        canonicalMessageId(sourceOutput[0], "assistant message");
      const assistant = memo(
        `${conversationId}:assistant:${responseKey}`,
        [
          generating,
          options.selectedVariantOverrides,
          pendingAcpOriginRunner,
          ...sourceOutput,
        ],
        () => {
          const output = sourceOutput.map(toConversationOutputMessage);
          return buildAssistantResponse(
            conversationId,
            output,
            responseKey,
            generating
              ? "generating"
              : responseStatusFromOutput(output, "completed"),
            options.selectedVariantOverrides,
            pendingAcpOriginRunner,
          );
        },
      );
      items.push(assistant);
      precedingUserId = undefined;
      pendingAcpOriginRunner = undefined;
      pendingUserBelongsToActiveTurn = false;
    }
  }

  const lastItem = items[items.length - 1];
  if (
    baseTurnActivity.kind === "active" &&
    lastItem?.kind === "user" &&
    (pendingUserBelongsToActiveTurn || options.hasLiveProjection)
  ) {
    items.push(
      buildAssistantResponse(
        conversationId,
        [],
        lastItem.id,
        "generating",
        options.selectedVariantOverrides,
        pendingAcpOriginRunner ?? activeTurnAcpOriginRunner,
      ),
    );
  }

  const lastAssistant = [...items]
    .reverse()
    .find(
      (item): item is ConversationAssistantResponse =>
        item.kind === "assistant",
    );
  const identityMatchedResponse = items.find(
    (item): item is ConversationAssistantResponse =>
      item.kind === "assistant" &&
      Boolean(activeTurnId) &&
      item.turn_id === activeTurnId,
  );
  const generatingResponse = items.find(
    (item): item is ConversationAssistantResponse =>
      item.kind === "assistant" && item.status === "generating",
  );
  const activeResponse =
    baseTurnActivity.kind === "active"
      ? identityMatchedResponse ??
        generatingResponse ??
        (options.hasLiveProjection ? lastAssistant : undefined)
      : undefined;
  const turnActivity: TurnActivity =
    baseTurnActivity.kind === "active"
      ? { ...baseTurnActivity, responseId: activeResponse?.id ?? null }
      : baseTurnActivity;

  return {
    conversationId,
    turnActivity,
    items,
  };
}

export function assistantResponseText(
  response: ConversationAssistantResponse,
): string {
  return response.output
    .filter((msg) => !isTurnMarkerType(msg.type))
    .flatMap((msg) =>
      msg.content
        .filter((part) => part.type === "text" && typeof part.text === "string")
        .map((part) => part.text || ""),
    )
    .filter(Boolean)
    .join("\n\n");
}
