import { chatApi } from "@/api/modules/chat";
import type { ChatHistory } from "@/api/types";
import {
  INITIAL_CHAT_TURN_PAGE_SIZE,
  primeChatViewCache,
} from "./owned/useConversationViewRuntime";
import { resolveConversationRoute } from "./conversationRouteController";
import { saveDraftConversationId } from "./conversationLifecycle";

let preparationGeneration = 0;
let cancelActivePreparation: (() => void) | null = null;

export function cancelPreparedChatTransition() {
  preparationGeneration += 1;
  cancelActivePreparation?.();
  cancelActivePreparation = null;
}

export async function prepareChatTransition(
  agentId: string,
  chatId: string,
): Promise<boolean> {
  cancelActivePreparation?.();
  const generation = ++preparationGeneration;
  let resolveCancellation: ((ready: false) => void) | undefined;
  const cancellation = new Promise<false>((resolve) => {
    resolveCancellation = resolve;
  });
  cancelActivePreparation = () => resolveCancellation?.(false);

  const load = (async () => {
    let history: ChatHistory | null;
    try {
      const route = await resolveConversationRoute({
        scope: agentId,
        conversationId: chatId,
        loadHistory: () =>
          chatApi.getChat(chatId, agentId, {
            turnLimit: INITIAL_CHAT_TURN_PAGE_SIZE,
          }),
      });
      history = route.history;
      if (route.unmaterialized) saveDraftConversationId(agentId, chatId);
    } catch (err) {
      if (generation !== preparationGeneration) return false;
      throw err;
    }
    if (generation !== preparationGeneration) return false;

    if (history) primeChatViewCache(agentId, chatId, history);
    return true;
  })();
  try {
    return await Promise.race([load, cancellation]);
  } finally {
    if (generation === preparationGeneration) {
      cancelActivePreparation = null;
    }
  }
}
