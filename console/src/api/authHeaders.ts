import { getApiToken } from "./config";
import { routeAgentId } from "@/routes/routeAgent";

export function getSelectedAgentId(): string {
  return routeAgentId();
}

export function buildAuthHeaders(): Record<string, string> {
  const headers: Record<string, string> = {};
  const token = getApiToken();
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }
  const agentId = getSelectedAgentId();
  if (agentId && agentId !== "default") {
    headers["X-Agent-Id"] = agentId;
  }
  return headers;
}
