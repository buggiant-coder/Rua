export interface MCPApplication {
  status: "applied" | "deferred" | "failed";
  saved: boolean;
  failed_servers: string[];
}

export interface MCPClientInfo {
  application?: MCPApplication;
  key: string;

  name: string;

  description: string;

  enabled: boolean;

  transport: "stdio" | "streamable_http" | "sse";

  url: string;

  headers: Record<string, string>;

  command: string;

  args: string[];

  env: Record<string, string>;

  cwd: string;

  oauth?: MCPOAuthConfig | null;

  disabled_tools?: string[];
}

export interface MCPOAuthConfig {
  client_id: string;
  scope: string;
  access_token: string;
  refresh_token: string;
  expires_at: number;
  token_endpoint: string;
  auth_endpoint: string;
}

export interface MCPOAuthStartRequest {
  url?: string;
  scope?: string;
  client_id?: string;
  auth_endpoint?: string;
  token_endpoint?: string;
}

export interface MCPOAuthStartResponse {
  auth_url: string;
  session_id: string;
}

export interface MCPOAuthStatusResponse {
  authorized: boolean;
  expires_at: number;
  scope: string;
  client_id: string;
}

export interface MCPToolInfo {
  id?: string;
  name: string;
  description: string;
  enabled?: boolean;
  capability?: string;
  capability_enabled?: boolean;
  schema?: unknown;
}

export interface MCPClientCreateRequest {
  client_key: string;

  client: {
    name: string;

    description?: string;

    enabled?: boolean;

    transport?: "stdio" | "streamable_http" | "sse";

    url?: string;

    headers?: Record<string, string>;

    command?: string;

    args?: string[];

    env?: Record<string, string>;

    cwd?: string;
    oauth?: MCPOAuthConfig | null;
    disabled_tools?: string[];
  };
}

export interface MCPClientUpdateRequest {
  name?: string;

  description?: string;

  enabled?: boolean;

  transport?: "stdio" | "streamable_http" | "sse";

  url?: string;

  headers?: Record<string, string>;

  command?: string;

  args?: string[];

  env?: Record<string, string>;

  cwd?: string;
  oauth?: MCPOAuthConfig | null;
  disabled_tools?: string[];
}
