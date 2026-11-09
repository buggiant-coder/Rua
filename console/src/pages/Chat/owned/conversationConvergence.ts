import type {
  ChatHistory,
  ConversationControlView,
  Message,
} from "@/api/types";
import type { ConversationApiTarget } from "@/api/modules/conversation";
import {
  deviceDeliveryIdFromMessage,
  submissionIdFromMessage,
} from "./conversationLiveActivityViewer";
import { liveRunMessages, liveMessageCoveredByHistory } from "./renderState";
import { controlVersionIsOlder } from "./conversationStateConvergence";
import {
  acceptsDurableHistory,
  reduceConversationProjection,
  type ConversationProjectionInput,
  type ConversationProjectionState,
} from "./conversationProjectionReducer";
import {
  foldConversationStreamEvent,
  initialConversationStreamFold,
  type ConversationStreamEvent,
  type ConversationStreamFold,
} from "./stream";

function streamEventMatchesConversation(
  event: ConversationStreamEvent,
  target: ConversationApiTarget,
  chatId: string,
): boolean {
  if (!("raw" in event) || !event.raw || typeof event.raw !== "object") {
    return true;
  }
  const raw = event.raw as Record<string, unknown>;
  const address = raw.address;
  if (address && typeof address === "object" && !Array.isArray(address)) {
    const addressRecord = address as Record<string, unknown>;
    if (addressRecord.chat_id !== chatId) return false;
    const scope = addressRecord.scope;
    if (!scope || typeof scope !== "object" || Array.isArray(scope)) {
      return false;
    }
    const scopeRecord = scope as Record<string, unknown>;
    if (target.scope === "project") {
      return (
        scopeRecord.kind === "project" &&
        scopeRecord.project_id === target.projectId
      );
    }
    return (
      scopeRecord.kind === "agent" && scopeRecord.agent_id === target.agentId
    );
  }
  if (event.kind === "history_watermark_reached") {
    if (event.chatId && event.chatId !== chatId) return false;
    if (
      target.scope === "agent" &&
      event.agentId &&
      event.agentId !== target.agentId
    ) {
      return false;
    }
  }
  if (event.kind === "history_refresh_required" && event.chatId) {
    return event.chatId === chatId;
  }
  return true;
}

export type ConversationActivityCursor = {
  generation: string;
  sequence: number;
};

function conversationActivityPosition(
  event: ConversationStreamEvent,
): ConversationActivityCursor | null {
  if (!("raw" in event) || !event.raw || typeof event.raw !== "object") {
    return null;
  }
  const raw = event.raw as Record<string, unknown>;
  const generation = raw.activity_generation;
  const sequence = raw.activity_sequence;
  if (
    typeof generation !== "string" ||
    !generation ||
    typeof sequence !== "number" ||
    !Number.isSafeInteger(sequence) ||
    sequence < 1
  ) {
    return null;
  }
  return { generation, sequence };
}

function advanceConversationActivityCursor(
  cursor: ConversationActivityCursor | null,
  event: ConversationStreamEvent,
): {
  cursor: ConversationActivityCursor | null;
  accept: boolean;
} {
  const position = conversationActivityPosition(event);
  if (!position) return { cursor, accept: true };
  if (!cursor || cursor.generation !== position.generation) {
    return { cursor: position, accept: true };
  }
  if (position.sequence <= cursor.sequence) {
    return { cursor, accept: false };
  }
  return { cursor: position, accept: true };
}

function messageStringId(message: Message): string | null {
  return typeof message.id === "string" && message.id ? message.id : null;
}

function messageTurnId(message: Message | null): string | null {
  return typeof message?.turn_id === "string" && message.turn_id
    ? message.turn_id
    : null;
}

function matchingUserMessageIndex(
  messages: Message[],
  activeUser: Message,
): number {
  const activeSubmissionId = submissionIdFromMessage(activeUser);
  if (activeSubmissionId) {
    return messages.findIndex(
      (message) =>
        message.role === "user" &&
        submissionIdFromMessage(message) === activeSubmissionId,
    );
  }
  const activeId = messageStringId(activeUser);
  const activeDeviceDeliveryId = deviceDeliveryIdFromMessage(activeUser);
  const activeTurnId = messageTurnId(activeUser);
  if (activeId) {
    const idIndex = messages.findIndex(
      (message) =>
        message.role === "user" && messageStringId(message) === activeId,
    );
    if (idIndex >= 0) return idIndex;
  }
  if (activeDeviceDeliveryId) {
    return messages.findIndex(
      (message) =>
        message.role === "user" &&
        deviceDeliveryIdFromMessage(message) === activeDeviceDeliveryId,
    );
  }
  if (activeTurnId) {
    return messages.findIndex(
      (message) =>
        message.role === "user" && messageTurnId(message) === activeTurnId,
    );
  }
  return -1;
}

function responseMessagesAfterUser(messages: Message[], userIndex: number) {
  const response: Message[] = [];
  for (let index = userIndex + 1; index < messages.length; index += 1) {
    const message = messages[index];
    if (message.role === "user") break;
    response.push(message);
  }
  return response;
}

export function activeRunCoveredByHistory(
  history: ChatHistory | null,
  activeUser: Message | null,
  fold: ConversationStreamFold,
): boolean {
  const messages = history?.messages || [];
  if (messages.length === 0) return false;

  const responseMessages = (() => {
    if (!activeUser)
      return messages.filter((message) => message.role !== "user");
    const userIndex = matchingUserMessageIndex(messages, activeUser);
    return userIndex >= 0 ? responseMessagesAfterUser(messages, userIndex) : [];
  })();

  if (fold.output.length === 0) return responseMessages.length > 0;
  if (responseMessages.length === 0) return false;
  return liveRunMessages(activeUser, fold)
    .filter((message) => message.role !== "user")
    .every((message) => liveMessageCoveredByHistory(message, responseMessages));
}

export type CompactionStatus = { active: boolean; message?: string };
type Mutation = Extract<ConversationStreamEvent, { kind: "history_mutated" }>;

export type ActivityUpdate = {
  transitions: (
    | Mutation
    | Extract<ConversationStreamEvent, { kind: "conversation_control" }>
  )[];
  completedDeliveries: string[];
  refreshHistory: boolean;
  hydrationDevices: Set<string>;
  refreshApproval: boolean;
  event?: ConversationStreamEvent;
  fold?: ConversationStreamFold;
  compaction?: CompactionStatus | null;
  replay?: {
    fold: ConversationStreamFold;
    user: Message | null;
    compaction: CompactionStatus | null | undefined;
  };
};

function emptyUpdate(): ActivityUpdate {
  return {
    transitions: [],
    completedDeliveries: [],
    refreshHistory: false,
    hydrationDevices: new Set(),
    refreshApproval: false,
  };
}

/** One scoped view owns acceptance of history, controls and activity. No I/O. */
export class ConversationConvergence {
  readonly viewKey: string;

  constructor(viewKey: string) {
    this.viewKey = viewKey;
  }
  private historyVersion = 0;
  private coverage: ChatHistory["durable_coverage"];
  private control: ConversationControlView | null = null;
  private projection: ConversationProjectionState | null = null;

  beginHistoryRead(): { viewKey: string; version: number } {
    return { viewKey: this.viewKey, version: this.historyVersion };
  }

  acceptHistory(
    ticket: { viewKey: string; version: number },
    history: ChatHistory,
  ): boolean {
    if (
      ticket.viewKey !== this.viewKey ||
      ticket.version !== this.historyVersion ||
      !acceptsDurableHistory(this.coverage ?? null, history.durable_coverage)
    )
      return false;
    this.coverage = history.durable_coverage ?? this.coverage;
    return true;
  }

  invalidateHistory(): void {
    this.historyVersion += 1;
    this.coverage = undefined;
    this.projection = null;
  }

  acceptControl(control: ConversationControlView): boolean {
    if (
      this.control &&
      controlVersionIsOlder(
        control.placement?.authority_epoch ?? null,
        control.revision,
        this.control.placement?.authority_epoch ?? null,
        this.control.revision,
      )
    )
      return false;
    this.control = control;
    return true;
  }

  project(input: ConversationProjectionInput): ConversationProjectionState {
    this.projection = reduceConversationProjection(this.projection, input);
    if (
      acceptsDurableHistory(
        this.coverage ?? null,
        this.projection.durableCoverage ?? undefined,
      )
    ) {
      this.coverage = this.projection.durableCoverage ?? this.coverage;
    }
    return this.projection;
  }

  openActivity(
    target: ConversationApiTarget,
    chatId: string,
    cursor: ConversationActivityCursor | null,
  ): ConversationActivity {
    return new ConversationActivity(target, chatId, cursor);
  }
}

/** Subscription-local replay staging; only complete batches advance the visible cursor. */
class ConversationActivity {
  private watermarks = new Set<string>();
  private pending: {
    fold: ConversationStreamFold;
    user: Message | null;
    compaction: CompactionStatus | null | undefined;
    cursor: ConversationActivityCursor | null;
    update: ActivityUpdate;
  } | null = null;

  private readonly target: ConversationApiTarget;
  private readonly chatId: string;
  cursor: ConversationActivityCursor | null;

  constructor(
    target: ConversationApiTarget,
    chatId: string,
    cursor: ConversationActivityCursor | null,
  ) {
    this.target = target;
    this.chatId = chatId;
    this.cursor = cursor;
  }

  consume(
    event: ConversationStreamEvent,
    current: {
      fold: ConversationStreamFold;
      user: Message | null;
      hasOverlay: boolean;
      currentDeviceId: string | null | undefined;
    },
  ): ActivityUpdate | null {
    const replay = event.kind === "stream_replay";
    if (replay && !this.pending) {
      const checkpoint = event.events.some(
        (frame) =>
          "raw" in frame &&
          typeof frame.raw === "object" &&
          frame.raw?.activity_checkpoint === true,
      );
      this.pending = {
        fold: checkpoint ? initialConversationStreamFold : current.fold,
        user: null,
        compaction: undefined,
        cursor: checkpoint ? null : this.cursor,
        update: emptyUpdate(),
      };
    }
    const update = this.pending?.update ?? emptyUpdate();
    for (const frame of replay ? event.events : [event]) {
      if (!streamEventMatchesConversation(frame, this.target, this.chatId))
        continue;
      const activity = advanceConversationActivityCursor(
        this.pending ? this.pending.cursor : this.cursor,
        frame,
      );
      if (this.pending) this.pending.cursor = activity.cursor;
      else this.cursor = activity.cursor;
      if (!activity.accept) continue;
      switch (frame.kind) {
        case "conversation_control":
          update.transitions.push(frame);
          if (!frame.control.active_turn && current.hasOverlay)
            update.refreshHistory = true;
          continue;
        case "history_watermark_reached": {
          const key = `${frame.durableSequence}:${frame.manifestHash}`;
          if (!this.watermarks.has(key)) {
            this.watermarks.add(key);
            update.refreshHistory = true;
            if (
              frame.authorityDeviceId &&
              frame.authorityDeviceId !== current.currentDeviceId
            )
              update.hydrationDevices.add(frame.authorityDeviceId);
          }
          continue;
        }
        case "history_refresh_required":
          update.refreshHistory = true;
          continue;
        case "history_mutated":
          update.transitions.push(frame);
          if (frame.action === "rolled_back") {
            update.refreshHistory = true;
            if (this.pending) {
              this.pending.fold = initialConversationStreamFold;
              this.pending.user = null;
              this.pending.compaction = null;
              update.compaction = null;
              update.refreshApproval = false;
            }
          }
          continue;
        case "parse_error":
          update.refreshHistory = true;
          break;
        case "turn_terminal":
          update.refreshHistory = true;
          if (frame.deviceDeliveryId)
            update.completedDeliveries.push(frame.deviceDeliveryId);
          break;
      }
      if (
        frame.kind === "message_updated" &&
        frame.message.type === "tool_approval_request"
      )
        update.refreshApproval = true;
      if (frame.kind === "context_compaction") {
        update.compaction =
          frame.status === "started"
            ? { active: true, message: frame.message }
            : null;
      }
      const user = this.pending ? this.pending.user : current.user;
      const fold = this.pending ? this.pending.fold : current.fold;
      const nextFold =
        frame.kind === "user_message"
          ? initialConversationStreamFold
          : foldConversationStreamEvent(
              { ...fold, turnId: fold.turnId ?? messageTurnId(user) },
              frame,
            );
      if (this.pending) {
        this.pending.fold = nextFold;
        if (frame.kind === "user_message") this.pending.user = frame.message;
        if (update.compaction !== undefined)
          this.pending.compaction = update.compaction;
      } else {
        update.event = frame;
        update.fold = nextFold;
      }
    }
    if (replay) {
      if (!event.complete) return null;
      const pending = this.pending!;
      this.cursor = pending.cursor;
      update.replay = {
        fold: pending.fold,
        user: pending.user,
        compaction: pending.compaction,
      };
      this.pending = null;
    }
    return update;
  }
}
