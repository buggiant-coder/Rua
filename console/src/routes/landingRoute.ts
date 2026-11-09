import { redirect, type LoaderFunctionArgs } from "react-router-dom";
import { resolveAgentChatPath } from "@/pages/Chat/conversationAddress";
import { useAgentStore } from "@/stores/agentStore";
import { lastProjectChatPath } from "./lastChatRoute";
import { requireRouteAccess } from "./routeAuth";

export async function landingLoader({ request }: LoaderFunctionArgs) {
  await requireRouteAccess(request);
  const projectChatPath = lastProjectChatPath();
  if (projectChatPath) {
    return redirect(projectChatPath);
  }
  const { selectedAgent } = useAgentStore.getState();
  return redirect(resolveAgentChatPath(selectedAgent || "default"));
}
