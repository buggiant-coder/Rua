import { useAgentStore } from "@/stores/agentStore";

let pendingAgentId: string | null = null;

export function beginRouteAgentPreparation(agentId: string) {
  pendingAgentId = agentId;
}

export function finishRouteAgentPreparation(agentId: string) {
  if (pendingAgentId === agentId) {
    pendingAgentId = null;
  }
}

export function routeAgentId(): string {
  return pendingAgentId ?? useAgentStore.getState().selectedAgent;
}
