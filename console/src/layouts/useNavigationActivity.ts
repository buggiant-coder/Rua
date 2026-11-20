import { addressFromChatPath } from "@/pages/Chat/conversationAddress";
import { useEffect } from "react";
import { useLocation } from "react-router-dom";
import {
  startConversationActivity,
  setViewedConversation,
} from "./conversationActivity";

export function useNavigationActivity() {
  const { pathname } = useLocation();
  useEffect(() => startConversationActivity(), []);
  useEffect(() => {
    setViewedConversation(addressFromChatPath(pathname));
    return () => setViewedConversation(null);
  }, [pathname]);
}
