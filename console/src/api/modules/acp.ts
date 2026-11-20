import { request } from "../request";
import type {
  ACPChatEnterRequest,
  ACPChatEnterResponse,
  ACPChatExitResponse,
  ACPChatSessionEntry,
  ACPChatStatus,
  ACPRunnerInfo,
  ACPRunnerCheckResult,
  ACPRunnerInstallResult,
  ACPRunnerInstallTask,
  ACPRunnerCreateRequest,
  ACPRunnerUpdateRequest,
  ACPPermissionRequest,
  ACPPermissionSnapshot,
} from "../types";

export const acpApi = {
  listRunners: () => request<ACPRunnerInfo[]>("/acp"),

  getRunner: (runnerKey: string) =>
    request<ACPRunnerInfo>(`/acp/${encodeURIComponent(runnerKey)}`),

  checkRunner: (runnerKey: string) =>
    request<ACPRunnerCheckResult>(
      `/acp/${encodeURIComponent(runnerKey)}/check`,
    ),

  installRunner: (runnerKey: string) =>
    request<ACPRunnerInstallResult>(
      `/acp/${encodeURIComponent(runnerKey)}/install`,
      { method: "POST" },
    ),

  startRunnerInstall: (runnerKey: string) =>
    request<ACPRunnerInstallTask>(
      `/acp/${encodeURIComponent(runnerKey)}/install/start`,
      { method: "POST" },
    ),

  getRunnerInstallTask: (taskId: string) =>
    request<ACPRunnerInstallTask>(
      `/acp/install-tasks/${encodeURIComponent(taskId)}`,
    ),

  createRunner: (body: ACPRunnerCreateRequest) =>
    request<ACPRunnerInfo>("/acp", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  updateRunner: (runnerKey: string, body: ACPRunnerUpdateRequest) =>
    request<ACPRunnerInfo>(`/acp/${encodeURIComponent(runnerKey)}`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),

  toggleRunner: (runnerKey: string) =>
    request<ACPRunnerInfo>(`/acp/${encodeURIComponent(runnerKey)}/toggle`, {
      method: "PATCH",
    }),

  deleteRunner: (runnerKey: string) =>
    request<{ message: string }>(`/acp/${encodeURIComponent(runnerKey)}`, {
      method: "DELETE",
    }),

  listChatRunners: (
    agentId: string | undefined,
    chatId: string,
    targetDeviceId?: string | null,
  ) => {
    const params = new URLSearchParams();
    if (agentId) params.set("agent_id", agentId);
    if (targetDeviceId) params.set("target_device_id", targetDeviceId);
    const query = params.toString();
    return request<ACPRunnerInfo[]>(
      `/chats/${encodeURIComponent(chatId)}/acp/runners${
        query ? `?${query}` : ""
      }`,
    );
  },

  listChatSessions: (
    agentId: string | undefined,
    chatId: string,
    runner: string,
    cwd?: string,
    targetDeviceId?: string | null,
  ) => {
    const params = new URLSearchParams({ runner });
    if (cwd && cwd.trim()) params.set("cwd", cwd.trim());
    if (agentId) params.set("agent_id", agentId);
    if (targetDeviceId) params.set("target_device_id", targetDeviceId);
    return request<ACPChatSessionEntry[]>(
      `/chats/${encodeURIComponent(chatId)}/acp/sessions?${params.toString()}`,
    );
  },

  enterChat: (
    _agentId: string | undefined,
    chatId: string,
    body: ACPChatEnterRequest,
  ) =>
    request<ACPChatEnterResponse>(
      `/chats/${encodeURIComponent(chatId)}/acp/enter`,
      { method: "POST", body: JSON.stringify(body) },
    ),

  exitChat: (
    _agentId: string | undefined,
    chatId: string,
    targetDeviceId?: string | null,
  ) => {
    const params = new URLSearchParams();
    if (targetDeviceId) params.set("target_device_id", targetDeviceId);
    const query = params.toString();
    return request<ACPChatExitResponse>(
      `/chats/${encodeURIComponent(chatId)}/acp/exit${
        query ? `?${query}` : ""
      }`,
      { method: "POST" },
    );
  },

  chatStatus: (
    _agentId: string | undefined,
    chatId: string,
    targetDeviceId?: string | null,
  ) => {
    const params = new URLSearchParams();
    if (targetDeviceId) params.set("target_device_id", targetDeviceId);
    const query = params.toString();
    return request<ACPChatStatus>(
      `/chats/${encodeURIComponent(chatId)}/acp/status${
        query ? `?${query}` : ""
      }`,
    );
  },

  getPermissionSnapshot: (chatId: string) =>
    request<ACPPermissionSnapshot>(
      `/chats/${encodeURIComponent(chatId)}/acp/permissions`,
    ),

  decidePermission: (chatId: string, requestId: string, optionId: string) =>
    request<ACPPermissionRequest>(
      `/chats/${encodeURIComponent(
        chatId,
      )}/acp/permissions/${encodeURIComponent(requestId)}/decide`,
      {
        method: "POST",
        body: JSON.stringify({ option_id: optionId }),
      },
    ),
};
