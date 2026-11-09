import type { ChatHistory, Message } from "@/api/types";
import type { ConversationStreamFold } from "./stream";

export type ActiveRunOverlay = {
  user: Message | null;
  fold: ConversationStreamFold | null;
  acpOriginRunner?: string | null;
};

export type RemotePendingDelivery = {
  deviceDeliveryId: string;
  targetDeviceId: string;
  deviceDeliverySequence: number;
  acceptedAt?: string;
  submissionId?: string;
  inputId?: string;
  turnId?: string;
  message: Message;
};

function isMessage(value: unknown): value is Message {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

type IndexedMessages = {
  messages: Message[];
  identity: ReturnType<typeof buildMessageIdentityIndex>;
};

// History arrays are immutable snapshots. A new snapshot gets a new index;
// weak keys let replaced histories and their derived indexes be collected.
const historyMessageIndexes = new WeakMap<Message[], IndexedMessages>();
const EMPTY_MESSAGES: Message[] = [];

function indexHistoryMessages(messages: Message[]): IndexedMessages {
  const cached = historyMessageIndexes.get(messages);
  if (cached) return cached;
  const valid = messages.filter(isMessage);
  const indexed = {
    messages: valid.length === messages.length ? messages : valid,
    identity: buildMessageIdentityIndex(valid),
  };
  historyMessageIndexes.set(messages, indexed);
  return indexed;
}

function withAcpOrigin(message: Message, runner: string | null | undefined) {
  if (runner === undefined || runner === null) return message;
  const metadata =
    message.metadata && typeof message.metadata === "object"
      ? { ...(message.metadata as Record<string, unknown>) }
      : {};
  const existingOrigin = metadata.origin;
  if (
    existingOrigin &&
    typeof existingOrigin === "object" &&
    (existingOrigin as Record<string, unknown>).kind === "acp"
  ) {
    return message;
  }
  return {
    ...message,
    metadata: {
      ...metadata,
      origin: { kind: "acp", runner },
    },
  };
}

function acpRunnerFromMessage(message: Message | null): string | null {
  const metadata = message?.metadata;
  if (!metadata || typeof metadata !== "object") return null;
  const origin = (metadata as Record<string, unknown>).origin;
  if (!origin || typeof origin !== "object") return null;
  if ((origin as Record<string, unknown>).kind !== "acp") return null;
  const runner = (origin as Record<string, unknown>).runner;
  return typeof runner === "string" ? runner : "";
}

function messageId(message: Message): string | null {
  return typeof message.id === "string" && message.id ? message.id : null;
}

function turnId(message: Message): string | null {
  return typeof message.turn_id === "string" && message.turn_id
    ? message.turn_id
    : null;
}

function withTokenUsage(
  message: Message,
  fold: ConversationStreamFold,
  index: number,
): Message {
  if (!fold.usage || index !== 0 || message.role !== "assistant")
    return message;
  const metadata =
    message.metadata && typeof message.metadata === "object"
      ? { ...(message.metadata as Record<string, unknown>) }
      : {};
  return {
    ...message,
    metadata: {
      ...metadata,
      rua_token_usage: fold.usage,
    },
  };
}

function metadataString(message: Message, key: string): string | null {
  const metadata = message.metadata;
  if (!metadata || typeof metadata !== "object") return null;
  const direct = (metadata as Record<string, unknown>)[key];
  if (typeof direct === "string" && direct) return direct;
  const nested = (metadata as Record<string, unknown>).metadata;
  if (!nested || typeof nested !== "object") return null;
  const nestedValue = (nested as Record<string, unknown>)[key];
  return typeof nestedValue === "string" && nestedValue ? nestedValue : null;
}

function deviceDeliveryId(message: Message): string | null {
  return metadataString(message, "device_delivery_id");
}

function pendingDeliveryMatchesMessage(
  delivery: RemotePendingDelivery,
  message: Message,
): boolean {
  if (message.role !== "user") return false;
  if (deviceDeliveryId(message) === delivery.deviceDeliveryId) return true;
  if (
    delivery.submissionId &&
    metadataString(message, "submission_id") === delivery.submissionId
  ) {
    return true;
  }
  if (
    delivery.inputId &&
    metadataString(message, "input_id") === delivery.inputId
  ) {
    return true;
  }
  const pendingMessageId = messageId(delivery.message);
  if (pendingMessageId && messageId(message) === pendingMessageId) return true;
  const pendingTurnId = delivery.turnId ?? turnId(delivery.message);
  return Boolean(pendingTurnId && turnId(message) === pendingTurnId);
}

export function remotePendingDeliveryCoveredByHistory(
  history: ChatHistory | null,
  delivery: RemotePendingDelivery,
): boolean {
  return (history?.messages || []).some((message) =>
    pendingDeliveryMatchesMessage(delivery, message),
  );
}

function pendingDeliveryMatchesLiveUser(
  delivery: RemotePendingDelivery,
  liveUser: Message | null,
): boolean {
  return Boolean(liveUser && pendingDeliveryMatchesMessage(delivery, liveUser));
}

function pendingDeliveryMessage(
  delivery: RemotePendingDelivery,
  acpOriginRunner?: string | null,
): Message {
  const metadata =
    delivery.message.metadata && typeof delivery.message.metadata === "object"
      ? { ...(delivery.message.metadata as Record<string, unknown>) }
      : {};
  const message: Message = withAcpOrigin(
    {
      ...delivery.message,
      id:
        messageId(delivery.message) ||
        `remote-pending-${delivery.deviceDeliveryId}`,
      remote_delivery_state: "pending",
      metadata: {
        ...metadata,
        ...(delivery.submissionId
          ? { submission_id: delivery.submissionId }
          : {}),
        ...(delivery.inputId ? { input_id: delivery.inputId } : {}),
        device_delivery_id: delivery.deviceDeliveryId,
        device_delivery_sequence: delivery.deviceDeliverySequence,
        target_device_id: delivery.targetDeviceId,
        device_delivery_pending: true,
      },
      ...(delivery.turnId ? { turn_id: delivery.turnId } : {}),
    },
    acpOriginRunner,
  );
  delete message.remote_delivery_status;
  delete message.remote_pending;
  return message;
}

function pendingDeliveryMessages(
  history: ChatHistory | null,
  deliveries: RemotePendingDelivery[],
  liveUser: Message | null,
  acpOriginRunner?: string | null,
): Message[] {
  if (deliveries.length === 0) return [];
  return deliveries
    .filter(
      (delivery) => !remotePendingDeliveryCoveredByHistory(history, delivery),
    )
    .filter((delivery) => !pendingDeliveryMatchesLiveUser(delivery, liveUser))
    .sort(
      (a, b) =>
        a.deviceDeliverySequence - b.deviceDeliverySequence ||
        a.deviceDeliveryId.localeCompare(b.deviceDeliveryId),
    )
    .map((delivery) => pendingDeliveryMessage(delivery, acpOriginRunner));
}

function buildMessageIdentityIndex(messages: Message[]) {
  const userTurnIds = new Set<string>();
  const responseTurnIds = new Set<string>();
  const positionsById = new Map<string, number[]>();

  for (const [position, message] of messages.entries()) {
    const id = messageId(message);
    if (id) {
      const positions = positionsById.get(id);
      if (positions) positions.push(position);
      else positionsById.set(id, [position]);
    }
    const turn = turnId(message);
    if (!turn) continue;
    if (message.role === "user") {
      userTurnIds.add(turn);
    } else {
      responseTurnIds.add(turn);
    }
  }

  return { userTurnIds, responseTurnIds, positionsById };
}

export function streamMessages(
  fold: ConversationStreamFold,
  acpOriginRunner?: string | null,
  activeTurnId?: string | null,
): Message[] {
  return fold.output.filter(isMessage).map((msg, index) =>
    withTokenUsage(
      withAcpOrigin(
        {
          ...msg,
          role: msg.role,
          type: msg.type,
          content: msg.content,
          ...(turnId(msg) || !activeTurnId ? {} : { turn_id: activeTurnId }),
        },
        acpOriginRunner,
      ),
      fold,
      index,
    ),
  );
}

export function liveRunMessages(
  user: Message | null,
  fold: ConversationStreamFold | null,
  acpOriginRunner?: string | null,
  controlTurnId?: string | null,
): Message[] {
  const activeTurnId =
    fold?.turnId ??
    fold?.terminalTurnId ??
    (user ? turnId(user) : null) ??
    controlTurnId ??
    null;
  const effectiveAcpOriginRunner =
    acpRunnerFromMessage(user) ?? acpOriginRunner;
  return [
    ...(user ? [withAcpOrigin(user, effectiveAcpOriginRunner)] : []),
    ...(fold
      ? streamMessages(fold, effectiveAcpOriginRunner, activeTurnId)
      : []),
  ];
}

export function liveMessageCoveredByHistory(
  message: Message,
  history: Message[],
): boolean {
  const id = messageId(message);
  const turn = turnId(message);
  return history.some(
    (candidate) =>
      (id && messageId(candidate) === id) ||
      (turn &&
        turnId(candidate) === turn &&
        (message.type === "turn_aborted" ||
          message.type === "turn_interjected") &&
        candidate.type === message.type),
  );
}

export function mergeLiveMessagesIntoHistory(
  base: Message[],
  live: Message[],
): Message[] {
  if (!live.length) return base;
  const ends = new Map<string, number>();
  base.forEach((message, index) => {
    const turn = turnId(message);
    if (turn) ends.set(turn, index);
  });
  const insertions = new Map<number, Message[]>();
  const tail: Message[] = [];
  for (const message of live) {
    const turn = turnId(message);
    const end = turn ? ends.get(turn) : undefined;
    if (end === undefined) tail.push(message);
    else insertions.set(end, [...(insertions.get(end) || []), message]);
  }
  return base
    .flatMap((message, index) => [message, ...(insertions.get(index) || [])])
    .concat(tail);
}

export function buildConversationRenderHistory(params: {
  serverHistory: ChatHistory | null;
  pendingDeliveries: RemotePendingDelivery[];
  activeRun: ActiveRunOverlay;
}): ChatHistory | null {
  const source = params.serverHistory?.messages;
  const indexed = indexHistoryMessages(
    Array.isArray(source) ? source : EMPTY_MESSAGES,
  );
  const base = params.serverHistory
    ? { ...params.serverHistory, messages: indexed.messages }
    : null;
  const fold = params.activeRun.fold;
  const activeRunning =
    Boolean(params.activeRun.user || (fold && fold.output.length > 0)) &&
    !fold?.sawTerminal;
  const liveRun = liveRunMessages(
    params.activeRun.user,
    fold,
    params.activeRun.acpOriginRunner,
    base?.active_turn?.turn_id ?? base?.control?.active_turn?.turn_id ?? null,
  );
  let baseMessages = indexed.messages;
  let identityChanged = false;
  if (activeRunning) {
    for (const message of liveRun) {
      if (message.role === "user") continue;
      const id = messageId(message);
      const positions = id ? indexed.identity.positionsById.get(id) : undefined;
      if (!positions) continue;
      if (baseMessages === indexed.messages)
        baseMessages = baseMessages.slice();
      for (const position of positions) {
        const previous = baseMessages[position];
        if (
          previous.role !== message.role ||
          turnId(previous) !== turnId(message)
        ) {
          identityChanged = true;
        }
        baseMessages[position] = message;
      }
    }
  }
  const remotePendingMessages = pendingDeliveryMessages(
    base,
    params.pendingDeliveries,
    params.activeRun.user,
    params.activeRun.acpOriginRunner,
  );
  const covered = identityChanged
    ? buildMessageIdentityIndex(baseMessages)
    : indexed.identity;
  const pendingCovered = buildMessageIdentityIndex(remotePendingMessages);
  const liveMessages = liveRun.filter((message) => {
    const id = messageId(message);
    if (
      id &&
      (covered.positionsById.has(id) || pendingCovered.positionsById.has(id))
    )
      return false;

    const turn = turnId(message);
    if (!turn) return true;

    if (message.role === "user") {
      return (
        !covered.userTurnIds.has(turn) && !pendingCovered.userTurnIds.has(turn)
      );
    }
    if (
      message.type === "turn_aborted" ||
      message.type === "turn_interjected"
    ) {
      return !liveMessageCoveredByHistory(message, baseMessages);
    }
    return true;
  });
  const messages =
    remotePendingMessages.length || liveMessages.length
      ? mergeLiveMessagesIntoHistory(
          [...baseMessages, ...remotePendingMessages],
          liveMessages,
        )
      : baseMessages;
  if (messages.length === 0 && !base) return null;
  const terminalMatchesActive = Boolean(
    fold?.sawTerminal &&
      fold.terminalTurnId &&
      fold.terminalTurnId ===
        (base?.active_turn?.turn_id ?? base?.control?.active_turn?.turn_id),
  );
  const status = terminalMatchesActive
    ? "idle"
    : base?.control
    ? base.control.active_turn
      ? "running"
      : "idle"
    : base?.status || (activeRunning ? "running" : "idle");
  return {
    ...(base || {}),
    status,
    ...(terminalMatchesActive
      ? {
          active_turn: null,
          ...(base?.control
            ? { control: { ...base.control, active_turn: null } }
            : {}),
        }
      : {}),
    messages,
  };
}
