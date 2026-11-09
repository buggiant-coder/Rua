import { request } from "../request";

export interface HookDefinitionView {
  id: string;
  owner_namespace: string;
  logical_id: string;
  display_name: string;
  source: "native" | "user" | "project" | "plugin";
  latest_revision: string;
  revision: string;
  event: string;
  capabilities: string[];
  handler_kind: string;
  registered: boolean;
  availability: { status: "available" | "unavailable"; diagnostics?: string[] };
  validation: { status: "valid" | "invalid"; diagnostics?: string[] };
  active: boolean;
  disabled: boolean;
  revision_changed: boolean;
  diagnostics?: string[];
}

export interface HookDefinitionDetailView extends HookDefinitionView {
  revisions: Array<{
    revision: string;
    event: string;
    capabilities: string[];
    handler: {
      kind: string;
      executable?: string;
      program?: string;
      args?: string[];
      working_directory?: string;
      timeout_seconds?: number;
      available: boolean;
    };
  }>;
}

export interface CreateHookDefinitionInput {
  logical_id: string;
  display_name: string;
  event: string;
  capabilities: string[];
  handler: {
    kind: "command";
    program: string;
    args: string[];
    working_directory: string;
    timeout_seconds: number;
    available: boolean;
  };
}

export interface ChatHookConfiguration {
  epoch: string;
  selected_agent_id: string;
  authority_device_id?: string;
  definition_revisions: string[];
  effective_hooks: EffectiveHookPreview["hooks"];
  warnings: EffectiveHookPreview["warnings"];
  execution_requirements: Array<{
    definition_revision: string;
    authority_device_id: string;
    handler_kind: string;
    capabilities: string[];
    runtime: string;
    executable: string;
    working_directory?: string;
    required_resources: string[];
    available: boolean;
  }>;
}

export interface HookRunView {
  invocation_id: string;
  agent_id: string;
  chat_id: string;
  turn_id: string;
  event: string;
  definition_revision: string;
  source: string;
  resolved_order: number;
  status: "completed" | "failed" | "cancelled";
  decision_summary?: string;
  diagnostics?: string[];
  started_at: string;
  completed_at: string;
}

export interface HookContributionView {
  chat_address: { scope: { kind: string }; chat_id: string };
  turn_id: string;
  tool_call_id: string;
  event: string;
  definition_revision: string;
  source: string;
  resolved_order: number;
  kind: "feedback" | "context" | "diagnostic" | "permission_decision";
  content: unknown;
  diagnostic?: string;
}

export interface HookBindingInput {
  definition_revision: string;
  event: string;
  source: "agent" | "project" | "chat";
  configured_order?: number;
  parameters?: Record<string, unknown>;
  before?: string[];
  after?: string[];
  requires?: string[];
}

export interface EffectiveHookPreview {
  hooks: Array<{
    definition_revision: string;
    event: string;
    resolved_order: number;
    parameters: Record<string, unknown>;
    selected_by: Array<"agent" | "project" | "chat">;
    overridden_parameters: string[];
  }>;
  warnings: Array<{
    definition_revision: string;
    relationship: string;
    missing_target: string;
  }>;
}

const agentHooksPath = (agentId: string) =>
  `/agents/${encodeURIComponent(agentId)}/hooks`;
const projectHooksPath = (projectId: string) =>
  `/projects/${encodeURIComponent(projectId)}/hooks`;

export const hooksApi = {
  listDefinitions: (agentId: string) =>
    request<HookDefinitionView[]>(`${agentHooksPath(agentId)}/definitions`),
  createDefinition: (agentId: string, input: CreateHookDefinitionInput) =>
    request<HookDefinitionView>(`${agentHooksPath(agentId)}/definitions`, {
      method: "POST",
      body: JSON.stringify(input),
    }),
  getDefinition: (agentId: string, ownerNamespace: string, logicalId: string) =>
    request<HookDefinitionDetailView>(
      `${agentHooksPath(agentId)}/definitions/${encodeURIComponent(
        ownerNamespace,
      )}/${encodeURIComponent(logicalId)}`,
    ),
  reviseDefinition: (
    agentId: string,
    ownerNamespace: string,
    logicalId: string,
    input: { display_name?: string; executable?: string; available?: boolean },
  ) =>
    request<HookDefinitionView>(
      `${agentHooksPath(agentId)}/definitions/${encodeURIComponent(
        ownerNamespace,
      )}/${encodeURIComponent(logicalId)}`,
      { method: "PUT", body: JSON.stringify(input) },
    ),
  updateDefault: (agentId: string, definitionId: string, enabled: boolean) =>
    request<HookDefinitionView[]>(`${agentHooksPath(agentId)}/defaults`, {
      method: "PUT",
      body: JSON.stringify({ definition_id: definitionId, enabled }),
    }),
  listProjectDefinitions: (projectId: string) =>
    request<HookDefinitionView[]>(`${projectHooksPath(projectId)}/definitions`),
  updateProjectDefault: (
    projectId: string,
    definitionId: string,
    enabled: boolean,
  ) =>
    request<HookDefinitionView[]>(`${projectHooksPath(projectId)}/defaults`, {
      method: "PUT",
      body: JSON.stringify({ definition_id: definitionId, enabled }),
    }),
  getChatConfiguration: (agentId: string, chatId: string) =>
    request<ChatHookConfiguration>(
      `${agentHooksPath(agentId)}/chats/${encodeURIComponent(
        chatId,
      )}/configuration`,
    ),
  listRuns: (agentId: string, chatId?: string) => {
    const query = chatId ? `?chat_id=${encodeURIComponent(chatId)}` : "";
    return request<HookRunView[]>(`${agentHooksPath(agentId)}/runs${query}`);
  },
  listContributions: (agentId: string, chatId: string, turnId: string) =>
    request<HookContributionView[]>(
      `${agentHooksPath(agentId)}/contributions?chat_id=${encodeURIComponent(
        chatId,
      )}&turn_id=${encodeURIComponent(turnId)}`,
    ),
  preview: (
    agentId: string,
    input: {
      agent?: HookBindingInput[];
      project?: HookBindingInput[];
      chat?: HookBindingInput[];
    },
  ) =>
    request<EffectiveHookPreview>(
      `${agentHooksPath(agentId)}/effective-preview`,
      {
        method: "POST",
        body: JSON.stringify(input),
      },
    ),
};
