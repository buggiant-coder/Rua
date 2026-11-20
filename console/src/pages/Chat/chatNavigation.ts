import { useAgentStore } from "@/stores/agentStore";
import type { ModelSlot } from "@/api/types";
import { rememberProjectId } from "../Projects/projectSelection";
import { saveDraftChatContext } from "./chatContext";
import {
  chatDraftScope,
  forgetConversation,
  lastConversationKey,
  loadDraftConversationId,
  loadLastConversationId,
  loadRecentConversationIds,
  recentConversationsKey,
  rememberConversation,
  resolveOrCreateDraftConversationId,
} from "./conversationLifecycle";

export function lastChatKey(agentId: string): string {
  return lastConversationKey(agentId);
}

export function recentChatKey(agentId: string): string {
  return recentConversationsKey(agentId);
}

export function loadLastChatId(agentId: string): string | null {
  return loadLastConversationId(agentId);
}

export function loadRecentChatIds(agentId: string): string[] {
  return loadRecentConversationIds(agentId);
}

export function saveLastChatId(agentId: string, chatId: string): void {
  rememberConversation(agentId, chatId);
}

export function forgetChatId(agentId: string, chatId: string): void {
  forgetConversation(agentId, chatId);
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

export function resolveChatEntry(agentId: string): string {
  return (
    loadLastChatId(agentId) ??
    loadRecentChatIds(agentId)[0] ??
    loadDraftConversationId(chatDraftScope()) ??
    openChatDraft()
  );
}
