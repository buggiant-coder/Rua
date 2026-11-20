import { chatPath } from "../Chat/conversationAddress";
import {
  readDraftChatContext,
  saveDraftChatContext,
} from "../Chat/chatContext";
import * as drafts from "../Chat/drafts";
import {
  forgetRememberedProject,
  lastProjectId as loadLastProjectId,
  rememberProjectId,
} from "./projectSelection";
import {
  clearConversationScope,
  clearDraftConversation,
  draftConversationModelKey,
  forgetConversation,
  loadLastConversationId,
  materializeConversation,
  rememberConversation,
  resolveOrCreateDraftConversationId,
} from "../Chat/conversationLifecycle";

const PROJECT_RECENT_AGENT_PREFIX = "rua_project_recent_agent_";

function scopedKey(prefix: string, projectId: string, chatId: string): string {
  return `${prefix}${projectId}:${chatId}`;
}

export function projectChatDraftKey(projectId: string, chatId: string): string {
  return `project:${projectId}:${chatId}`;
}

export function projectConversationScope(projectId: string): string {
  return `project:${projectId}`;
}

export function createProjectChat(
  projectId: string,
  agentId = projectLastSelectedAgent(projectId) || "default",
): string {
  const scope = projectConversationScope(projectId);
  const chatId = resolveOrCreateDraftConversationId(scope);
  if (!readDraftChatContext(chatId))
    setProjectChatDraftAgent(projectId, chatId, agentId);
  rememberProjectId(projectId);
  return chatId;
}

export function projectChatExecutionAgent(
  projectId: string,
  chatId: string,
): string {
  const context = readDraftChatContext(chatId);
  return context?.projectId === projectId ? context.agentId : "default";
}

export function setProjectChatDraftAgent(
  projectId: string,
  chatId: string,
  agentId: string,
): void {
  saveDraftChatContext(chatId, { agentId, projectId });
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
  clearDraftConversation(projectConversationScope(projectId), chatId);
  drafts.clear(projectChatDraftKey(projectId, chatId));
}

export function projectLastSelectedAgent(projectId: string): string | null {
  try {
    const value = localStorage.getItem(
      `${PROJECT_RECENT_AGENT_PREFIX}${projectId}`,
    );
    return value?.trim() || null;
  } catch {
    return null;
  }
}

export function setProjectLastSelectedAgent(
  projectId: string,
  agentId: string,
): void {
  try {
    localStorage.setItem(`${PROJECT_RECENT_AGENT_PREFIX}${projectId}`, agentId);
  } catch {}
}

export function lastProjectId(): string | null {
  return loadLastProjectId();
}

export function lastProjectChatId(projectId: string): string | null {
  return loadLastConversationId(projectConversationScope(projectId));
}

export function rememberProjectChat(projectId: string, chatId: string): void {
  rememberConversation(projectConversationScope(projectId), chatId);
  rememberProjectId(projectId);
}

export function materializeProjectChat(
  projectId: string,
  chatId: string,
): void {
  materializeConversation(projectConversationScope(projectId), chatId);
  rememberProjectId(projectId);
}

export async function resolveProjectChatPath(
  projectId: string,
): Promise<string> {
  const hintedChatId = lastProjectChatId(projectId);
  if (hintedChatId) {
    return chatPath(hintedChatId);
  }

  return chatPath(createProjectChat(projectId));
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
  clearConversationScope(projectConversationScope(projectId));
  forgetRememberedProject(projectId);
  try {
    localStorage.removeItem(`${PROJECT_RECENT_AGENT_PREFIX}${projectId}`);
  } catch {}
  try {
    const prefixes = [
      `${PROJECT_CHAT_DIRECTORY_PREFIX}${projectId}:`,
      draftConversationModelKey(projectConversationScope(projectId), ""),
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
  forgetConversation(projectConversationScope(projectId), chatId);
}
