import type { ApprovalRequest, ApprovalSnapshot } from "@/api/modules/console";
import type {
  ConversationAssistantResponse,
  ConversationOutputMessage,
} from "./projection";

export function dataAt(msg: ConversationOutputMessage, index: number): unknown {
  return msg.content[index]?.data ?? msg.content[index];
}

export function toolCallId(msg: ConversationOutputMessage): string | null {
  const direct =
    (typeof msg.tool_call_id === "string" && msg.tool_call_id) ||
    (typeof msg.call_id === "string" && msg.call_id);
  if (direct) return direct;
  for (const part of msg.content) {
    const nested =
      (typeof part.tool_call_id === "string" && part.tool_call_id) ||
      (typeof part.call_id === "string" && part.call_id);
    if (nested) return nested;
    const data = part.data;
    if (data && typeof data === "object") {
      const record = data as Record<string, unknown>;
      const dataId =
        (typeof record.tool_call_id === "string" && record.tool_call_id) ||
        (typeof record.call_id === "string" && record.call_id) ||
        (typeof record.id === "string" && record.id);
      if (dataId) return dataId;
    }
    if (typeof part.id === "string" && part.id) return part.id;
  }
  return typeof msg.id === "string" && msg.id ? msg.id : null;
}

export function approvalDataFromMessage(
  msg: ConversationOutputMessage,
): Record<string, unknown> {
  for (const part of msg.content) {
    if (part.type === "tool_approval_request") {
      const data = part.data;
      if (data && typeof data === "object") {
        return data as Record<string, unknown>;
      }
      return part as Record<string, unknown>;
    }
  }
  const first = dataAt(msg, 0);
  return first && typeof first === "object"
    ? (first as Record<string, unknown>)
    : {};
}

export function toolApprovalDataFromMessage(
  msg: ConversationOutputMessage,
): Record<string, unknown> {
  const first = dataAt(msg, 0);
  const record =
    first && typeof first === "object"
      ? (first as Record<string, unknown>)
      : {};
  const approval = record.approval;
  return approval && typeof approval === "object"
    ? (approval as Record<string, unknown>)
    : {};
}

export function approvalRecordFromAuthorityRequest(
  request: ApprovalRequest,
): Record<string, unknown> {
  return {
    id: request.id,
    approval_request_id: request.id,
    call_id: request.tool_call_id,
    tool_call_id: request.tool_call_id,
    name: request.tool_name,
    tool_name: request.tool_name,
    arguments: request.tool_call.args,
    prompt: request.tool_call.display,
    trigger: request.trigger,
    status: request.status,
    decided_at: request.decided_at,
    decided_by: request.decided_by,
    reuse_options: request.reuse_options,
  };
}

export function approvalSnapshotForAssistant(
  item: ConversationAssistantResponse,
  snapshot: ApprovalSnapshot | undefined,
  isLastAssistant: boolean,
): ApprovalSnapshot | undefined {
  if (!snapshot) return undefined;
  const toolCallIds = new Set(
    item.output.map((message) => toolCallId(message)).filter(Boolean),
  );
  const belongsToItem = (request: ApprovalRequest) => {
    if (request.tool_call_id && toolCallIds.has(request.tool_call_id)) {
      return true;
    }
    if (request.turn_id && item.turn_id) {
      return request.turn_id === item.turn_id;
    }
    return isLastAssistant;
  };
  const pending = snapshot.pending.filter(belongsToItem);
  const resolved = snapshot.resolved.filter(belongsToItem);
  if (pending.length === 0 && resolved.length === 0) return undefined;
  return { pending, resolved };
}

export function mergeToolMessagesForApproval(
  messages: ConversationOutputMessage[],
  approvalSnapshot: ApprovalSnapshot | undefined,
  askUserRequestFromToolCall: (
    msg: ConversationOutputMessage,
  ) => ConversationOutputMessage | null,
): ConversationOutputMessage[] {
  const outputByCallId = new Map<string, ConversationOutputMessage>();
  const approvalByCallId = new Map<string, ConversationOutputMessage>();
  const authorityApprovalByCallId = new Map<string, ApprovalRequest>();
  const toolCallIds = new Set<string>();
  for (const request of [
    ...(approvalSnapshot?.pending || []),
    ...(approvalSnapshot?.resolved || []),
  ]) {
    if (request.tool_call_id) {
      authorityApprovalByCallId.set(request.tool_call_id, request);
    }
  }
  const hasAskUserRequest = messages.some(
    (msg) => msg.type === "ask_user_request",
  );
  for (const msg of messages) {
    if (msg.type === "plugin_call" || msg.type === "mcp_call") {
      const id = toolCallId(msg);
      if (id) toolCallIds.add(id);
    }
    if (msg.type === "plugin_call_output" || msg.type === "mcp_call_output") {
      const id = toolCallId(msg);
      if (id) outputByCallId.set(id, msg);
    }
    if (msg.type === "tool_approval_request") {
      const data = approvalDataFromMessage(msg);
      const callId = typeof data.call_id === "string" ? data.call_id : "";
      if (callId) approvalByCallId.set(callId, msg);
    }
  }

  const merged: ConversationOutputMessage[] = [];
  const consumedOutputIds = new Set<string>();
  const consumedApprovalIds = new Set<string>();
  for (const msg of messages) {
    if (msg.type === "plugin_call" || msg.type === "mcp_call") {
      const id = toolCallId(msg);
      const output = id ? outputByCallId.get(id) : undefined;
      const approval = id ? approvalByCallId.get(id) : undefined;
      const authorityApproval = id
        ? authorityApprovalByCallId.get(id)
        : undefined;
      const approvalRecord = authorityApproval
        ? approvalRecordFromAuthorityRequest(authorityApproval)
        : approval
        ? approvalDataFromMessage(approval)
        : null;
      const askUserRequest = askUserRequestFromToolCall(msg);
      if (askUserRequest) {
        if (output) consumedOutputIds.add(output.id);
        if (!hasAskUserRequest) merged.push(askUserRequest);
        continue;
      }
      const baseContent =
        approvalRecord && msg.content[0]
          ? [
              {
                ...msg.content[0],
                data: {
                  ...(msg.content[0].data &&
                  typeof msg.content[0].data === "object"
                    ? (msg.content[0].data as Record<string, unknown>)
                    : {}),
                  approval: approvalRecord,
                },
              },
              ...msg.content.slice(1),
            ]
          : msg.content;
      if (approval) consumedApprovalIds.add(approval.id);
      if (output) {
        consumedOutputIds.add(output.id);
        merged.push({
          ...msg,
          content: [...baseContent, ...output.content],
        });
        continue;
      }
      if (approvalRecord) {
        merged.push({
          ...msg,
          status: "approval_pending",
          content: baseContent,
        });
        continue;
      }
    }
    if (msg.type === "plugin_call_output" || msg.type === "mcp_call_output") {
      const id = toolCallId(msg);
      if (consumedOutputIds.has(msg.id) || (id && toolCallIds.has(id))) {
        continue;
      }
    }
    if (
      msg.type === "tool_approval_request" &&
      consumedApprovalIds.has(msg.id)
    ) {
      continue;
    }
    if (msg.type === "tool_approval_request") {
      continue;
    }
    merged.push(msg);
  }
  return merged;
}
