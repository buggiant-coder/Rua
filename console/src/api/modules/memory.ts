import { request } from "../request";

export type MemoryScope = "user" | "agent" | "workspace" | "project";
export type MemoryKind =
  | "fact"
  | "preference"
  | "convention"
  | "project_fact"
  | "tool_quirk"
  | "personal_fact";
export type MemoryRecordStatus =
  | "active"
  | "superseded"
  | "expired"
  | "deleted";
export type MemoryVisibilityMode = "private" | "selected" | "all_agents";

export interface MemorySourceEvidence {
  chat_id?: string | null;
  turn_id?: string | null;
  message_id?: string | null;
  quote?: string | null;
}

export interface MemoryTerm {
  term: string;
  normalized_term: string;
  term_kind: "label" | "search_term" | "derived_term" | "resource_token";
  source: "agent" | "memory_manager" | "autodream";
  language?: string | null;
  created_at: string;
}

export interface MemoryRecord {
  id: string;
  agent_id: string;
  scope: MemoryScope;
  kind: MemoryKind;
  title: string;
  content: string;
  status: MemoryRecordStatus;
  confidence: number;
  importance?: number | null;
  created_at: string;
  updated_at?: string | null;
  last_used_at?: string | null;
  expires_at?: string | null;
  superseded_by?: string | null;
  written_on_device_id?: string | null;
  written_on_device_name_snapshot?: string | null;
  written_on_host_profile?: string | null;
  resource_device_id?: string | null;
  resource_device_name_snapshot?: string | null;
  metadata?: unknown;
  sources?: MemorySourceEvidence[];
  links?: unknown[];
  terms?: MemoryTerm[];
}

export interface MemoryVisibilityGrant {
  memory_id: string;
  owner_agent_id: string;
  grantee_agent_id: string;
  permission: string;
  created_by: string;
  created_at: string;
}

export interface MemoryVisibility {
  memory_id: string;
  owner_agent_id: string;
  mode: MemoryVisibilityMode;
  grantee_agent_ids: string[];
  grants: MemoryVisibilityGrant[];
}

export interface MemoryVisibilityAgentOption {
  id: string;
  name: string;
  enabled: boolean;
}

export interface MemoryListResponse {
  records: MemoryRecord[];
  visibility?: Record<string, MemoryVisibility>;
}

export interface MemoryHealthResponse {
  healthy: boolean;
  document_count: number;
  index_size_bytes: number;
  issues?: string[];
}

export interface MemoryReindexResponse {
  documents_indexed: number;
}

export interface MemoryUpdateRequest {
  kind: MemoryKind;
  scope: MemoryScope;
  title: string;
  content: string;
  confidence: number;
  importance?: number | null;
  search_terms: string[];
}

export interface MemoryVisibilityUpdateRequest {
  mode: MemoryVisibilityMode;
  grantee_agent_ids?: string[];
}

export const memoryApi = {
  listMemoryRecords: (agentId: string) =>
    request<MemoryListResponse>(
      `/agents/${encodeURIComponent(agentId)}/memory`,
    ),

  deleteMemoryRecord: (agentId: string, memoryId: string) =>
    request<{ deleted: boolean }>(
      `/agents/${encodeURIComponent(agentId)}/memory/${encodeURIComponent(
        memoryId,
      )}`,
      { method: "DELETE" },
    ),

  updateMemoryRecord: (
    agentId: string,
    memoryId: string,
    body: MemoryUpdateRequest,
  ) =>
    request<MemoryRecord>(
      `/agents/${encodeURIComponent(agentId)}/memory/${encodeURIComponent(
        memoryId,
      )}`,
      { method: "PATCH", body: JSON.stringify(body) },
    ),

  getMemoryVisibility: (agentId: string, memoryId: string) =>
    request<MemoryVisibility>(
      `/agents/${encodeURIComponent(agentId)}/memory/${encodeURIComponent(
        memoryId,
      )}/visibility`,
    ),

  updateMemoryVisibility: (
    agentId: string,
    memoryId: string,
    body: MemoryVisibilityUpdateRequest,
  ) =>
    request<MemoryVisibility>(
      `/agents/${encodeURIComponent(agentId)}/memory/${encodeURIComponent(
        memoryId,
      )}/visibility`,
      { method: "PATCH", body: JSON.stringify(body) },
    ),

  getMemoryVisibilityOptions: (agentId: string) =>
    request<{ agents: MemoryVisibilityAgentOption[] }>(
      `/agents/${encodeURIComponent(agentId)}/memory/visibility-options`,
    ),

  getMemoryHealth: (agentId: string) =>
    request<MemoryHealthResponse>(
      `/agents/${encodeURIComponent(agentId)}/memory/health`,
    ),

  reindexMemory: (agentId: string) =>
    request<MemoryReindexResponse>(
      `/agents/${encodeURIComponent(agentId)}/memory/reindex`,
      { method: "POST" },
    ),
};
