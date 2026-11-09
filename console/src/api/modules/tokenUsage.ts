import { request } from "../request";
import type { TokenUsageRecord, TokenUsageSummary } from "../types/tokenUsage";

export interface GetTokenUsageParams {
  start_date: string;
  end_date: string;
  model?: string;
  provider?: string;
  agent_id?: string;
  chat_id?: string;
}

function buildQuery(params: GetTokenUsageParams): string {
  const search = new URLSearchParams({
    start_date: params.start_date,
    end_date: params.end_date,
  });
  if (params.model) search.set("model", params.model);
  if (params.provider) search.set("provider", params.provider);
  if (params.agent_id) search.set("agent_id", params.agent_id);
  if (params.chat_id) search.set("chat_id", params.chat_id);
  return `?${search.toString()}`;
}

export const tokenUsageApi = {
  getTokenUsage: (params: GetTokenUsageParams) =>
    request<TokenUsageSummary>(`/token-usage${buildQuery(params)}`),
  getTokenUsageDetails: (params: GetTokenUsageParams) =>
    request<TokenUsageRecord[]>(`/token-usage/details${buildQuery(params)}`),
};
