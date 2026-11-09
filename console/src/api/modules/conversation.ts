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
import { consoleApi, type ApprovalDecisionRequest } from "./console";
import { projectChatsApi } from "./projectChats";
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

export type ConversationApiTarget =
  | {
      scope: "agent";
      agentId: string;
    }
  | {
      scope: "project";
      projectId: string;
    };

export function agentConversationTarget(
  agentId: string,
): ConversationApiTarget {
  return { scope: "agent", agentId };
}

export function projectConversationTarget(
  projectId: string,
): ConversationApiTarget {
  return { scope: "project", projectId };
}

function conversationPath(
  target: ConversationApiTarget,
  chatId: string,
): string {
  return target.scope === "project"
    ? `/projects/${encodeURIComponent(
        target.projectId,
      )}/chats/${encodeURIComponent(chatId)}`
    : `/agents/${encodeURIComponent(target.agentId)}/chats/${encodeURIComponent(
        chatId,
      )}`;
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
    target: ConversationApiTarget,
    chatId: string,
    body: Record<string, unknown>,
    projectExecutionAgentId?: string,
    signal?: AbortSignal,
  ) =>
    fetch(
      getApiUrl(
        target.scope === "project"
          ? projectChatsApi.submitPath(target.projectId, chatId)
          : chatApi.submitPath(chatId, target.agentId),
      ),
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...buildAuthHeaders(),
        },
        body: JSON.stringify(
          target.scope === "project"
            ? { ...body, agent_id: projectExecutionAgentId }
            : body,
        ),
        signal,
      },
    ),

  openLive: (
    target: ConversationApiTarget,
    chatId: string,
    body: Record<string, unknown>,
    signal: AbortSignal,
  ) =>
    consoleFetch(
      target.scope === "project"
        ? projectChatsApi.livePath(target.projectId, chatId)
        : chatApi.livePath(chatId, target.agentId),
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...buildAuthHeaders(),
        },
        body: JSON.stringify(body),
        signal,
      },
    ),

  getHistory: (
    target: ConversationApiTarget,
    chatId: string,
    options?: GetChatOptions,
  ) =>
    target.scope === "project"
      ? projectChatsApi.getChat(target.projectId, chatId, options)
      : chatApi.getChat(chatId, target.agentId, options),

  updateModel: (
    target: ConversationApiTarget,
    chatId: string,
    model: ModelSlot,
  ) =>
    target.scope === "project"
      ? projectChatsApi.updateChat(target.projectId, chatId, { model })
      : chatApi.updateChat(chatId, { model }, target.agentId),

  getRunningConfig: (target: ConversationApiTarget, chatId: string) =>
    target.scope === "project"
      ? projectChatsApi.getRunningConfig(target.projectId, chatId)
      : chatApi.getRunningConfig(chatId, target.agentId),

  updateRunningConfig: (
    target: ConversationApiTarget,
    chatId: string,
    config: ChatRunningConfig,
  ) =>
    target.scope === "project"
      ? projectChatsApi.updateRunningConfig(target.projectId, chatId, config)
      : chatApi.updateRunningConfig(chatId, config, target.agentId),

  persistAcpHistoryPreview: (
    target: ConversationApiTarget,
    chatId: string,
    body: AcpHistoryPreviewRequest,
  ) =>
    target.scope === "project"
      ? projectChatsApi.persistAcpHistoryPreview(target.projectId, chatId, body)
      : chatApi.persistAcpHistoryPreview(chatId, body, target.agentId),

  stopChat: (
    target: ConversationApiTarget,
    chatId: string,
    body: StopChatRequest,
  ) =>
    target.scope === "project"
      ? projectChatsApi.stopChat(target.projectId, chatId, body)
      : chatApi.stopChat(chatId, body, target.agentId),

  selectVariant: (
    target: ConversationApiTarget,
    chatId: string,
    turnId: string,
    variantId: string,
  ) =>
    target.scope === "project"
      ? projectChatsApi.selectVariant(
          target.projectId,
          chatId,
          turnId,
          variantId,
        )
      : chatApi.selectVariant(chatId, turnId, variantId, target.agentId),

  rollback: (
    target: ConversationApiTarget,
    chatId: string,
    body: { before_turn_id: string },
  ) =>
    target.scope === "project"
      ? projectChatsApi.rollback(target.projectId, chatId, body)
      : chatApi.rollback(chatId, body, target.agentId),

  getProjectionExclusions: (target: ConversationApiTarget, chatId: string) =>
    target.scope === "project"
      ? projectChatsApi.getProjectionExclusions(target.projectId, chatId)
      : chatApi.getProjectionExclusions(chatId, target.agentId),

  updateProjectionExclusions: (
    target: ConversationApiTarget,
    chatId: string,
    messageIds: string[],
  ) =>
    target.scope === "project"
      ? projectChatsApi.updateProjectionExclusions(
          target.projectId,
          chatId,
          messageIds,
        )
      : chatApi.updateProjectionExclusions(chatId, messageIds, target.agentId),

  branch: (
    target: ConversationApiTarget,
    chatId: string,
    body: { before_turn_id: string },
  ) =>
    target.scope === "project"
      ? projectChatsApi.branch(target.projectId, chatId, body)
      : chatApi.branch(chatId, body, target.agentId),

  sendNowQueuedInput: (
    target: ConversationApiTarget,
    chatId: string,
    inputId: string,
    body: QueueInputSendNowRequest,
  ) =>
    target.scope === "project"
      ? projectChatsApi.sendNowQueuedInput(
          target.projectId,
          chatId,
          inputId,
          body,
        )
      : chatApi.sendNowQueuedInput(chatId, inputId, body, target.agentId),

  deleteQueuedInput: (
    target: ConversationApiTarget,
    chatId: string,
    inputId: string,
  ) =>
    target.scope === "project"
      ? projectChatsApi.deleteQueuedInput(target.projectId, chatId, inputId)
      : chatApi.deleteQueuedInput(chatId, inputId, target.agentId),

  editQueuedInput: (
    target: ConversationApiTarget,
    chatId: string,
    inputId: string,
    message: string,
  ) =>
    target.scope === "project"
      ? projectChatsApi.editQueuedInput(
          target.projectId,
          chatId,
          inputId,
          message,
        )
      : chatApi.editQueuedInput(chatId, inputId, message, target.agentId),

  getApprovalSnapshot: (target: ConversationApiTarget, chatId: string) =>
    target.scope === "project"
      ? projectChatsApi.getApprovalSnapshot(target.projectId, chatId)
      : consoleApi.getApprovalSnapshot(target.agentId, chatId),

  decideApproval: (
    target: ConversationApiTarget,
    chatId: string,
    approvalId: string,
    body: ApprovalDecisionRequest,
  ) =>
    target.scope === "project"
      ? projectChatsApi.decideApproval(
          target.projectId,
          chatId,
          approvalId,
          body,
        )
      : consoleApi.decideApproval(target.agentId, approvalId, body),

  listAcpRunners: (
    target: ConversationApiTarget,
    chatId: string,
    targetDeviceId?: string | null,
  ) =>
    target.scope === "project"
      ? projectChatsApi.listAcpRunners(target.projectId, chatId)
      : acpApi.listChatRunners(target.agentId, chatId, targetDeviceId),

  listAcpSessions: (
    target: ConversationApiTarget,
    chatId: string,
    runner: string,
    cwd?: string,
    targetDeviceId?: string | null,
  ) =>
    target.scope === "project"
      ? projectChatsApi.listAcpSessions(target.projectId, chatId, runner, cwd)
      : acpApi.listChatSessions(
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
  ) =>
    target.scope === "project"
      ? projectChatsApi.enterAcp(target.projectId, chatId, body)
      : acpApi.enterChat(target.agentId, chatId, body),

  exitAcp: (
    target: ConversationApiTarget,
    chatId: string,
    targetDeviceId?: string | null,
  ) =>
    target.scope === "project"
      ? projectChatsApi.exitAcp(target.projectId, chatId)
      : acpApi.exitChat(target.agentId, chatId, targetDeviceId),

  getAcpStatus: (
    target: ConversationApiTarget,
    chatId: string,
    targetDeviceId?: string | null,
  ) =>
    target.scope === "project"
      ? projectChatsApi.getAcpStatus(target.projectId, chatId)
      : acpApi.chatStatus(target.agentId, chatId, targetDeviceId),

  getAcpPermissionSnapshot: (target: ConversationApiTarget, chatId: string) =>
    target.scope === "project"
      ? projectChatsApi.getAcpPermissionSnapshot(target.projectId, chatId)
      : acpApi.getPermissionSnapshot(target.agentId, chatId),

  decideAcpPermission: (
    target: ConversationApiTarget,
    chatId: string,
    requestId: string,
    optionId: string,
  ) =>
    target.scope === "project"
      ? projectChatsApi.decideAcpPermission(
          target.projectId,
          chatId,
          requestId,
          optionId,
        )
      : acpApi.decidePermission(target.agentId, chatId, requestId, optionId),
};
