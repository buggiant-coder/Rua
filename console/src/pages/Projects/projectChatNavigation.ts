import { openChatDraft } from "../Chat/chatNavigation";
import { chatPath } from "../Chat/conversationAddress";
import { readDraftChatContext } from "../Chat/chatContext";
import * as drafts from "../Chat/drafts";
import {
  forgetRememberedProject,
  lastProjectId as loadLastProjectId,
  rememberProjectId,
} from "./projectSelection";
import {
  chatDraftScope,
  clearConversationScope,
  clearDraftConversation,
  draftConversationModelKey,
  forgetConversation,
  loadLastConversationId,
  materializeConversation,
  rememberConversation,
  loadDraftConversationId,
} from "../Chat/conversationLifecycle";

function scopedKey(prefix: string, projectId: string, chatId: string): string {
  return `${prefix}${projectId}:${chatId}`;
}

export function projectChatDraftKey(projectId: string, chatId: string): string {
  return `project:${projectId}:${chatId}`;
}

export function projectChatExecutionAgent(
  projectId: string,
  chatId: string,
): string {
  const context = readDraftChatContext(chatId);
  return context?.projectId === projectId ? context.agentId : "default";
}

const PROJECT_CHAT_DIRECTORY_PREFIX = "rua_project_chat_directory_";

export function clearProjectChatDraftState(
  projectId: string,
  chatId: string,
): void {
  try {
    sessionStorage.removeItem(
      scopedKey(PROJECT_CHAT_DIRECTORY_PREFIX, projectId, chatId),
    );
  } catch {}
  clearDraftConversation(chatDraftScope(projectId), chatId);
  drafts.clear(projectChatDraftKey(projectId, chatId));
}

export function lastProjectId(): string | null {
  return loadLastProjectId();
}

export function lastProjectChatId(projectId: string): string | null {
  return loadLastConversationId(chatDraftScope(projectId));
}

export function rememberProjectChat(projectId: string, chatId: string): void {
  rememberConversation(chatDraftScope(projectId), chatId);
  rememberProjectId(projectId);
}

export function materializeProjectChat(
  projectId: string,
  chatId: string,
): void {
  materializeConversation(chatDraftScope(projectId), chatId);
  rememberProjectId(projectId);
}

export async function resolveProjectChatPath(
  projectId: string,
): Promise<string> {
  const chatId =
    lastProjectChatId(projectId) ??
    loadDraftConversationId(chatDraftScope(projectId));
  if (chatId) {
    rememberProjectId(projectId);
    return chatPath(chatId);
  }
  return chatPath(openChatDraft(projectId));
}

export function discardProjectChatLocalState(
  projectId: string,
  chatId: string,
): void {
  clearProjectChatDraftState(projectId, chatId);
  forgetProjectChat(projectId, chatId);
}

export function discardProjectLocalState(projectId: string): void {
  drafts.clearByPrefix(projectChatDraftKey(projectId, ""));
  clearConversationScope(chatDraftScope(projectId));
  forgetRememberedProject(projectId);
  try {
    const prefixes = [
      `${PROJECT_CHAT_DIRECTORY_PREFIX}${projectId}:`,
      draftConversationModelKey(chatDraftScope(projectId), ""),
    ];
    const keys = Array.from({ length: sessionStorage.length }, (_, index) =>
      sessionStorage.key(index),
    ).filter((key): key is string => Boolean(key));
    for (const key of keys) {
      if (prefixes.some((prefix) => key.startsWith(prefix))) {
        sessionStorage.removeItem(key);
      }
    }
  } catch {}
}

export function forgetProjectChat(projectId: string, chatId: string): void {
  forgetConversation(chatDraftScope(projectId), chatId);
}
