import { useEffect } from "react";
import { useLocation } from "react-router-dom";
import type { ConversationAddress } from "@/pages/Chat/conversationAddress";
import {
  startConversationActivity,
  setViewedConversation,
} from "./conversationActivity";

export function useNavigationActivity() {
  const { pathname } = useLocation();
  useEffect(() => startConversationActivity(), []);
  useEffect(() => {
    const match = /^\/(agents|projects)\/([^/]+)\/chats\/([^/]+)$/.exec(
      pathname,
    );
    let address: ConversationAddress | null = null;
    if (match) {
      try {
        address =
          match[1] === "agents"
            ? {
                scope: "agent",
                agentId: decodeURIComponent(match[2]),
                chatId: decodeURIComponent(match[3]),
              }
            : {
                scope: "project",
                projectId: decodeURIComponent(match[2]),
                chatId: decodeURIComponent(match[3]),
              };
      } catch {
        /* A malformed URL cannot identify a viewed conversation. */
      }
    }
    setViewedConversation(address);
    return () => setViewedConversation(null);
  }, [pathname]);
}
