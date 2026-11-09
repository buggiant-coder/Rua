import { request } from "../request";
import { buildAuthHeaders } from "../authHeaders";
import { getApiUrl } from "../config";
import type {
  ACPChatEnterRequest,
  ACPChatEnterResponse,
  ACPChatExitResponse,
  ACPChatSessionEntry,
  ACPChatStatus,
  ACPPermissionRequest,
  ACPPermissionSnapshot,
  ACPRunnerInfo,
  AcpHistoryPreviewRequest,
  AcpHistoryPreviewResponse,
  ChatRunningConfig,
  ChatHistory,
  ChatSearchResponse,
  ChatSpec,
  ChatTag,
  AgentCollaborationMode,
  ModelSlot,
  QueueInputDeleteResponse,
  QueueInputEditResponse,
  QueueInputSendNowRequest,
  QueueInputSendNowResponse,
} from "../types";
import type {
  GetChatOptions,
  StopChatResult,
  StopChatRequest,
  LlmDebugCaptureStatus,
} from "./chat";
import type {
  ApprovalDecisionRequest,
  ApprovalRequest,
  ApprovalSnapshot,
} from "./console";
import type { ProjectChatAgentState } from "../types/project";
import type { ChatListSort } from "../types/chatListSort";

export type ProjectChatListParams = Partial<ChatListSort> & {
  offset?: number;
  limit?: number;
  search?: string;
  tag?: ChatTag;
  view?: "user" | "collaboration" | "all";
  archived?: boolean;
  source_chat_id?: string;
  executor?: string;
  status?: string;
};

export type ProjectChatUpdate = {
  archived?: boolean;
  name?: string;
  pinned?: boolean;
  pin_order?: number;
  tag?: ChatTag;
  model?: ModelSlot | null;
  collaboration_mode?: AgentCollaborationMode;
};

export type ProjectChatDeleteResponse = {
  deleted: true;
  chat_id: string;
  was_busy: boolean;
};

export type ProjectBranchAction = {
  before_turn_id: string;
};

export type ProjectBranchActionResponse = {
  chat: ChatSpec;
};

export type ProjectRollbackActionResponse = ProjectBranchActionResponse & {
  cancelled_tasks: number;
};

export type ProjectWorkspaceProfile =
  | "read-only"
  | "resource-write"
  | "danger-full-access";

export type ProjectResourcePolicyView = {
  policy: {
    profile: ProjectWorkspaceProfile;
    source: "project_default" | "agent_security";
    cwd: string;
    writable_roots: string[];
    dedicated_operations_root_confined: boolean;
    tool_guard_enabled: boolean;
    approval_mode: string;
  };
  pending_approval_count: number;
  chat_rule_count: number;
  agent_rule_count: number;
};

function projectChatPath(projectId: string, chatId: string): string {
  return `/projects/${encodeURIComponent(projectId)}/chats/${encodeURIComponent(
    chatId,
  )}`;
}

function projectChatChildPath(
  projectId: string,
  chatId: string,
  child: string,
): string {
  return `${projectChatPath(projectId, chatId)}/${child}`;
}

function projectChatsPath(
  projectId: string,
  params?: ProjectChatListParams,
): string {
  const searchParams = new URLSearchParams();
  if (params?.archived != null) {
    searchParams.set("archived", String(params.archived));
  }
  if (params?.offset != null) {
    searchParams.set("offset", String(params.offset));
  }
  if (params?.limit != null) {
    searchParams.set("limit", String(params.limit));
  }
  if (params?.search?.trim()) {
    searchParams.set("search", params.search.trim());
  }
  if (params?.tag) {
    searchParams.set("tag", params.tag);
  }
  if (params?.view) {
    searchParams.set("view", params.view);
  }
  if (params?.source_chat_id) {
    searchParams.set("source_chat_id", params.source_chat_id);
  }
  if (params?.executor) {
    searchParams.set("executor", params.executor);
  }
  if (params?.status) {
    searchParams.set("status", params.status);
  }
  if (params?.sort_by) {
    searchParams.set("sort_by", params.sort_by);
  }
  if (params?.sort_order) {
    searchParams.set("sort_order", params.sort_order);
  }
  const query = searchParams.toString();
  return `/projects/${encodeURIComponent(projectId)}/chats${
    query ? `?${query}` : ""
  }`;
}

function historyPath(
  projectId: string,
  chatId: string,
  options?: GetChatOptions,
): string {
  const searchParams = new URLSearchParams();
  if (options?.turnLimit != null) {
    searchParams.set("turn_limit", String(options.turnLimit));
  }
  if (options?.beforeTurnOrdinal != null) {
    searchParams.set("before_turn_ordinal", String(options.beforeTurnOrdinal));
  }
  if (options?.aroundTurnId)
    searchParams.set("around_turn_id", options.aroundTurnId);
  const query = searchParams.toString();
  return `${projectChatPath(projectId, chatId)}${query ? `?${query}` : ""}`;
}

export const projectChatsApi = {
  clearArchive: (projectId: string, params: ProjectChatListParams) =>
    request<{
      deleted: string[];
      skipped: Array<{ chat_id: string; reason: string }>;
    }>(
      `/projects/${encodeURIComponent(
        projectId,
      )}/chats/delete-archived?${new URLSearchParams(
        Object.entries(params)
          .filter(([, value]) => value !== undefined)
          .map(([key, value]) => [key, String(value)]),
      )}`,
      {
        method: "POST",
      },
    ),
  getLlmDebugCapture: (projectId: string, chatId: string) =>
    request<LlmDebugCaptureStatus>(
      `/projects/${encodeURIComponent(projectId)}/chats/${encodeURIComponent(
        chatId,
      )}/llm-debug-capture`,
    ),
  startLlmDebugCapture: (projectId: string, chatId: string) =>
    request<LlmDebugCaptureStatus>(
      `/projects/${encodeURIComponent(projectId)}/chats/${encodeURIComponent(
        chatId,
      )}/llm-debug-capture`,
      { method: "POST" },
    ),
  stopLlmDebugCapture: (projectId: string, chatId: string) =>
    request<LlmDebugCaptureStatus>(
      `/projects/${encodeURIComponent(projectId)}/chats/${encodeURIComponent(
        chatId,
      )}/llm-debug-capture`,
      { method: "DELETE" },
    ),
  listChats: async (
    projectId: string,
    params?: ProjectChatListParams,
  ): Promise<{ data: ChatSpec[]; total: number }> => {
    const response = await fetch(
      getApiUrl(projectChatsPath(projectId, params)),
      {
        headers: buildAuthHeaders(),
        cache: "no-store",
      },
    );
    if (!response.ok) {
      throw new Error(`Failed to list Project Chats: ${response.status}`);
    }
    const data = (await response.json()) as ChatSpec[];
    const total = Number.parseInt(
      response.headers.get("X-Total-Count") || "0",
      10,
    );
    return { data, total: Number.isFinite(total) ? total : 0 };
  },

  searchChats: (projectId: string, query: string, offset = 0, limit = 50) =>
    request<ChatSearchResponse>(
      `/projects/${encodeURIComponent(
        projectId,
      )}/chats/search?q=${encodeURIComponent(
        query,
      )}&offset=${offset}&limit=${limit}`,
    ),

  getChat: (projectId: string, chatId: string, options?: GetChatOptions) =>
    request<ChatHistory>(historyPath(projectId, chatId, options)),

  getMetadata: (
    projectId: string,
    chatId: string,
    options?: { signal?: AbortSignal },
  ) =>
    request<ChatSpec>(
      projectChatChildPath(projectId, chatId, "metadata"),
      options,
    ),

  updateChat: (projectId: string, chatId: string, update: ProjectChatUpdate) =>
    request<ChatSpec>(projectChatPath(projectId, chatId), {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(update),
    }),

  selectVariant: (
    projectId: string,
    chatId: string,
    turnId: string,
    variantId: string,
  ) =>
    request<{ turn_id: string; selected_variant_id: string }>(
      projectChatPath(projectId, chatId),
      {
        method: "PUT",
        body: JSON.stringify({
          turn_id: turnId,
          selected_variant_id: variantId,
        }),
      },
    ),

  rollback: (projectId: string, chatId: string, body: ProjectBranchAction) =>
    request<ProjectRollbackActionResponse>(
      projectChatChildPath(projectId, chatId, "rollback"),
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  getProjectionExclusions: (projectId: string, chatId: string) =>
    request<{ message_ids: string[] }>(
      projectChatChildPath(projectId, chatId, "projection-exclusions"),
    ),

  updateProjectionExclusions: (
    projectId: string,
    chatId: string,
    messageIds: string[],
  ) =>
    request<{ message_ids: string[] }>(
      projectChatChildPath(projectId, chatId, "projection-exclusions"),
      {
        method: "PUT",
        body: JSON.stringify({ message_ids: messageIds }),
      },
    ),

  branch: (projectId: string, chatId: string, body: ProjectBranchAction) =>
    request<ProjectBranchActionResponse>(
      projectChatChildPath(projectId, chatId, "branch"),
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  deleteChat: (projectId: string, chatId: string, force = false) =>
    request<ProjectChatDeleteResponse>(
      `${projectChatPath(projectId, chatId)}?force=${String(force)}`,
      { method: "DELETE" },
    ),

  getRunningConfig: (projectId: string, chatId: string) =>
    request<ChatRunningConfig>(
      projectChatChildPath(projectId, chatId, "running-config"),
    ),

  getResourcePolicy: (projectId: string, chatId: string) =>
    request<ProjectResourcePolicyView>(
      projectChatChildPath(projectId, chatId, "resource-policy"),
    ),

  updateRunningConfig: (
    projectId: string,
    chatId: string,
    config: ChatRunningConfig,
  ) =>
    request<ChatRunningConfig>(
      projectChatChildPath(projectId, chatId, "running-config"),
      {
        method: "PUT",
        body: JSON.stringify(config),
      },
    ),

  persistAcpHistoryPreview: (
    projectId: string,
    chatId: string,
    body: AcpHistoryPreviewRequest,
  ) =>
    request<AcpHistoryPreviewResponse>(
      projectChatChildPath(projectId, chatId, "acp-history-preview"),
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  deleteQueuedInput: (projectId: string, chatId: string, inputId: string) =>
    request<QueueInputDeleteResponse>(
      projectChatChildPath(
        projectId,
        chatId,
        `queue/${encodeURIComponent(inputId)}`,
      ),
      { method: "DELETE" },
    ),

  editQueuedInput: (
    projectId: string,
    chatId: string,
    inputId: string,
    message: string,
  ) =>
    request<QueueInputEditResponse>(
      projectChatChildPath(
        projectId,
        chatId,
        `queue/${encodeURIComponent(inputId)}`,
      ),
      {
        method: "PATCH",
        body: JSON.stringify({ message }),
      },
    ),

  sendNowQueuedInput: (
    projectId: string,
    chatId: string,
    inputId: string,
    body: QueueInputSendNowRequest,
  ) =>
    request<QueueInputSendNowResponse>(
      projectChatChildPath(
        projectId,
        chatId,
        `queue/${encodeURIComponent(inputId)}/send-now`,
      ),
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  stopChat: (projectId: string, chatId: string, body: StopChatRequest) =>
    request<StopChatResult>(
      projectChatChildPath(projectId, chatId, "console/stop"),
      { method: "POST", body: JSON.stringify(body) },
    ),

  getApprovalSnapshot: (projectId: string, chatId: string) =>
    request<ApprovalSnapshot>(
      projectChatChildPath(projectId, chatId, "approvals/snapshot"),
    ),

  decideApproval: (
    projectId: string,
    chatId: string,
    approvalId: string,
    body: ApprovalDecisionRequest,
  ) =>
    request<ApprovalRequest>(
      projectChatChildPath(
        projectId,
        chatId,
        `approvals/${encodeURIComponent(approvalId)}/decide`,
      ),
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  listAcpRunners: (projectId: string, chatId: string) =>
    request<ACPRunnerInfo[]>(
      projectChatChildPath(projectId, chatId, "acp/runners"),
    ),

  listAcpSessions: (
    projectId: string,
    chatId: string,
    runner: string,
    cwd?: string,
  ) => {
    const params = new URLSearchParams({ runner });
    if (cwd?.trim()) params.set("cwd", cwd.trim());
    return request<ACPChatSessionEntry[]>(
      `${projectChatChildPath(
        projectId,
        chatId,
        "acp/sessions",
      )}?${params.toString()}`,
    );
  },

  enterAcp: (projectId: string, chatId: string, body: ACPChatEnterRequest) => {
    const { target_device_id: _fixedProjectAuthority, ...fixedBody } = body;
    return request<ACPChatEnterResponse>(
      projectChatChildPath(projectId, chatId, "acp/enter"),
      {
        method: "POST",
        body: JSON.stringify(fixedBody),
      },
    );
  },

  exitAcp: (projectId: string, chatId: string) =>
    request<ACPChatExitResponse>(
      projectChatChildPath(projectId, chatId, "acp/exit"),
      { method: "POST" },
    ),

  getAcpStatus: (projectId: string, chatId: string) =>
    request<ACPChatStatus>(
      projectChatChildPath(projectId, chatId, "acp/status"),
    ),

  getAcpPermissionSnapshot: (projectId: string, chatId: string) =>
    request<ACPPermissionSnapshot>(
      projectChatChildPath(projectId, chatId, "acp/permissions"),
    ),

  decideAcpPermission: (
    projectId: string,
    chatId: string,
    requestId: string,
    optionId: string,
  ) =>
    request<ACPPermissionRequest>(
      projectChatChildPath(
        projectId,
        chatId,
        `acp/permissions/${encodeURIComponent(requestId)}/decide`,
      ),
      {
        method: "POST",
        body: JSON.stringify({ option_id: optionId }),
      },
    ),

  submitPath: (projectId: string, chatId: string) =>
    `${projectChatPath(projectId, chatId)}/console/submit`,

  livePath: (projectId: string, chatId: string) =>
    `${projectChatPath(projectId, chatId)}/console/live`,

  switchAgent: (
    projectId: string,
    chatId: string,
    expectedAgentId: string,
    agentId: string,
  ) =>
    request<ProjectChatAgentState>(
      `${projectChatPath(projectId, chatId)}/agent-selection`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          expected_agent_id: expectedAgentId,
          agent_id: agentId,
        }),
      },
    ),
};
