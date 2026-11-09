import { useMemo } from "react";
import type { ConversationApiTarget } from "@/api/modules/conversation";
import type { ChatMediaScope } from "@/api/modules/chat";
import type { ConversationAddress } from "../conversationAddress";

/** All view identity comes from the complete route, never the execution Agent. */
export function useConversationIdentity(address: ConversationAddress) {
  const { scope, chatId } = address;
  const ownerId = scope === "agent" ? address.agentId : address.projectId;
  return useMemo(() => {
    const target: ConversationApiTarget =
      scope === "agent"
        ? { scope, agentId: ownerId }
        : { scope, projectId: ownerId };
    const mediaScope: ChatMediaScope =
      scope === "agent"
        ? { kind: "agent", agentId: ownerId }
        : { kind: "project", projectId: ownerId };
    // Preserve the existing browser draft and view-cache namespace.
    const ownerKey = scope === "agent" ? ownerId : `project:${ownerId}`;
    return {
      chatId,
      target,
      mediaScope,
      ownerKey,
      viewKey: `${ownerKey}:${chatId}`,
      agentId: scope === "agent" ? ownerId : undefined,
      projectId: scope === "project" ? ownerId : undefined,
    };
  }, [scope, ownerId, chatId]);
}

export type ConversationIdentity = ReturnType<typeof useConversationIdentity>;
