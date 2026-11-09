import { request } from "../request";
import type { HubInstallTaskResponse } from "../types";

export interface MarketProviderInfo {
  key: string;
  label: string;
  available: boolean;
  reason: string | null;
}

export interface MarketResult {
  source: string;
  slug: string;
  name: string;
  description: string | null;
  source_url: string;
  version: string | null;
  author: string | null;
  icon_url: string | null;
  stats: Record<string, string | number> | null;
}

export interface MarketSearchError {
  provider: string;
  message: string;
}

export interface ProviderPageInfo {
  has_more: boolean;
  total: number;
}

export interface MarketSearchResponse {
  results: MarketResult[];
  errors: MarketSearchError[];
  by_provider: Record<string, ProviderPageInfo>;
}

export interface MarketSearchPayload {
  query: string;
  provider_pages: Record<string, number>;
  limit?: number;
  lang?: string;
}

export interface HubInstallStartPayload {
  bundle_url: string;
  version?: string;
  enable?: boolean;
  target_name?: string;
  installed_from?: string;
  market_meta?: Record<string, unknown>;
}

export interface PoolImportPayload {
  bundle_url: string;
  version?: string;
  target_name?: string;
  overwrite?: boolean;
  installed_from?: string;
  market_meta?: Record<string, unknown>;
}

export interface PoolImportResponse {
  installed: boolean;
  name: string;
  enabled: boolean;
  source_url: string;
}

export const marketApi = {
  listMarketProviders: () => request<MarketProviderInfo[]>("/market/providers"),

  searchMarket: (payload: MarketSearchPayload) =>
    request<MarketSearchResponse>("/market/search", {
      method: "POST",
      body: JSON.stringify(payload),
    }),

  startHubInstall: (payload: HubInstallStartPayload, agentId: string) =>
    request<HubInstallTaskResponse>(
      `/agents/${encodeURIComponent(agentId)}/skills/hub/install/start`,
      {
        method: "POST",
        body: JSON.stringify(payload),
      },
    ),

  getHubInstallStatus: (taskId: string, agentId: string) =>
    request<HubInstallTaskResponse>(
      `/agents/${encodeURIComponent(
        agentId,
      )}/skills/hub/install/status/${encodeURIComponent(taskId)}`,
    ),

  cancelHubInstall: (taskId: string, agentId: string) =>
    request<{ task_id: string; status: string }>(
      `/agents/${encodeURIComponent(
        agentId,
      )}/skills/hub/install/cancel/${encodeURIComponent(taskId)}`,
      { method: "POST" },
    ),

  importPoolFromHub: (payload: PoolImportPayload) =>
    request<PoolImportResponse>("/skills/pool/import", {
      method: "POST",
      body: JSON.stringify(payload),
    }),
};
