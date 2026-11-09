import { request } from "../request";
import type {
  MCPClientInfo,
  MCPClientCreateRequest,
  MCPClientUpdateRequest,
} from "../types";

export interface SharedMcpBroadcastResult {
  client_key: string;
  agent_id?: string;
  status: "created" | "overwritten" | "skipped" | "conflict" | "error";
  conflict?: boolean;
  error?: string;
}

export interface SharedMcpSyncResult {
  client_key: string;
  status: "created" | "overwritten" | "skipped" | "conflict" | "error";
  conflict?: boolean;
  error?: string;
}

export const sharedMcpApi = {
  list: () => request<MCPClientInfo[]>("/shared-mcp"),

  get: (clientKey: string) =>
    request<MCPClientInfo>(`/shared-mcp/${encodeURIComponent(clientKey)}`),

  create: (
    clientKey: string,
    client: Omit<MCPClientCreateRequest, "client_key">["client"],
  ) =>
    request<MCPClientInfo>("/shared-mcp", {
      method: "POST",
      body: JSON.stringify({ client_key: clientKey, client }),
    }),

  update: (clientKey: string, updates: MCPClientUpdateRequest) =>
    request<MCPClientInfo>(`/shared-mcp/${encodeURIComponent(clientKey)}`, {
      method: "PUT",
      body: JSON.stringify(updates),
    }),

  delete: (clientKey: string) =>
    request<{ message: string }>(
      `/shared-mcp/${encodeURIComponent(clientKey)}`,
      {
        method: "DELETE",
      },
    ),

  broadcast: (
    clientKeys: string[],
    agentIds: string[],
    conflictMode: string = "ask",
  ) =>
    request<{ results: SharedMcpBroadcastResult[] }>("/shared-mcp/broadcast", {
      method: "POST",
      body: JSON.stringify({
        client_keys: clientKeys,
        agent_ids: agentIds,
        conflict_mode: conflictMode,
      }),
    }),

  sync: (agentId: string, clientKeys: string[], conflictMode: string = "ask") =>
    request<{ results: SharedMcpSyncResult[] }>("/shared-mcp/sync", {
      method: "POST",
      body: JSON.stringify({
        agent_id: agentId,
        client_keys: clientKeys,
        conflict_mode: conflictMode,
      }),
    }),

  listAgentMcp: (agentId: string) =>
    request<MCPClientInfo[]>("/mcp", {
      headers: { "X-Agent-Id": agentId },
    }),
};
