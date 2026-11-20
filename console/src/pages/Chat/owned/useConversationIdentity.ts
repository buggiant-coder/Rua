import { useMemo } from "react";
import type { ConversationApiTarget } from "@/api/modules/conversation";
import type { ConversationAddress } from "../conversationAddress";

/** The route identifies the Chat; executor and Project are separate context. */
export function useConversationIdentity(
  address: ConversationAddress,
  context: ConversationApiTarget = {},
) {
  const { chatId } = address;
  const { agentId, projectId } = context;
  return useMemo(
    () => ({
      chatId,
      target: { agentId, projectId },
      ownerKey: "chat",
      viewKey: `chat:${chatId}`,
      agentId,
      projectId,
    }),
    [chatId, agentId, projectId],
  );
}
export type ConversationIdentity = ReturnType<typeof useConversationIdentity>;
