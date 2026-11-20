import { redirect, type LoaderFunctionArgs } from "react-router-dom";
import { resolveAgentChatPath } from "@/pages/Chat/conversationAddress";
import { useAgentStore } from "@/stores/agentStore";
import { lastChatPath } from "./lastChatRoute";
import { requireRouteAccess } from "./routeAuth";

export async function landingLoader({ request }: LoaderFunctionArgs) {
  await requireRouteAccess(request);
  const chatPath = lastChatPath();
  if (chatPath) {
    return redirect(chatPath);
  }
  const { selectedAgent } = useAgentStore.getState();
  return redirect(resolveAgentChatPath(selectedAgent || "default"));
}
