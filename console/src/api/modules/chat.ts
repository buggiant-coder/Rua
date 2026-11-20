import { withTransferSlot } from "../transferAdmission";
import { request } from "../request";
import { getApiUrl, getApiToken } from "../config";
import { buildAuthHeaders, getSelectedAgentId } from "../authHeaders";
import type {
  ChatSpec,
  ChatTag,
  ChatHistory,
  ChatDeleteResponse,
  ChatUpdateRequest,
  ConversationAuthorityPlacementUpdateRequest,
  AcpHistoryPreviewRequest,
  AcpHistoryPreviewResponse,
  QueueInputDeleteResponse,
  QueueInputEditResponse,
  QueueInputSendNowRequest,
  QueueInputSendNowResponse,
  ChatSearchResponse,
  ModelSlot,
  ChatRunningConfig,
} from "../types";
import type { ChatListSort } from "../types/chatListSort";

export type ChatMediaUploadContext = {
  agentId?: string;
  projectId?: string | null;
  model?: ModelSlot | null;
};

export interface ChatUploadResponse {
  media_ref: string;
  media_id: string;
  mime_type: string;
  byte_size: number;
  authority_epoch?: string;
  original_filename?: string;
}

function chatMediaPath(chatId: string): string {
  return `/chats/${encodeURIComponent(chatId)}/media`;
}

const FILES_PREVIEW = "/files/preview";

export type GetChatOptions = {
  turnLimit?: number;
  beforeTurnOrdinal?: number;
  aroundTurnId?: string;
};

export type StopChatRequest = { turn_id?: string; authority_epoch: string };

export type StopChatResult = {
  stopped: boolean;
  mode?: "remote_control";
  control?: "remote_stop";
  detail?: string;
};

export type LlmDebugCaptureStatus = {
  active: boolean;
  capture_id?: string;
  path?: string;
};

function chatHistoryPath(chatId: string, options?: GetChatOptions): string {
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
  return `/chats/${encodeURIComponent(chatId)}${query ? `?${query}` : ""}`;
}

export const chatApi = {
  getRunningConfig: (chatId: string, _agentId?: string) =>
    request<ChatRunningConfig>(
      `/chats/${encodeURIComponent(chatId)}/running-config`,
    ),

  updateRunningConfig: (
    chatId: string,
    config: ChatRunningConfig,
    _agentId?: string,
  ) =>
    request<ChatRunningConfig>(
      `/chats/${encodeURIComponent(chatId)}/running-config`,
      {
        method: "PUT",
        body: JSON.stringify(config),
      },
    ),

  getLlmDebugCapture: (chatId: string, _agentId?: string) =>
    request<LlmDebugCaptureStatus>(
      `/chats/${encodeURIComponent(chatId)}/llm-debug-capture`,
    ),

  startLlmDebugCapture: (chatId: string, _agentId?: string) =>
    request<LlmDebugCaptureStatus>(
      `/chats/${encodeURIComponent(chatId)}/llm-debug-capture`,
      { method: "POST" },
    ),

  stopLlmDebugCapture: (chatId: string, _agentId?: string) =>
    request<LlmDebugCaptureStatus>(
      `/chats/${encodeURIComponent(chatId)}/llm-debug-capture`,
      { method: "DELETE" },
    ),

  submitPath: (chatId: string, _agentId?: string) =>
    `/chats/${encodeURIComponent(chatId)}/console/submit`,

  livePath: (chatId: string, _agentId?: string) =>
    `/chats/${encodeURIComponent(chatId)}/console/live`,

  uploadFile: async (
    chatId: string,
    file: File,
    context?: ChatMediaUploadContext,
  ): Promise<ChatUploadResponse> => {
    return withTransferSlot(async () => {
      const formData = new FormData();
      const agentId = context?.agentId;
      const projectId = context?.projectId;
      if (agentId) formData.append("agent_id", agentId);
      if (projectId) formData.append("project_id", projectId);
      if (context?.model)
        formData.append("model", JSON.stringify(context.model));
      formData.append("file", file);
      const response = await fetch(getApiUrl(chatMediaPath(chatId)), {
        method: "POST",
        headers: buildAuthHeaders(),
        body: formData,
      });
      if (!response.ok) {
        const text = await response.text().catch(() => "");
        throw new Error(
          `Upload failed: ${response.status} ${response.statusText}${
            text ? ` - ${text}` : ""
          }`,
        );
      }
      return response.json();
    });
  },

  mediaUrl: (chatId: string, mediaId: string): string => {
    const url = getApiUrl(
      `${chatMediaPath(chatId)}/${encodeURIComponent(mediaId)}`,
    );
    const token = getApiToken();
    return token ? `${url}?token=${encodeURIComponent(token)}` : url;
  },

  filePreviewUrl: (filename: string): string => {
    if (!filename) return "";
    if (filename.startsWith("http://") || filename.startsWith("https://"))
      return filename;
    const path = `${FILES_PREVIEW}/${filename.replace(/^\/+/, "")}`;
    const url = getApiUrl(path);

    const token = getApiToken();
    if (token) {
      return `${url}?token=${encodeURIComponent(token)}`;
    }

    return url;
  },
  localImagePreviewUrl: (
    source: string,
    agentId: string = getSelectedAgentId(),
  ): string => {
    if (!source) return "";
    const params = new URLSearchParams({ path: source });
    const token = getApiToken();
    if (token) params.set("token", token);
    return getApiUrl(
      `/agents/${encodeURIComponent(
        agentId,
      )}${FILES_PREVIEW}?${params.toString()}`,
    );
  },
  listChats: async (
    params?: {
      user_id?: string;
      channel?: string;
      search?: string;
      tag?: ChatTag;
      offset?: number;
      limit?: number;
      view?: "user" | "collaboration" | "all";
      archived?: boolean;
      source_chat_id?: string;
      executor?: string;
      status?: string;
    } & Partial<ChatListSort>,
    agentId?: string,
  ): Promise<{ data: ChatSpec[]; total: number }> => {
    const searchParams = new URLSearchParams();
    if (agentId) searchParams.append("agent_id", agentId);
    if (params?.user_id) searchParams.append("user_id", params.user_id);
    if (params?.channel) searchParams.append("channel", params.channel);
    if (params?.search?.trim())
      searchParams.append("search", params.search.trim());
    if (params?.tag) searchParams.append("tag", params.tag);
    if (params?.offset != null)
      searchParams.append("offset", String(params.offset));
    if (params?.limit != null)
      searchParams.append("limit", String(params.limit));
    if (params?.view) searchParams.append("view", params.view);
    if (params?.archived != null)
      searchParams.append("archived", String(params.archived));
    if (params?.source_chat_id)
      searchParams.append("source_chat_id", params.source_chat_id);
    if (params?.executor) searchParams.append("executor", params.executor);
    if (params?.status) searchParams.append("status", params.status);
    if (params?.sort_by) searchParams.append("sort_by", params.sort_by);
    if (params?.sort_order)
      searchParams.append("sort_order", params.sort_order);
    const query = searchParams.toString();
    const response = await fetch(
      getApiUrl(`/chats${query ? `?${query}` : ""}`),
      {
        headers: buildAuthHeaders(),
      },
    );
    if (!response.ok)
      throw new Error(`Failed to list chats: ${response.status}`);
    const data: ChatSpec[] = await response.json();
    const total = parseInt(response.headers.get("X-Total-Count") || "0", 10);
    return { data, total };
  },

  createChat: (
    chat: Partial<ChatSpec>,
    agentId: string = getSelectedAgentId(),
  ) =>
    request<ChatSpec>(`/chats`, {
      method: "POST",
      body: JSON.stringify({ ...chat, agent_id: agentId }),
    }),

  getChat: (chatId: string, options?: GetChatOptions) =>
    request<ChatHistory>(chatHistoryPath(chatId, options)),

  getMetadata: (
    chatId: string,
    _agentId?: string,
    options?: { signal?: AbortSignal },
  ) =>
    request<ChatSpec>(`/chats/${encodeURIComponent(chatId)}/metadata`, options),

  updateChat: (chatId: string, chat: ChatUpdateRequest, _agentId?: string) =>
    request<ChatSpec>(`/chats/${encodeURIComponent(chatId)}`, {
      method: "PUT",
      body: JSON.stringify(chat),
    }),

  selectVariant: (
    chatId: string,
    turnId: string,
    variantId: string,
    _agentId?: string,
  ) =>
    request<ChatSpec>(`/chats/${encodeURIComponent(chatId)}`, {
      method: "PUT",
      body: JSON.stringify({
        turn_id: turnId,
        selected_variant_id: variantId,
      }),
    }),

  rollback: (
    chatId: string,
    body: { before_turn_id: string },
    _agentId?: string,
  ) =>
    request<{ chat: ChatSpec; cancelled_tasks: number }>(
      `/chats/${encodeURIComponent(chatId)}/rollback`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  getProjectionExclusions: (chatId: string, _agentId?: string) =>
    request<{ message_ids: string[] }>(
      `/chats/${encodeURIComponent(chatId)}/projection-exclusions`,
    ),

  updateProjectionExclusions: (
    chatId: string,
    messageIds: string[],
    _agentId?: string,
  ) =>
    request<{ message_ids: string[] }>(
      `/chats/${encodeURIComponent(chatId)}/projection-exclusions`,
      {
        method: "PUT",
        body: JSON.stringify({ message_ids: messageIds }),
      },
    ),

  branch: (
    chatId: string,
    body: { before_turn_id: string },
    _agentId?: string,
  ) =>
    request<{ chat: ChatSpec }>(`/chats/${encodeURIComponent(chatId)}/branch`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  setAuthorityPlacement: (
    chatId: string,
    deviceId: string,
    _agentId?: string,
    model?: ModelSlot,
  ) =>
    request<ChatSpec>(
      `/chats/${encodeURIComponent(chatId)}/authority-placement`,
      {
        method: "PUT",
        body: JSON.stringify({
          device_id: deviceId,
          model,
        } satisfies ConversationAuthorityPlacementUpdateRequest),
      },
    ),

  persistAcpHistoryPreview: (
    chatId: string,
    body: AcpHistoryPreviewRequest,
    _agentId?: string,
  ) =>
    request<AcpHistoryPreviewResponse>(
      `/chats/${encodeURIComponent(chatId)}/acp-history-preview`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  deleteQueuedInput: (chatId: string, inputId: string, _agentId?: string) =>
    request<QueueInputDeleteResponse>(
      `/chats/${encodeURIComponent(chatId)}/queue/${encodeURIComponent(
        inputId,
      )}`,
      {
        method: "DELETE",
      },
    ),

  editQueuedInput: (
    chatId: string,
    inputId: string,
    message: string,
    _agentId?: string,
  ) =>
    request<QueueInputEditResponse>(
      `/chats/${encodeURIComponent(chatId)}/queue/${encodeURIComponent(
        inputId,
      )}`,
      {
        method: "PATCH",
        body: JSON.stringify({ message }),
      },
    ),

  sendNowQueuedInput: (
    chatId: string,
    inputId: string,
    body: QueueInputSendNowRequest,
    _agentId?: string,
  ) =>
    request<QueueInputSendNowResponse>(
      `/chats/${encodeURIComponent(chatId)}/queue/${encodeURIComponent(
        inputId,
      )}/send-now`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  deleteChat: (chatId: string, _agentId?: string) =>
    request<ChatDeleteResponse>(`/chats/${encodeURIComponent(chatId)}`, {
      method: "DELETE",
    }),

  updatePinOrder: (
    items: { id: string; pin_order: number }[],
    _agentId?: string,
  ) =>
    request<{ success: boolean }>(`/chats/pin-order`, {
      method: "PUT",
      body: JSON.stringify(items),
    }),

  batchDeleteChats: (chatIds: string[], _agentId?: string) =>
    request<{ success: boolean; deleted_count: number }>(
      `/chats/batch-delete`,
      {
        method: "POST",
        body: JSON.stringify(chatIds),
      },
    ),

  stopChat: (chatId: string, body: StopChatRequest, _agentId?: string) =>
    request<StopChatResult>(
      `/chats/${encodeURIComponent(chatId)}/console/stop`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),

  searchChats: (query: string, agentId?: string) =>
    request<ChatSearchResponse>(
      `/chats/search?q=${encodeURIComponent(query)}${
        agentId ? `&agent_id=${encodeURIComponent(agentId)}` : ""
      }`,
    ),
};
