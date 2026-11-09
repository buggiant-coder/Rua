import { request } from "../request";
import type {
  MCPClientInfo,
  MCPClientCreateRequest,
  MCPClientUpdateRequest,
  MCPOAuthStartRequest,
  MCPOAuthStartResponse,
  MCPOAuthStatusResponse,
  MCPToolInfo,
} from "../types";

export const mcpApi = {
  listMCPClients: () => request<MCPClientInfo[]>("/mcp"),

  getMCPClient: (clientKey: string) =>
    request<MCPClientInfo>(`/mcp/${encodeURIComponent(clientKey)}`),

  createMCPClient: (body: MCPClientCreateRequest) =>
    request<MCPClientInfo>("/mcp", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  updateMCPClient: (clientKey: string, body: MCPClientUpdateRequest) =>
    request<MCPClientInfo>(`/mcp/${encodeURIComponent(clientKey)}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  toggleMCPClient: (clientKey: string) =>
    request<MCPClientInfo>(`/mcp/${encodeURIComponent(clientKey)}/toggle`, {
      method: "PATCH",
    }),

  deleteMCPClient: (clientKey: string) =>
    request<{ message: string }>(`/mcp/${encodeURIComponent(clientKey)}`, {
      method: "DELETE",
    }),

  listMCPClientTools: (clientKey: string) =>
    request<MCPToolInfo[]>(`/mcp/${encodeURIComponent(clientKey)}/tools`),

  toggleMCPClientTool: (
    clientKey: string,
    toolName: string,
    enabled: boolean,
  ) =>
    request<MCPClientInfo>(
      `/mcp/${encodeURIComponent(clientKey)}/tool-toggle/${encodeURIComponent(
        toolName,
      )}`,
      {
        method: "PATCH",
        body: JSON.stringify({ enabled }),
      },
    ),

  startOAuth: (clientKey: string, body: MCPOAuthStartRequest) =>
    request<MCPOAuthStartResponse>(
      `/mcp/oauth/start/${encodeURIComponent(clientKey)}`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  getOAuthStatus: (clientKey: string) =>
    request<MCPOAuthStatusResponse>(
      `/mcp/oauth/status/${encodeURIComponent(clientKey)}`,
    ),

  revokeOAuth: (clientKey: string) =>
    request<{ message: string }>(
      `/mcp/oauth/${encodeURIComponent(clientKey)}`,
      {
        method: "DELETE",
      },
    ),

  listStatus: () => request<McpStatusEntry[]>("/mcp/status"),
};

export interface McpStatusEntry {
  server_name: string;

  status:
    | "ready"
    | "connecting"
    | "failed"
    | "disabled"
    | "not_connected"
    | "degraded"
    | "stopping";
  last_error: string | null;
  auth?: McpAuthStatus;
}

export interface McpAuthStatus {
  kind: "none" | "oauth2" | "unknown";
  required: boolean;
  authorized: boolean;
  expired: boolean;
  expires_at: number;
  scope: string;
  client_id: string;
  reason?: string | null;
}
