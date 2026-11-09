import type { ModelSlot } from "@/api/types";
import {
  clearDraftConversation,
  clearDraftConversationModel,
  draftConversationKey,
  draftConversationModelKey,
  forgetConversation,
  lastConversationKey,
  loadDraftConversationId,
  loadDraftConversationModel,
  loadLastConversationId,
  loadRecentConversationIds,
  recentConversationsKey,
  rememberConversation,
  resolveConversationEntry,
  resolveOrCreateDraftConversationId,
  saveDraftConversationId,
  saveDraftConversationModel,
} from "./conversationLifecycle";

export function lastChatKey(agentId: string): string {
  return lastConversationKey(agentId);
}

export function recentChatKey(agentId: string): string {
  return recentConversationsKey(agentId);
}

export function draftChatKey(agentId: string): string {
  return draftConversationKey(agentId);
}

export function draftChatModelKey(agentId: string, chatId: string): string {
  return draftConversationModelKey(agentId, chatId);
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

export function loadDraftChatId(agentId: string): string | null {
  return loadDraftConversationId(agentId);
}

export function saveDraftChatId(agentId: string, chatId: string): void {
  saveDraftConversationId(agentId, chatId);
}

export function clearDraftChatId(agentId: string, chatId?: string): void {
  clearDraftConversation(agentId, chatId);
}

export function loadDraftChatModel(
  agentId: string,
  chatId: string,
): ModelSlot | null {
  return loadDraftConversationModel(agentId, chatId);
}

export function saveDraftChatModel(
  agentId: string,
  chatId: string,
  model: ModelSlot,
): void {
  saveDraftConversationModel(agentId, chatId, model);
}

export function clearDraftChatModel(agentId: string, chatId: string): void {
  clearDraftConversationModel(agentId, chatId);
}

export function resolveOrCreateDraftChatId(agentId: string): string {
  return resolveOrCreateDraftConversationId(agentId);
}

export function resolveChatEntry(agentId: string): string {
  return resolveConversationEntry(agentId);
}
