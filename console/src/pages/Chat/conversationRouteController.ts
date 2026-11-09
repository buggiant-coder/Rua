import type { ChatHistory } from "@/api/types";
import {
  isDraftConversation,
  type ConversationScope,
} from "./conversationLifecycle";

export interface ConversationRouteResolution {
  history: ChatHistory | null;
  unmaterialized: boolean;
}

export function historyProvesConversationMaterialized(
  history: ChatHistory,
): boolean {
  return (
    history.messages.length > 0 ||
    history.active_turn != null ||
    Boolean(history.control?.placement?.authority_device_id) ||
    history.status === "running" ||
    history.meta?.project_chat_authority_placement != null
  );
}

export async function resolveConversationRoute({
  scope,
  conversationId,
  loadHistory,
}: {
  scope: ConversationScope;
  conversationId: string;
  loadHistory: () => Promise<ChatHistory>;
}): Promise<ConversationRouteResolution> {
  if (isDraftConversation(scope, conversationId)) {
    return { history: null, unmaterialized: true };
  }

  const history = await loadHistory();
  return {
    history,
    unmaterialized: !historyProvesConversationMaterialized(history),
  };
}
