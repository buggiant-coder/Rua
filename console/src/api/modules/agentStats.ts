import { request } from "../request";
import type { AgentStatsSummary } from "../types/agentStats";

export interface GetAgentStatsParams {
  start_date: string;
  end_date: string;
}

function buildQuery(params: GetAgentStatsParams): string {
  const search = new URLSearchParams({
    start_date: params.start_date,
    end_date: params.end_date,
  });
  return `?${search.toString()}`;
}

export const agentStatsApi = {
  getAgentStats: (params: GetAgentStatsParams) =>
    request<AgentStatsSummary>(`/agent-stats${buildQuery(params)}`),
};
