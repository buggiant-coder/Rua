import { resolveChatEntry } from "./chatNavigation";

export type ConversationAddress =
  | { scope: "agent"; agentId: string; chatId: string }
  | { scope: "project"; projectId: string; chatId: string };

export function agentChatPath(agentId: string, chatId: string): string {
  return `/agents/${encodeURIComponent(agentId)}/chats/${encodeURIComponent(
    chatId,
  )}`;
}

export function projectChatPath(projectId: string, chatId: string): string {
  return `/projects/${encodeURIComponent(projectId)}/chats/${encodeURIComponent(
    chatId,
  )}`;
}

export function conversationPath(address: ConversationAddress): string {
  return address.scope === "agent"
    ? agentChatPath(address.agentId, address.chatId)
    : projectChatPath(address.projectId, address.chatId);
}

export function resolveAgentChatPath(agentId: string): string {
  return agentChatPath(agentId, resolveChatEntry(agentId));
}
