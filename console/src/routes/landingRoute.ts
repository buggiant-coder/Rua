import { redirect, type LoaderFunctionArgs } from "react-router-dom";
import { resolveStandaloneChatPath } from "@/pages/Chat/conversationAddress";
import { lastChatPath } from "./lastChatRoute";
import { requireRouteAccess } from "./routeAuth";

export async function landingLoader({ request }: LoaderFunctionArgs) {
  await requireRouteAccess(request);
  const chatPath = lastChatPath();
  if (chatPath) {
    return redirect(chatPath);
  }
  return redirect(resolveStandaloneChatPath());
}
