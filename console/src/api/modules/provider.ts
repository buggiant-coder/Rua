import { request } from "../request";
import type {
  ProviderInfo,
  ProviderConfigRequest,
  ActiveModelsInfo,
  GetActiveModelsRequest,
  ModelSlotRequest,
  CreateCustomProviderRequest,
  AddModelRequest,
  ModelConfigRequest,
  TestConnectionResponse,
  TestProviderRequest,
  TestModelRequest,
  DiscoverModelsResponse,
  ProbeMultimodalResponse,
  SeriesResponse,
  DiscoverExtendedResponse,
  FilterModelsRequest,
  FilterModelsResponse,
} from "../types";

function buildActiveModelQuery(params?: GetActiveModelsRequest): string {
  if (!params?.scope && !params?.agent_id) {
    return "/models/active";
  }

  const searchParams = new URLSearchParams();
  if (params.scope) {
    searchParams.set("scope", params.scope);
  }
  if (params.agent_id) {
    searchParams.set("agent_id", params.agent_id);
  }

  return `/models/active?${searchParams.toString()}`;
}

export const providerApi = {
  listProviders: () => request<ProviderInfo[]>("/models"),

  configureProvider: (providerId: string, body: ProviderConfigRequest) =>
    request<ProviderInfo>(`/models/${encodeURIComponent(providerId)}/config`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  getActiveModels: (params?: GetActiveModelsRequest) =>
    request<ActiveModelsInfo>(buildActiveModelQuery(params)),

  setActiveLlm: (body: ModelSlotRequest) =>
    request<ActiveModelsInfo>("/models/active", {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  createCustomProvider: (body: CreateCustomProviderRequest) =>
    request<ProviderInfo>("/models/custom-providers", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  deleteProvider: (providerId: string) =>
    request<ProviderInfo[]>(
      `/models/providers/${encodeURIComponent(providerId)}`,
      { method: "DELETE" },
    ),

  listProviderTemplates: () => request<ProviderInfo[]>("/models/templates"),

  addModel: (providerId: string, body: AddModelRequest) =>
    request<ProviderInfo>(`/models/${encodeURIComponent(providerId)}/models`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  removeModel: (providerId: string, modelId: string) =>
    request<ProviderInfo>(
      `/models/${encodeURIComponent(providerId)}/models/delete`,
      {
        method: "POST",
        body: JSON.stringify({ model_id: modelId }),
      },
    ),

  configureModel: (
    providerId: string,
    modelId: string,
    body: ModelConfigRequest,
  ) =>
    request<ProviderInfo>(
      `/models/${encodeURIComponent(providerId)}/models/config`,
      {
        method: "PUT",
        body: JSON.stringify({ model_id: modelId, ...body }),
      },
    ),

  testProviderConnection: (providerId: string, body?: TestProviderRequest) =>
    request<TestConnectionResponse>(
      `/models/${encodeURIComponent(providerId)}/test`,
      {
        method: "POST",
        body: body ? JSON.stringify(body) : undefined,
      },
    ),

  testModelConnection: (providerId: string, body: TestModelRequest) =>
    request<TestConnectionResponse>(
      `/models/${encodeURIComponent(providerId)}/models/test`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  discoverModels: (providerId: string, body?: TestProviderRequest) =>
    request<DiscoverModelsResponse>(
      `/models/${encodeURIComponent(providerId)}/discover`,
      {
        method: "POST",
        body: body ? JSON.stringify(body) : undefined,
      },
    ),

  probeMultimodal: (providerId: string, modelId: string) =>
    request<ProbeMultimodalResponse>(
      `/models/${encodeURIComponent(providerId)}/models/probe-multimodal`,
      {
        method: "POST",
        body: JSON.stringify({ model_id: modelId }),
      },
    ),

  getOpenRouterSeries: () =>
    request<SeriesResponse>("/models/openrouter/series"),

  discoverOpenRouterExtended: (body?: TestProviderRequest) =>
    request<DiscoverExtendedResponse>("/models/openrouter/discover-extended", {
      method: "POST",
      body: body ? JSON.stringify(body) : undefined,
    }),

  filterOpenRouterModels: (body: FilterModelsRequest) =>
    request<FilterModelsResponse>("/models/openrouter/models/filter", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  startProviderOAuth: (providerId: string) =>
    request<{ authorize_url: string; state: string; flow_type: string }>(
      `/providers/${encodeURIComponent(providerId)}/oauth/start`,
      { method: "POST" },
    ),

  getProviderOAuthStatus: (providerId: string, state: string) =>
    request<{ status: "pending" | "completed" | "failed"; error?: string }>(
      `/providers/${encodeURIComponent(
        providerId,
      )}/oauth/status?state=${encodeURIComponent(state)}`,
    ),
};
