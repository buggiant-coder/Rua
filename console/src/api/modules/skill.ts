import { request } from "../request";
import { getApiUrl } from "../config";
import { buildAuthHeaders } from "../authHeaders";
import type {
  BuiltinImportSpec,
  BuiltinUpdateNotice,
  HubInstallTaskResponse,
  HubSkillSpec,
  PoolSkillSpec,
  SkillSpec,
  WorkspaceSkillSummary,
} from "../types";

declare const VITE_API_BASE_URL: string;

function getStreamApiUrl(): string {
  const base = typeof VITE_API_BASE_URL === "string" ? VITE_API_BASE_URL : "";
  return `${base}/api`;
}

async function _uploadZip(
  endpoint: string,
  file: File,
  options?: {
    enable?: boolean;
    target_name?: string;
    rename_map?: Record<string, string>;
  },
): Promise<Record<string, unknown>> {
  const formData = new FormData();
  formData.append("file", file);

  const params = new URLSearchParams();
  if (options?.enable !== undefined) {
    params.set("enable", String(options.enable));
  }
  if (options?.target_name) {
    params.set("target_name", options.target_name);
  }
  if (options?.rename_map && Object.keys(options.rename_map).length) {
    params.set("rename_map", JSON.stringify(options.rename_map));
  }
  const qs = params.toString();
  const url = getApiUrl(`${endpoint}${qs ? `?${qs}` : ""}`);

  const headers = buildAuthHeaders();

  const response = await fetch(url, {
    method: "POST",
    headers,
    body: formData,
  });

  if (!response.ok) {
    throw new Error(await response.text());
  }

  return await response.json();
}

export const skillApi = {
  getSkill: (skillName: string, agentId: string) => {
    const opts: RequestInit = {};

    return request<SkillSpec>(
      `/agents/${encodeURIComponent(agentId)}/skills/${encodeURIComponent(
        skillName,
      )}`,
      opts,
    );
  },

  listSkills: async (agentId: string) => {
    const opts: RequestInit = {};

    return request<SkillSpec[]>(
      `/agents/${encodeURIComponent(agentId)}/skills`,
      opts,
    );
  },

  listSkillWorkspaces: async () => {
    return request<WorkspaceSkillSummary[]>("/skills/workspaces");
  },

  listSkillPoolSkills: async () => {
    return request<PoolSkillSpec[]>("/skills/pool");
  },

  refreshSkills: async (agentId: string) => {
    const opts: RequestInit = { method: "POST" };

    return request<SkillSpec[]>(
      `/agents/${encodeURIComponent(agentId)}/skills/refresh`,
      opts,
    );
  },

  refreshSkillPool: async () => {
    return request<PoolSkillSpec[]>("/skills/pool/refresh", {
      method: "POST",
    });
  },

  searchHubSkills: (q: string, limit: number = 20) =>
    request<HubSkillSpec[]>(
      `/skills/hub/search?q=${encodeURIComponent(q)}&limit=${limit}`,
    ),

  createSkillPoolSkill: (payload: {
    name: string;
    content: string;
    config?: Record<string, unknown>;
  }) =>
    request<{ created: boolean; name: string }>("/skills/pool/create", {
      method: "POST",
      body: JSON.stringify(payload),
    }),

  saveSkillPoolSkill: (payload: {
    name: string;
    content: string;
    source_name?: string;
    config?: Record<string, unknown>;
    overwrite?: boolean;
  }) =>
    request<{
      success: boolean;
      mode: "edit" | "rename" | "noop";
      name: string;
    }>("/skills/pool/save", {
      method: "PUT",
      body: JSON.stringify(payload),
    }),

  disableSkill: (skillName: string, agentId: string) =>
    request<void>(
      `/agents/${encodeURIComponent(agentId)}/skills/${encodeURIComponent(
        skillName,
      )}/disable`,
      {
        method: "POST",
      },
    ),

  batchDeletePoolSkills: (skillNames: string[]) =>
    request<{
      results: Record<string, { success: boolean; reason?: string }>;
    }>("/skills/pool/batch-delete", {
      method: "POST",
      body: JSON.stringify(skillNames),
    }),

  deleteSkill: (skillName: string, agentId: string) =>
    request<{ deleted: boolean }>(
      `/agents/${encodeURIComponent(agentId)}/skills/${encodeURIComponent(
        skillName,
      )}`,
      {
        method: "DELETE",
      },
    ),

  startHubSkillInstall: (
    payload: {
      bundle_url: string;
      version?: string;
      enable?: boolean;
      target_name?: string;
    },
    agentId: string,
  ) =>
    request<HubInstallTaskResponse>(
      `/agents/${encodeURIComponent(agentId)}/skills/hub/install/start`,
      {
        method: "POST",
        body: JSON.stringify(payload),
      },
    ),

  importPoolSkillFromHub: (payload: {
    bundle_url: string;
    version?: string;
    target_name?: string;
    overwrite?: boolean;
  }) =>
    request<{
      installed: boolean;
      name: string;
      enabled: boolean;
      source_url: string;
    }>("/skills/pool/import", {
      method: "POST",
      body: JSON.stringify(payload),
    }),

  getHubSkillInstallStatus: (taskId: string, agentId: string) =>
    request<HubInstallTaskResponse>(
      `/agents/${encodeURIComponent(
        agentId,
      )}/skills/hub/install/status/${encodeURIComponent(taskId)}`,
    ),

  cancelHubSkillInstall: (taskId: string, agentId: string) =>
    request<{ task_id: string; status: string }>(
      `/agents/${encodeURIComponent(
        agentId,
      )}/skills/hub/install/cancel/${encodeURIComponent(taskId)}`,
      {
        method: "POST",
      },
    ),

  listPoolBuiltinSources: () =>
    request<BuiltinImportSpec[]>("/skills/pool/builtin-sources"),

  importSelectedPoolBuiltins: (payload: {
    skill_names: string[];
    overwrite_conflicts?: boolean;
  }) =>
    request<{
      imported: string[];
      updated: string[];
      unchanged: string[];
      conflicts: Array<{
        skill_name: string;
        source_version_text?: string;
        current_version_text?: string;
        current_source?: string;
      }>;
    }>("/skills/pool/import-builtin", {
      method: "POST",
      body: JSON.stringify(payload),
    }),

  updatePoolBuiltin: (skillName: string) =>
    request<Record<string, unknown>>(
      `/skills/pool/${encodeURIComponent(skillName)}/update-builtin`,
      { method: "POST" },
    ),

  getPoolBuiltinNotice: () =>
    request<BuiltinUpdateNotice>("/skills/pool/builtin-notice"),

  updatePoolSkillTags: (skillName: string, tags: string[]) =>
    request<{ success: boolean; tags: string[] }>(
      `/skills/pool/${encodeURIComponent(skillName)}/tags`,
      {
        method: "PUT",
        body: JSON.stringify({ tags }),
      },
    ),

  deleteSkillPoolSkill: (skillName: string) =>
    request<{ deleted: boolean }>(
      `/skills/pool/${encodeURIComponent(skillName)}`,
      {
        method: "DELETE",
      },
    ),

  uploadWorkspaceSkillToPool: (payload: {
    workspace_id: string;
    skill_name: string;
    overwrite?: boolean;
    preview_only?: boolean;
  }) =>
    request<{ success: boolean; name: string }>("/skills/pool/upload", {
      method: "POST",
      body: JSON.stringify(payload),
    }),

  downloadSkillPoolSkill: (payload: {
    skill_name: string;
    targets: Array<{ workspace_id: string }>;
    all_workspaces?: boolean;
    overwrite?: boolean;
    preview_only?: boolean;
  }) =>
    request<{
      downloaded: Array<{
        workspace_id: string;
        workspace_name?: string;
        name: string;
      }>;
      conflicts?: Array<{
        reason?: string;
        skill_name?: string;
        workspace_id?: string;
        workspace_name?: string;
        suggested_name?: string;
        current_version_text?: string;
        source_version_text?: string;
      }>;
    }>("/skills/pool/download", {
      method: "POST",
      body: JSON.stringify(payload),
    }),

  getPoolSkillConfig: (skillName: string) =>
    request<{ config: Record<string, unknown> }>(
      `/skills/pool/${encodeURIComponent(skillName)}/config`,
    ),

  updatePoolSkillConfig: (skillName: string, config: Record<string, unknown>) =>
    request<{ updated: boolean }>(
      `/skills/pool/${encodeURIComponent(skillName)}/config`,
      {
        method: "PUT",
        body: JSON.stringify({ config }),
      },
    ),

  deletePoolSkillConfig: (skillName: string) =>
    request<{ cleared: boolean }>(
      `/skills/pool/${encodeURIComponent(skillName)}/config`,
      { method: "DELETE" },
    ),

  streamOptimizeSkill: async function (
    content: string,
    onChunk: (text: string) => void,
    signal: AbortSignal,
    language: string = "en",
  ): Promise<void> {
    const apiUrl = getStreamApiUrl();

    const response = await fetch(`${apiUrl}/skills/ai/optimize/stream`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ content, language }),
      signal,
    });

    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }

    const reader = response.body?.getReader();
    if (!reader) {
      throw new Error("No reader available");
    }

    const decoder = new TextDecoder();
    let buffer = "";

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split("\n");

        for (let i = 0; i < lines.length - 1; i++) {
          const line = lines[i].trim();
          if (line.startsWith("data: ")) {
            const data = line.slice(6);
            try {
              const parsed = JSON.parse(data);
              if (parsed.text) {
                onChunk(parsed.text);
              } else if (parsed.error) {
                throw new Error(parsed.error);
              } else if (parsed.done) {
                return;
              }
            } catch {}
          }
        }

        buffer = lines[lines.length - 1];
      }
    } finally {
      reader.releaseLock();
    }
  },

  checkSkillUpdate: (name: string, agentId: string) =>
    request<{
      has_update: boolean;
      current_version: string;
      latest_version: string;
      local_modified: boolean;
    }>(
      `/agents/${encodeURIComponent(agentId)}/skills/${encodeURIComponent(
        name,
      )}/check-update`,
      { method: "POST" },
    ),

  checkPoolSkillUpdate: (name: string) =>
    request<{
      has_update: boolean;
      current_version: string;
      latest_version: string;
      local_modified: boolean;
    }>(`/skills/pool/${encodeURIComponent(name)}/check-update`, {
      method: "POST",
    }),

  updateSkill: (name: string, force: boolean, agentId: string) =>
    request<{
      updated: boolean;
      old_version: string;
      new_version: string;
    }>(
      `/agents/${encodeURIComponent(agentId)}/skills/${encodeURIComponent(
        name,
      )}/update`,
      {
        method: "POST",
        body: JSON.stringify({ force }),
      },
    ),

  updatePoolSkill: (name: string, force: boolean) =>
    request<{
      updated: boolean;
      old_version: string;
      new_version: string;
    }>(`/skills/pool/${encodeURIComponent(name)}/update`, {
      method: "POST",
      body: JSON.stringify({ force }),
    }),

  uploadSkillPoolZip: (
    file: File,
    options?: {
      target_name?: string;
      rename_map?: Record<string, string>;
      overwrite?: boolean;
    },
  ) =>
    _uploadZip("/skills/pool/upload-zip", file, options) as Promise<{
      imported: string[];
      count: number;
      conflicts?: Array<{
        reason: string;
        skill_name: string;
        suggested_name: string;
      }>;
    }>,
};
