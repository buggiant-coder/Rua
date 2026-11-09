import { request } from "../request";
import { getApiUrl } from "../config";
import { buildAuthHeaders } from "../authHeaders";
import type {
  AgentListResponse,
  AgentProfileConfig,
  CreateAgentRequest,
  AgentProfileRef,
  ReorderAgentsResponse,
} from "../types/agents";
import type { MdFileInfo, MdFileContent, SubdirFile } from "../types/workspace";
import type { ToolConfig, ToolInfo } from "./tools";
import type { AgentsRunningConfig, ChatRunningConfig } from "../types/agent";
import type {
  MCPClientInfo,
  MCPClientCreateRequest,
  MCPClientUpdateRequest,
  MCPOAuthStartRequest,
  MCPOAuthStartResponse,
  MCPOAuthStatusResponse,
  MCPToolInfo,
} from "../types/mcp";
import type { SkillSpec } from "../types/skill";

export const agentsApi = {
  listAgents: () => request<AgentListResponse>("/agents"),

  getAgent: (agentId: string) =>
    request<AgentProfileConfig>(`/agents/${agentId}`),

  createAgent: (agent: CreateAgentRequest) =>
    request<AgentProfileRef>("/agents", {
      method: "POST",
      body: JSON.stringify(agent),
    }),

  updateAgent: (agentId: string, agent: AgentProfileConfig) =>
    request<AgentProfileConfig>(`/agents/${agentId}`, {
      method: "PUT",
      body: JSON.stringify(agent),
    }),

  deleteAgent: (agentId: string) =>
    request<{ success: boolean; agent_id: string }>(`/agents/${agentId}`, {
      method: "DELETE",
    }),

  reorderAgents: (agentIds: string[]) =>
    request<ReorderAgentsResponse>("/agents/order", {
      method: "PUT",
      body: JSON.stringify({ agent_ids: agentIds }),
    }),

  renameAgent: (agentId: string, newId: string) =>
    request<AgentProfileRef>("/agents/rename", {
      method: "POST",
      body: JSON.stringify({ old_id: agentId, new_id: newId }),
    }),

  toggleAgentEnabled: (agentId: string, enabled: boolean) =>
    request<{ success: boolean; agent_id: string; enabled: boolean }>(
      `/agents/${agentId}/toggle`,
      {
        method: "PATCH",
        body: JSON.stringify({ enabled }),
      },
    ),

  listAgentFiles: (agentId: string) =>
    request<MdFileInfo[]>(`/agents/${agentId}/files`),

  readAgentFile: (agentId: string, filename: string) =>
    request<MdFileContent>(
      `/agents/${agentId}/files/${encodeURIComponent(filename)}`,
    ),

  writeAgentFile: (agentId: string, filename: string, content: string) =>
    request<{ written: boolean; filename: string }>(
      `/agents/${agentId}/files/${encodeURIComponent(filename)}`,
      {
        method: "PUT",
        body: JSON.stringify({ content }),
      },
    ),

  getSubdirsInfo: (agentId: string) =>
    request<Record<string, number>>(`/agents/${agentId}/subdirs-info`),

  listSubdirFiles: (agentId: string, subdir: string) =>
    request<MdFileInfo[]>(
      `/agents/${agentId}/subdirs/${encodeURIComponent(subdir)}`,
    ).then(
      (files) =>
        files.map((file) => ({
          ...file,
          subdir,
          updated_at: new Date(file.modified_time).getTime(),
        })) as SubdirFile[],
    ),

  createSubdirFile: (agentId: string, subdir: string, filename = "new.md") =>
    request<MdFileInfo>(
      `/agents/${agentId}/subdirs/${encodeURIComponent(subdir)}`,
      {
        method: "POST",
        body: JSON.stringify({ filename }),
      },
    ),

  readSubdirFile: (agentId: string, subdir: string, filename: string) =>
    request<MdFileContent>(
      `/agents/${agentId}/subdirs/${encodeURIComponent(
        subdir,
      )}/${encodeURIComponent(filename)}`,
    ),

  writeSubdirFile: (
    agentId: string,
    subdir: string,
    filename: string,
    content: string,
  ) =>
    request<Record<string, unknown>>(
      `/agents/${agentId}/subdirs/${encodeURIComponent(
        subdir,
      )}/${encodeURIComponent(filename)}`,
      {
        method: "PUT",
        body: JSON.stringify({ content }),
      },
    ),

  getSystemPromptFiles: (agentId: string) =>
    request<string[]>(`/agents/${agentId}/system-prompt-files`),

  setSystemPromptFiles: (agentId: string, files: string[]) =>
    request<string[]>(`/agents/${agentId}/system-prompt-files`, {
      method: "PUT",
      body: JSON.stringify(files),
    }),

  listAgentTools: (agentId: string) =>
    request<ToolInfo[]>(`/agents/${encodeURIComponent(agentId)}/tools`),

  toggleAgentTool: (agentId: string, toolName: string) =>
    request<ToolInfo>(
      `/agents/${encodeURIComponent(agentId)}/tools/${encodeURIComponent(
        toolName,
      )}/toggle`,
      { method: "PATCH" },
    ),

  updateAgentToolAsyncExecution: (
    agentId: string,
    toolName: string,
    asyncExecution: boolean,
  ) =>
    request<ToolInfo>(
      `/agents/${encodeURIComponent(agentId)}/tools/${encodeURIComponent(
        toolName,
      )}/async-execution`,
      {
        method: "PATCH",
        body: JSON.stringify({ async_execution: asyncExecution }),
      },
    ),

  getAgentToolConfig: (agentId: string, toolName: string) =>
    request<ToolConfig>(
      `/agents/${encodeURIComponent(agentId)}/tools/${encodeURIComponent(
        toolName,
      )}/config`,
    ),

  updateAgentToolConfig: (
    agentId: string,
    toolName: string,
    config: ToolConfig,
  ) =>
    request<ToolConfig>(
      `/agents/${encodeURIComponent(agentId)}/tools/${encodeURIComponent(
        toolName,
      )}/config`,
      {
        method: "PUT",
        body: JSON.stringify({ config }),
      },
    ),

  getAgentRunningConfig: (agentId: string) =>
    request<AgentsRunningConfig>(`/agents/${agentId}/running-config`),

  getAgentChatRunningDefaults: (agentId: string) =>
    request<ChatRunningConfig>(
      `/agents/${agentId}/running-config/chat-defaults`,
    ),

  updateAgentRunningConfig: (agentId: string, config: AgentsRunningConfig) =>
    request<AgentsRunningConfig>(`/agents/${agentId}/running-config`, {
      method: "PUT",
      body: JSON.stringify(config),
    }),

  resetAgentRunningConfig: (agentId: string) =>
    request<AgentsRunningConfig>(`/agents/${agentId}/running-config`, {
      method: "DELETE",
    }),

  getAgentLanguage: (agentId: string) =>
    request<{ language: string }>(`/agents/${agentId}/language`),

  updateAgentLanguage: (agentId: string, language: string) =>
    request<{ language: string; copied_files: string[] }>(
      `/agents/${agentId}/language`,
      {
        method: "PUT",
        body: JSON.stringify({ language }),
      },
    ),

  deleteAgentFile: (agentId: string, filename: string, includeSubdir = false) =>
    request<Record<string, unknown>>(
      `/agents/${agentId}/files/${encodeURIComponent(filename)}${
        includeSubdir ? "?include_subdir=true" : ""
      }`,
      { method: "DELETE" },
    ),

  renameAgentFile: (agentId: string, filename: string, newName: string) =>
    request<Record<string, unknown>>(
      `/agents/${agentId}/files/${encodeURIComponent(filename)}/rename`,
      {
        method: "POST",
        body: JSON.stringify({ new_name: newName }),
      },
    ),

  deleteAgentSubdirFile: (agentId: string, subdir: string, filename: string) =>
    request<Record<string, unknown>>(
      `/agents/${agentId}/subdirs/${encodeURIComponent(
        subdir,
      )}/${encodeURIComponent(filename)}`,
      { method: "DELETE" },
    ),

  renameAgentSubdirFile: (
    agentId: string,
    subdir: string,
    filename: string,
    newName: string,
  ) =>
    request<Record<string, unknown>>(
      `/agents/${agentId}/subdirs/${encodeURIComponent(
        subdir,
      )}/${encodeURIComponent(filename)}/rename`,
      {
        method: "POST",
        body: JSON.stringify({ new_name: newName }),
      },
    ),

  downloadAgentWorkspace: async (
    agentId: string,
  ): Promise<{ blob: Blob; filename: string }> => {
    const response = await fetch(
      getApiUrl(`/agents/${agentId}/workspace/download`),
      {
        method: "GET",
        headers: buildAuthHeaders(),
      },
    );

    if (!response.ok) {
      throw new Error(
        `Workspace download failed: ${response.status} ${response.statusText}`,
      );
    }

    const blob = await response.blob();
    const disposition = response.headers.get("Content-Disposition");
    let filename: string;

    if (disposition) {
      const filenameMatch = disposition.match(/filename="(.+?)"/);
      filename = filenameMatch?.[1] || `rua_workspace_${agentId}.zip`;
    } else {
      filename = `rua_workspace_${agentId}.zip`;
    }

    return { blob, filename };
  },

  uploadAgentWorkspace: async (
    agentId: string,
    file: File,
  ): Promise<{ success: boolean; message: string }> => {
    const formData = new FormData();
    formData.append("file", file);

    const response = await fetch(
      getApiUrl(`/agents/${agentId}/workspace/upload`),
      {
        method: "POST",
        headers: buildAuthHeaders(),
        body: formData,
      },
    );

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(
        `Upload failed: ${response.status} ${response.statusText} - ${errorText}`,
      );
    }

    return await response.json();
  },

  listAgentMCPClients: (agentId: string) =>
    request<MCPClientInfo[]>(`/agents/${agentId}/mcp`),

  createAgentMCPClient: (agentId: string, body: MCPClientCreateRequest) =>
    request<MCPClientInfo>(`/agents/${agentId}/mcp`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  applyAgentMCP: (agentId: string) =>
    request<import("../types/mcp").MCPApplication>(
      `/agents/${agentId}/mcp/apply`,
      { method: "POST" },
    ),

  updateAgentMCPClient: (
    agentId: string,
    clientKey: string,
    body: MCPClientUpdateRequest,
  ) =>
    request<MCPClientInfo>(
      `/agents/${agentId}/mcp/${encodeURIComponent(clientKey)}`,
      {
        method: "PUT",
        body: JSON.stringify(body),
      },
    ),

  toggleAgentMCPClient: (agentId: string, clientKey: string) =>
    request<MCPClientInfo>(
      `/agents/${agentId}/mcp/${encodeURIComponent(clientKey)}/toggle`,
      {
        method: "PATCH",
      },
    ),

  deleteAgentMCPClient: (agentId: string, clientKey: string) =>
    request<{
      message: string;
      application?: import("../types/mcp").MCPApplication;
    }>(`/agents/${agentId}/mcp/${encodeURIComponent(clientKey)}`, {
      method: "DELETE",
    }),

  listAgentMCPClientTools: (agentId: string, clientKey: string) =>
    request<MCPToolInfo[]>(
      `/agents/${agentId}/mcp/${encodeURIComponent(clientKey)}/tools`,
    ),

  toggleAgentMCPClientTool: (
    agentId: string,
    clientKey: string,
    toolName: string,
    enabled: boolean,
  ) =>
    request<MCPClientInfo>(
      `/agents/${agentId}/mcp/${encodeURIComponent(
        clientKey,
      )}/tool-toggle/${encodeURIComponent(toolName)}`,
      {
        method: "PATCH",
        body: JSON.stringify({ enabled }),
      },
    ),

  startAgentMCPOAuth: (
    agentId: string,
    clientKey: string,
    body: MCPOAuthStartRequest,
  ) =>
    request<MCPOAuthStartResponse>(
      `/agents/${agentId}/mcp/oauth/start/${encodeURIComponent(clientKey)}`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  getAgentMCPOAuthStatus: (agentId: string, clientKey: string) =>
    request<MCPOAuthStatusResponse>(
      `/agents/${agentId}/mcp/oauth/status/${encodeURIComponent(clientKey)}`,
    ),

  revokeAgentMCPOAuth: (agentId: string, clientKey: string) =>
    request<{ message: string }>(
      `/agents/${agentId}/mcp/oauth/${encodeURIComponent(clientKey)}`,
      {
        method: "DELETE",
      },
    ),

  listAgentSkills: (agentId: string) =>
    request<SkillSpec[]>(`/agents/${agentId}/skills`),

  refreshAgentSkills: (agentId: string) =>
    request<SkillSpec[]>(`/agents/${agentId}/skills/refresh`, {
      method: "POST",
    }),

  createAgentSkill: (
    agentId: string,
    name: string,
    content: string,
    config?: Record<string, unknown>,
    enable?: boolean,
  ) =>
    request<{ created: boolean; name: string }>(`/agents/${agentId}/skills`, {
      method: "POST",
      body: JSON.stringify({ name, content, config, enable }),
    }),

  uploadAgentSkill: async (
    agentId: string,
    file: File,
    options?: {
      enable?: boolean;
      target_name?: string;
      rename_map?: Record<string, string>;
    },
  ): Promise<{
    imported: string[];
    count: number;
    enabled: boolean;
    conflicts?: Array<{
      reason: string;
      skill_name: string;
      suggested_name: string;
    }>;
  }> => {
    const formData = new FormData();
    formData.append("file", file);
    const params = new URLSearchParams();
    if (options?.enable !== undefined)
      params.set("enable", String(options.enable));
    if (options?.target_name) params.set("target_name", options.target_name);
    if (options?.rename_map && Object.keys(options.rename_map).length)
      params.set("rename_map", JSON.stringify(options.rename_map));
    const qs = params.toString();
    const url = getApiUrl(
      `/agents/${agentId}/skills/upload${qs ? `?${qs}` : ""}`,
    );
    const headers = buildAuthHeaders();
    const response = await fetch(url, {
      method: "POST",
      headers,
      body: formData,
    });
    if (!response.ok) throw new Error(await response.text());
    return (await response.json()) as {
      imported: string[];
      count: number;
      enabled: boolean;
      conflicts?: Array<{
        reason: string;
        skill_name: string;
        suggested_name: string;
      }>;
    };
  },

  enableAgentSkill: (agentId: string, name: string) =>
    request<void>(
      `/agents/${agentId}/skills/${encodeURIComponent(name)}/enable`,
      { method: "POST" },
    ),

  disableAgentSkill: (agentId: string, name: string) =>
    request<void>(
      `/agents/${agentId}/skills/${encodeURIComponent(name)}/disable`,
      { method: "POST" },
    ),

  deleteAgentSkill: (agentId: string, name: string) =>
    request<{ deleted: boolean }>(
      `/agents/${agentId}/skills/${encodeURIComponent(name)}`,
      { method: "DELETE" },
    ),

  saveAgentSkill: (
    agentId: string,
    payload: {
      name: string;
      content: string;
      source_name?: string;
      config?: Record<string, unknown>;
      overwrite?: boolean;
    },
  ) =>
    request<{
      success: boolean;
      mode: "edit" | "rename" | "noop";
      name: string;
    }>(`/agents/${agentId}/skills/save`, {
      method: "PUT",
      body: JSON.stringify(payload),
    }),

  updateAgentSkillChannels: (
    agentId: string,
    name: string,
    channels: string[],
  ) =>
    request<{ updated: boolean; channels: string[] }>(
      `/agents/${agentId}/skills/${encodeURIComponent(name)}/channels`,
      {
        method: "PUT",
        body: JSON.stringify(channels),
      },
    ),

  batchDeleteAgentSkills: (agentId: string, names: string[]) =>
    request<{
      results: Record<string, { success: boolean; reason?: string }>;
    }>(`/agents/${agentId}/skills/batch-delete`, {
      method: "POST",
      body: JSON.stringify(names),
    }),

  getAgentSkillConfig: (agentId: string, name: string) =>
    request<{ config: Record<string, unknown> }>(
      `/agents/${agentId}/skills/${encodeURIComponent(name)}/config`,
    ),

  batchEnableAgentSkills: (agentId: string, names: string[]) =>
    request<void>(`/agents/${agentId}/skills/batch-enable`, {
      method: "POST",
      body: JSON.stringify(names),
    }),
};
