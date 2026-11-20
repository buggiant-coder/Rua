/** Browser draft options. A materialized Chat always uses its server header. */
export type ChatContext = { agentId: string; projectId?: string | null };
const PREFIX = "rua_chat_draft_context_";

export function readDraftChatContext(chatId: string): ChatContext | null {
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(`${PREFIX}${chatId}`) ?? "null",
    );
    if (
      value &&
      typeof value === "object" &&
      "agentId" in value &&
      typeof value.agentId === "string"
    ) {
      const projectId =
        "projectId" in value && typeof value.projectId === "string"
          ? value.projectId
          : null;
      return { agentId: value.agentId, projectId };
    }
  } catch {}
  return null;
}

export function saveDraftChatContext(
  chatId: string,
  context: ChatContext,
): void {
  try {
    localStorage.setItem(`${PREFIX}${chatId}`, JSON.stringify(context));
  } catch {}
}

export function clearDraftChatContext(chatId: string): void {
  try {
    localStorage.removeItem(`${PREFIX}${chatId}`);
  } catch {}
}
