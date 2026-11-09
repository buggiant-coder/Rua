import type { ChatHistory, DurableConversationCoverage } from "@/api/types";
import {
  buildConversationRenderHistory,
  type ActiveRunOverlay,
  type RemotePendingDelivery,
} from "./renderState";

export type ConversationProjectionState = {
  history: ChatHistory | null;
  durableCoverage: DurableConversationCoverage | null;
};

export type ConversationProjectionInput = {
  serverHistory: ChatHistory | null;
  pendingDeliveries: RemotePendingDelivery[];
  activeRun: ActiveRunOverlay;
};

export function acceptsDurableHistory(
  previous: DurableConversationCoverage | null,
  candidate: DurableConversationCoverage | undefined,
): boolean {
  if (!previous || !candidate) return true;
  if (candidate.latest_message_sequence < previous.latest_message_sequence) {
    return false;
  }
  if (
    candidate.latest_message_sequence === previous.latest_message_sequence &&
    candidate.manifest_hash !== previous.manifest_hash
  ) {
    return false;
  }
  return true;
}

export function reduceConversationProjection(
  previous: ConversationProjectionState | null,
  input: ConversationProjectionInput,
): ConversationProjectionState {
  const previousCoverage = previous?.durableCoverage ?? null;
  const candidateCoverage = input.serverHistory?.durable_coverage;
  const acceptsCandidate = acceptsDurableHistory(
    previousCoverage,
    candidateCoverage,
  );
  const serverHistory = acceptsCandidate
    ? input.serverHistory
    : previous?.history ?? null;
  const durableCoverage = acceptsCandidate
    ? candidateCoverage ?? previousCoverage
    : previousCoverage;
  const history = buildConversationRenderHistory({ ...input, serverHistory });

  return {
    history:
      history && durableCoverage
        ? { ...history, durable_coverage: durableCoverage }
        : history,
    durableCoverage,
  };
}
