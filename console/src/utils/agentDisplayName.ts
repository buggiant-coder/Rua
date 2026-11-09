import type { AgentSummary } from "../api/types/agents";

export const DEFAULT_AGENT_ID = "default";

export function getAgentDisplayName(
  agent: Pick<AgentSummary, "id" | "name">,
): string {
  return agent.name || agent.id;
}
