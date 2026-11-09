import type { ChatHistory, Message } from "@/api/types";

function messageTurnId(message: Message | null | undefined): string | null {
  return typeof message?.turn_id === "string" && message.turn_id
    ? message.turn_id
    : null;
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
  if (!currentControl) return mergedStable;
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
    return {
      ...mergedStable,
      status: stableControl.active_turn ? "running" : "idle",
      active_turn: stableControl.active_turn ?? null,
    };
  }
  return {
    ...mergedStable,
    status: currentControl.active_turn ? "running" : "idle",
    active_turn: currentControl.active_turn ?? null,
    control: currentControl,
  };
}
