import type { ChatHistory, ConversationActiveTurn, Message } from "@/api/types";

function messageTurnId(message: Message | null | undefined): string | null {
  return typeof message?.turn_id === "string" && message.turn_id
    ? message.turn_id
    : null;
}

// These fields come from the durable Turn lifecycle, not Message completion.
// Regeneration may reuse a Turn id, so its later start must survive an old terminal.
function durableHistoryEndsActiveTurn(
  history: ChatHistory | null | undefined,
  activeTurn: ConversationActiveTurn | null | undefined,
): boolean {
  if (!activeTurn) return false;
  const startedAt = Date.parse(activeTurn.started_at);
  if (!Number.isFinite(startedAt)) return false;
  return Boolean(
    history?.messages.some((message) => {
      if (messageTurnId(message) !== activeTurn.turn_id) return false;
      if (
        !["completed", "interrupted", "interjected", "failed"].includes(
          String(message.turn_status),
        )
      )
        return false;
      if (typeof message.finished_at !== "string") return false;
      const finishedAt = Date.parse(message.finished_at);
      if (!Number.isFinite(finishedAt)) return false;
      if (finishedAt !== startedAt) return finishedAt > startedAt;
      // Date.parse truncates the sub-millisecond part of Rust RFC3339 timestamps.
      const [finishedFraction, startedFraction] = [
        message.finished_at,
        activeTurn.started_at,
      ].map((time) =>
        (time.match(/\.(\d+)(?:Z|[+-]\d{2}:\d{2})$/i)?.[1] ?? "")
          .padEnd(9, "0")
          .slice(3, 9),
      );
      return finishedFraction >= startedFraction;
    }),
  );
}

function reconcileDurableTerminal(history: ChatHistory): ChatHistory {
  const activeTurn = history.control
    ? history.control.active_turn
    : history.active_turn;
  if (!durableHistoryEndsActiveTurn(history, activeTurn)) return history;
  return {
    ...history,
    status: "idle",
    active_turn: null,
    ...(history.control
      ? { control: { ...history.control, active_turn: null } }
      : {}),
  };
}

export type TurnActivity =
  | { kind: "idle" }
  | {
      kind: "active";
      turnId: string | null;
      userMessageId: string | null;
      responseId: string | null;
    };

export function deriveTurnActivity(
  history: ChatHistory | null | undefined,
  liveActive = false,
): TurnActivity {
  const activeTurn = history?.control
    ? history.control.active_turn
    : history?.active_turn ?? null;
  if (durableHistoryEndsActiveTurn(history, activeTurn))
    return { kind: "idle" };
  if (!activeTurn) {
    return liveActive
      ? {
          kind: "active",
          turnId: null,
          userMessageId: null,
          responseId: null,
        }
      : { kind: "idle" };
  }

  const userMessage =
    history?.messages.find((message) => {
      if (message.role !== "user") return false;
      const turnId = messageTurnId(message);
      if (turnId) return turnId === activeTurn.turn_id;
      return Boolean(
        activeTurn.user_message_id && message.id === activeTurn.user_message_id,
      );
    }) ?? null;

  return {
    kind: "active",
    turnId: typeof activeTurn.turn_id === "string" ? activeTurn.turn_id : null,
    userMessageId:
      typeof activeTurn.user_message_id === "string"
        ? activeTurn.user_message_id
        : typeof userMessage?.id === "string"
        ? userMessage.id
        : null,
    responseId: null,
  };
}

export function controlVersionIsOlder(
  candidateEpoch: string | null,
  candidateRevision: number | undefined,
  currentEpoch: string | null,
  currentRevision: number | undefined,
): boolean {
  return (
    candidateEpoch === currentEpoch &&
    typeof candidateRevision === "number" &&
    typeof currentRevision === "number" &&
    candidateRevision < currentRevision
  );
}

export function reconcileStableConversationHistory(
  current: ChatHistory | null,
  stable: ChatHistory,
): ChatHistory {
  const mergedStable =
    stable.model === undefined && current?.model !== undefined
      ? { ...stable, model: current.model }
      : stable;
  const currentControl = current?.control;
  if (!currentControl) return reconcileDurableTerminal(mergedStable);
  const stableControl = mergedStable.control;
  if (
    stableControl &&
    !controlVersionIsOlder(
      stableControl.placement?.authority_epoch ?? null,
      stableControl.revision,
      currentControl.placement?.authority_epoch ?? null,
      currentControl.revision,
    )
  ) {
    return reconcileDurableTerminal({
      ...mergedStable,
      status: stableControl.active_turn ? "running" : "idle",
      active_turn: stableControl.active_turn ?? null,
    });
  }
  return reconcileDurableTerminal({
    ...mergedStable,
    status: currentControl.active_turn ? "running" : "idle",
    active_turn: currentControl.active_turn ?? null,
    control: currentControl,
  });
}
