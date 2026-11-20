import { consoleFetch } from "../consoleTransport";
import type {
  ACPChatEnterRequest,
  ChatRunningConfig,
  AcpHistoryPreviewRequest,
  ModelSlot,
  QueueInputSendNowRequest,
} from "../types";
import { buildAuthHeaders } from "../authHeaders";
import { getApiUrl } from "../config";
import { acpApi } from "./acp";
import { chatApi, type GetChatOptions, type StopChatRequest } from "./chat";
import { consoleApi } from "./console";
import { request } from "../request";

export interface ChatConfigurationPreview {
  current: Record<string, unknown> & { epoch: string };
  candidate: Record<string, unknown> & { epoch: string };
  applied: boolean;
  collaboration?: {
    source: Record<string, unknown>;
    desired_source_epoch: string;
    applied_source_epoch: string;
    status: "ready" | "applying" | "failed";
    error?: string;
  };
}

/** Creation/execution context; never part of Chat identity or its URL. */
export type ConversationApiTarget = { agentId?: string; projectId?: string };

export function agentConversationTarget(
  agentId: string,
): ConversationApiTarget {
  return { agentId };
}

export function projectConversationTarget(
  projectId: string,
): ConversationApiTarget {
  return { projectId };
}

function conversationPath(
  _target: ConversationApiTarget,
  chatId: string,
): string {
  return `/chats/${encodeURIComponent(chatId)}`;
}

export const conversationApi = {
  previewConfiguration: (target: ConversationApiTarget, chatId: string) =>
    request<ChatConfigurationPreview>(
      `${conversationPath(target, chatId)}/configuration`,
    ),

  applyConfiguration: (
    target: ConversationApiTarget,
    chatId: string,
    preview: ChatConfigurationPreview,
  ) =>
    request<ChatConfigurationPreview>(
      `${conversationPath(target, chatId)}/configuration`,
      {
        method: "PUT",
        body: JSON.stringify({
          expected_epoch: preview.current.epoch,
          candidate_epoch: preview.candidate.epoch,
        }),
      },
    ),
  submit: (
    chatId: string,
    body: Record<string, unknown>,
    signal?: AbortSignal,
  ) =>
    fetch(getApiUrl(chatApi.submitPath(chatId)), {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...buildAuthHeaders(),
      },
      body: JSON.stringify(body),
      signal,
    }),

  openLive: (
    chatId: string,
    body: Record<string, unknown>,
    signal: AbortSignal,
  ) =>
    consoleFetch(chatApi.livePath(chatId), {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...buildAuthHeaders(),
      },
      body: JSON.stringify(body),
      signal,
    }),

  getHistory: (chatId: string, options?: GetChatOptions) =>
    chatApi.getChat(chatId, options),

  updateModel: (
    target: ConversationApiTarget,
    chatId: string,
    model: ModelSlot,
  ) => chatApi.updateChat(chatId, { model }, target.agentId),

  getRunningConfig: (target: ConversationApiTarget, chatId: string) =>
    chatApi.getRunningConfig(chatId, target.agentId),

  updateRunningConfig: (
    target: ConversationApiTarget,
    chatId: string,
    config: ChatRunningConfig,
  ) => chatApi.updateRunningConfig(chatId, config, target.agentId),

  persistAcpHistoryPreview: (
    target: ConversationApiTarget,
    chatId: string,
    body: AcpHistoryPreviewRequest,
  ) => chatApi.persistAcpHistoryPreview(chatId, body, target.agentId),

  stopChat: (
    target: ConversationApiTarget,
    chatId: string,
    body: StopChatRequest,
  ) => chatApi.stopChat(chatId, body, target.agentId),

  selectVariant: (
    target: ConversationApiTarget,
    chatId: string,
    turnId: string,
    variantId: string,
  ) => chatApi.selectVariant(chatId, turnId, variantId, target.agentId),

  rollback: (
    target: ConversationApiTarget,
    chatId: string,
    body: { before_turn_id: string },
  ) => chatApi.rollback(chatId, body, target.agentId),

  getProjectionExclusions: (target: ConversationApiTarget, chatId: string) =>
    chatApi.getProjectionExclusions(chatId, target.agentId),

  updateProjectionExclusions: (
    target: ConversationApiTarget,
    chatId: string,
    messageIds: string[],
  ) => chatApi.updateProjectionExclusions(chatId, messageIds, target.agentId),

  branch: (
    target: ConversationApiTarget,
    chatId: string,
    body: { before_turn_id: string },
  ) => chatApi.branch(chatId, body, target.agentId),

  sendNowQueuedInput: (
    target: ConversationApiTarget,
    chatId: string,
    inputId: string,
    body: QueueInputSendNowRequest,
  ) => chatApi.sendNowQueuedInput(chatId, inputId, body, target.agentId),

  deleteQueuedInput: (
    target: ConversationApiTarget,
    chatId: string,
    inputId: string,
  ) => chatApi.deleteQueuedInput(chatId, inputId, target.agentId),

  editQueuedInput: (
    target: ConversationApiTarget,
    chatId: string,
    inputId: string,
    message: string,
  ) => chatApi.editQueuedInput(chatId, inputId, message, target.agentId),

  getApprovalSnapshot: consoleApi.getApprovalSnapshot,
  decideApproval: consoleApi.decideApproval,

  listAcpRunners: (
    target: ConversationApiTarget,
    chatId: string,
    targetDeviceId?: string | null,
  ) => acpApi.listChatRunners(target.agentId, chatId, targetDeviceId),

  listAcpSessions: (
    target: ConversationApiTarget,
    chatId: string,
    runner: string,
    cwd?: string,
    targetDeviceId?: string | null,
  ) =>
    acpApi.listChatSessions(
      target.agentId,
      chatId,
      runner,
      cwd,
      targetDeviceId,
    ),

  enterAcp: (
    target: ConversationApiTarget,
    chatId: string,
    body: ACPChatEnterRequest,
  ) => acpApi.enterChat(target.agentId, chatId, body),

  exitAcp: (
    target: ConversationApiTarget,
    chatId: string,
    targetDeviceId?: string | null,
  ) => acpApi.exitChat(target.agentId, chatId, targetDeviceId),

  getAcpStatus: (
    target: ConversationApiTarget,
    chatId: string,
    targetDeviceId?: string | null,
  ) => acpApi.chatStatus(target.agentId, chatId, targetDeviceId),

  getAcpPermissionSnapshot: acpApi.getPermissionSnapshot,
  decideAcpPermission: acpApi.decidePermission,
};
