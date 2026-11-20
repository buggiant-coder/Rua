import { useAgentStore } from "@/stores/agentStore";
import type { ModelSlot } from "@/api/types";
import { rememberProjectId } from "../Projects/projectSelection";
import { saveDraftChatContext } from "./chatContext";
import {
  chatDraftScope,
  forgetConversation,
  loadDraftConversationId,
  loadLastConversationId,
  loadRecentConversationIds,
  rememberConversation,
  resolveOrCreateDraftConversationId,
} from "./conversationLifecycle";

export function saveLastChatId(chatId: string): void {
  rememberConversation(chatDraftScope(), chatId);
}

export function forgetChatId(chatId: string): void {
  forgetConversation(chatDraftScope(), chatId);
}

export function createLazyChatId(): string {
  return crypto.randomUUID();
}

export function openChatDraft(
  projectId?: string,
  inheritedModel?: ModelSlot | null,
): string {
  const chatId = resolveOrCreateDraftConversationId(
    chatDraftScope(projectId),
    inheritedModel,
  );
  saveDraftChatContext(chatId, {
    agentId: useAgentStore.getState().selectedAgent,
    projectId,
  });
  if (projectId) rememberProjectId(projectId);
  return chatId;
}

export function resolveChatEntry(): string {
  return (
    loadLastConversationId(chatDraftScope()) ??
    loadRecentConversationIds(chatDraftScope())[0] ??
    loadDraftConversationId(chatDraftScope()) ??
    openChatDraft()
  );
}
