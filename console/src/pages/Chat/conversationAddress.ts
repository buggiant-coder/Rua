import { resolveChatEntry } from "./chatNavigation";

export type ConversationAddress = { chatId: string };

export function chatPath(chatId: string): string {
  return `/chats/${encodeURIComponent(chatId)}`;
}

export function conversationPath(address: ConversationAddress): string {
  return chatPath(address.chatId);
}

export function addressFromChatPath(
  pathname: string,
): ConversationAddress | null {
  const match = /^\/chats\/([^/]+)\/?$/.exec(pathname);
  if (!match) return null;
  try {
    return { chatId: decodeURIComponent(match[1]) };
  } catch {
    return null;
  }
}

export function resolveAgentChatPath(agentId: string): string {
  return chatPath(resolveChatEntry(agentId));
}
