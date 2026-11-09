import type { ConversationControlView, Message } from "@/api/types";
import type {
  ConversationOutputMessage,
  TokenUsageSnapshot,
} from "./projection";
import { isVisibleRuntimeNote, normalizeContentParts } from "./projection";

export type ConversationStreamEvent =
  | { kind: "response_started"; raw: Record<string, unknown> }
  | {
      kind: "output_updated";
      output: ConversationOutputMessage[];
      raw: Record<string, unknown>;
    }
  | {
      kind: "message_updated";
      message: ConversationOutputMessage;
      raw: Record<string, unknown>;
    }
  | {
      kind: "content_updated";
      msgId: string;
      contentType: string;
      index: number;
      isDelta: boolean;
      status?: string;
      text?: string;
      data?: unknown;
      raw: Record<string, unknown>;
    }
  | {
      kind: "text_delta";
      messageId: string;
      delta: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "thinking_delta";
      messageId: string;
      delta: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "model_call_reset";
      modelCallId: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "tool_call_delta";
      messageId: string;
      id: string;
      index: number;
      name?: string;
      argumentsDelta: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "tool_call";
      messageId: string;
      id: string;
      name: string;
      arguments: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "tool_result_delta";
      messageId: string;
      id: string;
      name?: string;
      outputDelta: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "tool_result";
      messageId: string;
      id: string;
      name?: string;
      content: unknown;
      raw: Record<string, unknown>;
    }
  | {
      kind: "user_message";
      message: Message;
      raw: Record<string, unknown>;
    }
  | {
      kind: "usage";
      usage: TokenUsageSnapshot;
      raw: Record<string, unknown>;
    }
  | {
      kind: "completed";
      output: ConversationOutputMessage[];
      raw: Record<string, unknown>;
    }
  | {
      kind: "failed";
      error: unknown;
      output: ConversationOutputMessage[];
      raw: Record<string, unknown>;
    }
  | {
      kind: "interrupted";
      output: ConversationOutputMessage[];
      reason?: string;
      raw?: Record<string, unknown>;
    }
  | {
      kind: "runtime_note";
      output: ConversationOutputMessage;
      raw: Record<string, unknown>;
    }
  | {
      kind: "device_delivery_accepted";
      chatId: string;
      deviceDeliveryId: string;
      targetDeviceId: string;
      deviceDeliverySequence: number;
      acceptedAt?: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "turn_terminal";
      turnId: string;
      status: "completed" | "failed" | "interrupted" | "interjected";
      deviceDeliveryId?: string;
      authorityDeviceId?: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "context_compaction";
      status: string;
      mode?: string;
      message?: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "conversation_control";
      control: ConversationControlView;
      reason?: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "history_watermark_reached";
      agentId?: string;
      chatId?: string;
      authorityDeviceId?: string;
      durableSequence: number;
      manifestHash: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "history_refresh_required";
      reason: "live_activity_gap";
      chatId?: string;
      requestedAfterSequence: number;
      firstAvailableSequence: number;
      raw: Record<string, unknown>;
    }
  | {
      kind: "history_mutated";
      action: "variant_selected";
      turnId: string;
      selectedVariantId: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "history_mutated";
      action: "rolled_back";
      beforeTurnId: string;
      selectedTurnId?: string;
      selectedVariantId?: string;
      raw: Record<string, unknown>;
    }
  | {
      kind: "stream_replay";
      events: ConversationStreamEvent[];
      complete: boolean;
      raw: Record<string, unknown>;
    }
  | { kind: "parse_error"; error: Error; raw: string }
  | { kind: "eof_without_terminal" };

export type ConversationStreamFold = {
  status: "idle" | "generating" | "completed" | "failed" | "interrupted";
  output: ConversationOutputMessage[];
  error: unknown;
  sawTerminal: boolean;
  terminalTurnId: string | null;
  turnId?: string | null;
  usage: TokenUsageSnapshot | null;
};

export const initialConversationStreamFold: ConversationStreamFold = {
  status: "idle",
  output: [],
  error: null,
  sawTerminal: false,
  terminalTurnId: null,
  usage: null,
};

const TURN_ABORTED_TEXT =
  "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>";
const TURN_INTERJECTED_TEXT =
  "<turn_interjected>\nThe user inserted a new message, so the previous turn was ended before it could continue.\n</turn_interjected>";

type RawOutputMessage = Record<string, unknown> & {
  id?: string;
  role?: string;
  type?: string;
  content?: unknown;
};

function toOutputMessage(raw: RawOutputMessage): ConversationOutputMessage {
  if (typeof raw.id !== "string" || !raw.id) {
    throw new Error("message output is missing canonical message_id");
  }
  const runtimeNote =
    raw.runtime_note as ConversationOutputMessage["runtime_note"];
  return {
    ...raw,
    id: raw.id,
    role: raw.role || "assistant",
    type: runtimeNote ? "runtime_note" : raw.type,
    content: normalizeContentParts(raw.content),
    runtime_note: runtimeNote,
  } as ConversationOutputMessage;
}

function extractOutput(
  payload: Record<string, unknown>,
): ConversationOutputMessage[] {
  if (!Array.isArray(payload.output)) return [];
  return (payload.output as RawOutputMessage[]).map(toOutputMessage);
}

function approvalRequestMessage(
  payload: Record<string, unknown>,
): ConversationOutputMessage {
  const requestId =
    stringField(payload, "request_id") ||
    stringField(payload, "id") ||
    stringField(payload, "call_id") ||
    "approval";
  const toolName =
    stringField(payload, "tool_name") || stringField(payload, "name");
  const content = Array.isArray(payload.content)
    ? payload.content
    : [
        {
          type: "tool_approval_request",
          data: {
            id: requestId,
            approval_request_id: requestId,
            call_id: stringField(payload, "call_id"),
            name: toolName || "Tool",
            arguments: payload.arguments ?? null,
            prompt: payload.prompt ?? payload.reason ?? null,
            status: "waiting",
          },
        },
      ];
  return toOutputMessage({
    id: requestId,
    role: "assistant",
    type: "tool_approval_request",
    status: "in_progress",
    content,
  });
}

function askUserRequestMessage(
  payload: Record<string, unknown>,
): ConversationOutputMessage {
  const requestId =
    stringField(payload, "request_id") ||
    stringField(payload, "id") ||
    "ask-user";
  const content = Array.isArray(payload.content)
    ? payload.content
    : [
        {
          type: "ask_user_request",
          data: {
            id: requestId,
            request_id: requestId,
            prompt: payload.prompt ?? null,
            questions: Array.isArray(payload.questions)
              ? payload.questions
              : [],
            status: "waiting",
          },
        },
      ];
  return toOutputMessage({
    id: requestId,
    role: "assistant",
    type: "ask_user_request",
    status: "in_progress",
    content,
  });
}

function streamUserMessage(payload: Record<string, unknown>): Message {
  const raw = payload.message;
  if (raw && typeof raw === "object" && !Array.isArray(raw)) {
    const message = raw as Message;
    if (typeof message.id !== "string" || !message.id) {
      throw new Error("user message is missing canonical message_id");
    }
    return {
      ...message,
      role: typeof message.role === "string" ? message.role : "user",
      content: message.content ?? [],
    };
  }
  throw new Error("user message is missing canonical message_id");
}

function normalizeTerminalOutput(
  output: ConversationOutputMessage[],
): ConversationOutputMessage[] {
  return output;
}

function markOutputStatus(
  output: ConversationOutputMessage[],
  status: string,
): ConversationOutputMessage[] {
  return output.map((message) =>
    message.status === "completed" ? message : { ...message, status },
  );
}

function stringField(
  payload: Record<string, unknown>,
  key: string,
): string | undefined {
  const value = payload[key];
  return typeof value === "string" ? value : undefined;
}

function numberField(payload: Record<string, unknown>, key: string): number {
  const value = payload[key];
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function terminalStatus(
  value: string | undefined,
): "completed" | "failed" | "interrupted" | "interjected" {
  if (
    value === "failed" ||
    value === "interrupted" ||
    value === "interjected"
  ) {
    return value;
  }
  return "completed";
}

function optionalNumberField(
  payload: Record<string, unknown>,
  key: string,
): number | undefined {
  const value = payload[key];
  return typeof value === "number" && Number.isFinite(value)
    ? value
    : undefined;
}

function usageFromPayload(
  payload: Record<string, unknown>,
): TokenUsageSnapshot | null {
  const usageSource =
    payload.usage && typeof payload.usage === "object"
      ? (payload.usage as Record<string, unknown>)
      : null;
  const lastCallSource =
    payload.last_call_usage && typeof payload.last_call_usage === "object"
      ? (payload.last_call_usage as Record<string, unknown>)
      : null;
  const promptTokens = usageSource
    ? optionalNumberField(usageSource, "prompt_tokens") ?? 0
    : 0;
  const completionTokens = usageSource
    ? optionalNumberField(usageSource, "completion_tokens") ?? 0
    : 0;
  const totalTokens =
    (usageSource
      ? optionalNumberField(usageSource, "total_tokens")
      : undefined) ?? promptTokens + completionTokens;
  const lastCallPromptTokens = lastCallSource
    ? optionalNumberField(lastCallSource, "prompt_tokens") ?? 0
    : 0;
  const lastCallCompletionTokens = lastCallSource
    ? optionalNumberField(lastCallSource, "completion_tokens") ?? 0
    : 0;
  const lastCallTotalTokens =
    (lastCallSource
      ? optionalNumberField(lastCallSource, "total_tokens")
      : undefined) ?? lastCallPromptTokens + lastCallCompletionTokens;
  if (totalTokens <= 0 && lastCallTotalTokens <= 0) return null;
  return {
    usage:
      totalTokens > 0
        ? {
            prompt_tokens: promptTokens,
            completion_tokens: completionTokens,
            total_tokens: totalTokens,
            cache_read_tokens: usageSource
              ? optionalNumberField(usageSource, "cache_read_tokens")
              : undefined,
            cache_write_tokens: usageSource
              ? optionalNumberField(usageSource, "cache_write_tokens")
              : undefined,
          }
        : null,
    last_call_usage:
      lastCallTotalTokens > 0
        ? {
            prompt_tokens: lastCallPromptTokens,
            completion_tokens: lastCallCompletionTokens,
            total_tokens: lastCallTotalTokens,
            cache_read_tokens: lastCallSource
              ? optionalNumberField(lastCallSource, "cache_read_tokens")
              : undefined,
            cache_write_tokens: lastCallSource
              ? optionalNumberField(lastCallSource, "cache_write_tokens")
              : undefined,
          }
        : null,
    max_input_length: optionalNumberField(payload, "max_input_length") ?? null,
  };
}

function mergeStreamUsage(
  current: TokenUsageSnapshot | null,
  next: TokenUsageSnapshot,
): TokenUsageSnapshot {
  return {
    usage: next.usage || current?.usage || null,
    last_call_usage: next.last_call_usage || current?.last_call_usage || null,
    max_input_length:
      next.max_input_length || current?.max_input_length || null,
  };
}

function booleanField(payload: Record<string, unknown>, key: string): boolean {
  const value = payload[key];
  return typeof value === "boolean" ? value : false;
}

function textFromContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (content == null) return "";
  const text = normalizeContentParts(content)
    .map((part) => {
      if (part.type === "text" && typeof part.text === "string") {
        return part.text;
      }
      if (typeof part.data === "string") return part.data;
      return "";
    })
    .filter(Boolean)
    .join("\n");
  if (text) return text;
  return JSON.stringify(content);
}

function appendTextPart(
  output: ConversationOutputMessage[],
  predicate: (msg: ConversationOutputMessage, index: number) => boolean,
  createMessage: () => ConversationOutputMessage,
  delta: string,
): ConversationOutputMessage[] {
  let index = -1;
  for (let i = output.length - 1; i >= 0; i -= 1) {
    if (predicate(output[i], i)) {
      index = i;
      break;
    }
  }
  const next = [...output];
  if (index < 0) {
    const msg = createMessage();
    next.push({
      ...msg,
      content: [{ type: "text", text: delta }],
    });
    return next;
  }

  const msg = next[index];
  const content = [...msg.content];
  let textIndex = -1;
  for (let i = content.length - 1; i >= 0; i -= 1) {
    const part = content[i];
    if (part.type === "text" && typeof part.text === "string") {
      textIndex = i;
      break;
    }
  }
  if (textIndex < 0) {
    content.push({ type: "text", text: delta });
  } else {
    const part = content[textIndex];
    content[textIndex] = { ...part, text: `${part.text || ""}${delta}` };
  }
  next[index] = { ...msg, content };
  return next;
}

function appendAssistantTextDelta(
  output: ConversationOutputMessage[],
  messageId: string,
  delta: string,
): ConversationOutputMessage[] {
  return appendTextPart(
    output,
    (msg) =>
      msg.id === messageId &&
      msg.role === "assistant" &&
      (msg.type === "message" || !msg.type),
    () => ({
      id: messageId,
      role: "assistant",
      type: "message",
      status: "in_progress",
      content: [],
    }),
    delta,
  );
}

function outputTypeFromContentContext(
  output: ConversationOutputMessage[],
  msgId: string,
): string {
  return output.find((msg) => msg.id === msgId)?.type || "message";
}

function upsertOutputMessage(
  output: ConversationOutputMessage[],
  incoming: ConversationOutputMessage,
): ConversationOutputMessage[] {
  const index = output.findIndex((msg) => msg.id === incoming.id);
  if (index < 0) return [...output, incoming];

  const existing = output[index];
  const next = [...output];
  next[index] = {
    ...existing,
    ...incoming,
    content: incoming.content.length > 0 ? incoming.content : existing.content,
  };
  return next;
}

function withTurnAbortedMarker(
  output: ConversationOutputMessage[],
): ConversationOutputMessage[] {
  if (output.some((msg) => msg.type === "turn_aborted")) {
    return output;
  }
  return [
    ...output,
    {
      id: "stream-turn-aborted",
      role: "system",
      type: "turn_aborted",
      status: "interrupted",
      content: [{ type: "text", text: TURN_ABORTED_TEXT }],
    } as ConversationOutputMessage,
  ];
}

function withTurnInterjectedMarker(
  output: ConversationOutputMessage[],
): ConversationOutputMessage[] {
  if (output.some((msg) => msg.type === "turn_interjected")) {
    return output;
  }
  return [
    ...output,
    {
      id: "stream-turn-interjected",
      role: "system",
      type: "turn_interjected",
      status: "interrupted",
      content: [{ type: "text", text: TURN_INTERJECTED_TEXT }],
    } as ConversationOutputMessage,
  ];
}

function partIndex(part: ConversationOutputMessage["content"][number]): number {
  const value = part.index;
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function upsertTextContent(
  output: ConversationOutputMessage[],
  params: {
    msgId: string;
    outputType: string;
    index: number;
    text: string;
    isDelta: boolean;
    status?: string;
  },
): ConversationOutputMessage[] {
  const messageIndex = output.findIndex((msg) => msg.id === params.msgId);
  const base: ConversationOutputMessage =
    messageIndex >= 0
      ? output[messageIndex]
      : ({
          id: params.msgId,
          role: "assistant",
          type: params.outputType,
          status: params.status || "in_progress",
          content: [],
        } as ConversationOutputMessage);

  const content = [...base.content];
  const textIndex = content.findIndex(
    (part) => part.type === "text" && partIndex(part) === params.index,
  );

  if (textIndex < 0) {
    content.push({
      type: "text",
      text: params.text,
      index: params.index,
      status: params.status,
    });
  } else {
    const part = content[textIndex];
    content[textIndex] = {
      ...part,
      text: params.isDelta ? `${part.text || ""}${params.text}` : params.text,
      status: params.status || part.status,
    };
  }

  const next = [...output];
  const updated = { ...base, status: params.status || base.status, content };
  if (messageIndex < 0) {
    next.push(updated);
  } else {
    next[messageIndex] = updated;
  }
  return next;
}

function upsertDataContent(
  output: ConversationOutputMessage[],
  params: {
    msgId: string;
    data: unknown;
    isDelta: boolean;
    status?: string;
  },
): ConversationOutputMessage[] {
  const messageIndex = output.findIndex((msg) => msg.id === params.msgId);
  if (messageIndex < 0) return output;
  const message = output[messageIndex];
  const incoming =
    params.data &&
    typeof params.data === "object" &&
    !Array.isArray(params.data)
      ? (params.data as Record<string, unknown>)
      : {};
  const content = [...message.content];
  const dataIndex = content.findIndex(
    (part) => part.data && typeof part.data === "object",
  );
  const existing =
    dataIndex >= 0 &&
    content[dataIndex].data &&
    typeof content[dataIndex].data === "object"
      ? (content[dataIndex].data as Record<string, unknown>)
      : {};
  const data = { ...existing, ...incoming };
  if (params.isDelta) {
    for (const field of ["arguments", "output"] as const) {
      const deltaField = `${field}_delta`;
      const delta = incoming[deltaField] ?? incoming[field];
      if (typeof delta === "string") {
        data[field] = `${
          typeof existing[field] === "string" ? existing[field] : ""
        }${delta}`;
      }
      delete data[deltaField];
    }
  }
  const part = {
    ...(dataIndex >= 0 ? content[dataIndex] : {}),
    type: "data",
    data,
    status: params.status,
  };
  if (dataIndex >= 0) {
    content[dataIndex] = part;
  } else {
    content.push(part);
  }

  const next = [...output];
  next[messageIndex] = {
    ...message,
    status: params.status || message.status,
    content,
  };
  return next;
}

function appendThinkingDelta(
  output: ConversationOutputMessage[],
  messageId: string,
  delta: string,
): ConversationOutputMessage[] {
  if (
    delta.trim().length === 0 &&
    output[output.length - 1]?.type !== "reasoning"
  ) {
    return output;
  }
  return appendTextPart(
    output,
    (msg) => msg.id === messageId && msg.type === "reasoning",
    () => ({
      id: messageId,
      role: "assistant",
      type: "reasoning",
      status: "in_progress",
      content: [],
    }),
    delta,
  );
}

function upsertToolCall(
  output: ConversationOutputMessage[],
  params: {
    messageId: string;
    callId: string;
    name?: string;
    argumentsDelta?: string;
    arguments?: string;
  },
): ConversationOutputMessage[] {
  const index = output.findIndex(
    (msg) =>
      msg.type === "plugin_call" &&
      (msg.id === params.messageId ||
        msg.call_id === params.callId ||
        msg.tool_call_id === params.callId ||
        msg.content.some((part) => {
          const data = part.data;
          return (
            data &&
            typeof data === "object" &&
            (data as Record<string, unknown>).id === params.callId
          );
        })),
  );
  const next = [...output];
  const existing = index >= 0 ? next[index] : undefined;
  const existingData =
    existing?.content[0]?.data && typeof existing.content[0].data === "object"
      ? (existing.content[0].data as Record<string, unknown>)
      : {};
  const existingArgs =
    typeof existingData.arguments === "string" ? existingData.arguments : "";
  const args =
    params.arguments ?? `${existingArgs}${params.argumentsDelta ?? ""}`;
  const data = {
    ...existingData,
    id: params.callId,
    name:
      params.name ||
      (typeof existingData.name === "string" ? existingData.name : undefined),
    arguments: args,
    status: "in_progress",
  };
  const msg: ConversationOutputMessage = {
    ...(existing || {}),
    id: params.messageId,
    role: "assistant",
    type: "plugin_call",
    status: "in_progress",
    call_id: params.callId,
    tool_call_id: params.callId,
    content: [{ type: "tool", data }],
  } as ConversationOutputMessage;

  if (index >= 0) {
    next[index] = msg;
  } else {
    next.push(msg);
  }
  return next;
}

function upsertToolResult(
  output: ConversationOutputMessage[],
  params: {
    messageId: string;
    callId: string;
    name?: string;
    outputDelta?: string;
    content?: unknown;
  },
): ConversationOutputMessage[] {
  const index = output.findIndex(
    (msg) =>
      msg.type === "plugin_call_output" &&
      (msg.id === params.messageId ||
        msg.call_id === params.callId ||
        msg.tool_call_id === params.callId ||
        msg.content.some((part) => {
          const data = part.data;
          return (
            data &&
            typeof data === "object" &&
            (data as Record<string, unknown>).id === params.callId
          );
        })),
  );
  const next = [...output];
  const existing = index >= 0 ? next[index] : undefined;
  const existingData =
    existing?.content[0]?.data && typeof existing.content[0].data === "object"
      ? (existing.content[0].data as Record<string, unknown>)
      : {};
  const existingOutput =
    typeof existingData.output === "string" ? existingData.output : "";
  const finalOutput =
    params.content === undefined
      ? `${existingOutput}${params.outputDelta ?? ""}`
      : textFromContent(params.content);
  const data = {
    ...existingData,
    id: params.callId,
    name:
      params.name ||
      (typeof existingData.name === "string" ? existingData.name : undefined),
    output: finalOutput,
    status: params.content === undefined ? "in_progress" : "completed",
  };
  const msg: ConversationOutputMessage = {
    ...(existing || {}),
    id: params.messageId,
    role: "tool",
    type: "plugin_call_output",
    status: data.status,
    call_id: params.callId,
    tool_call_id: params.callId,
    content: [{ type: "tool_result", data }],
  } as ConversationOutputMessage;

  if (index >= 0) {
    next[index] = msg;
  } else {
    next.push(msg);
  }
  return next;
}

function replayEvents(payload: Record<string, unknown>) {
  if (!Array.isArray(payload.events)) return [];
  return payload.events
    .map((event) => {
      const raw = typeof event === "string" ? event : JSON.stringify(event);
      return parseConversationStreamChunk(raw);
    })
    .filter((event) => event.kind !== "stream_replay");
}

export function parseConversationStreamChunk(
  raw: string,
): ConversationStreamEvent {
  let payload: Record<string, unknown>;
  try {
    payload = JSON.parse(raw) as Record<string, unknown>;
  } catch (err) {
    return {
      kind: "parse_error",
      error: err instanceof Error ? err : new Error(String(err)),
      raw,
    };
  }

  let output: ConversationOutputMessage[];
  try {
    output = extractOutput(payload);
  } catch (error) {
    return {
      kind: "parse_error",
      error: error instanceof Error ? error : new Error(String(error)),
      raw,
    };
  }
  const status = payload.status;
  const type = payload.type;
  const finishReason = stringField(payload, "finish_reason");

  if (type === "stream_replay") {
    return {
      kind: "stream_replay",
      events: replayEvents(payload),
      complete: payload.complete === true,
      raw: payload,
    };
  }

  if (payload.object === "response" && finishReason === "interjected") {
    return { kind: "interrupted", output, reason: finishReason, raw: payload };
  }
  if (payload.object === "response" && status === "completed") {
    return { kind: "completed", output, raw: payload };
  }
  if (payload.object === "response" && status === "failed") {
    return {
      kind: "failed",
      error: payload.error || payload,
      output,
      raw: payload,
    };
  }
  if (payload.object === "response" && status === "interrupted") {
    return {
      kind: "interrupted",
      output,
      reason: finishReason || stringField(payload, "reason"),
      raw: payload,
    };
  }
  if (payload.object === "response" && output.length > 0) {
    return { kind: "output_updated", output, raw: payload };
  }
  if (payload.object === "response") {
    return { kind: "response_started", raw: payload };
  }

  if (type === "conversation_control") {
    const control = payload.control;
    if (control && typeof control === "object") {
      return {
        kind: "conversation_control",
        control: control as ConversationControlView,
        reason: stringField(payload, "reason"),
        raw: payload,
      };
    }
  }
  if (type === "history_watermark_reached") {
    const durableSequence = payload.durable_sequence;
    const manifestHash = stringField(payload, "manifest_hash");
    if (
      typeof durableSequence === "number" &&
      Number.isInteger(durableSequence) &&
      durableSequence >= 0 &&
      manifestHash
    ) {
      return {
        kind: "history_watermark_reached",
        agentId: stringField(payload, "agent_id"),
        chatId: stringField(payload, "chat_id"),
        authorityDeviceId: stringField(payload, "authority_device_id"),
        durableSequence,
        manifestHash,
        raw: payload,
      };
    }
  }
  if (type === "history_refresh_required") {
    const reason = stringField(payload, "reason");
    const requestedAfterSequence = payload.requested_after_sequence;
    const firstAvailableSequence = payload.first_available_sequence;
    if (
      reason === "live_activity_gap" &&
      typeof requestedAfterSequence === "number" &&
      Number.isInteger(requestedAfterSequence) &&
      requestedAfterSequence >= 0 &&
      typeof firstAvailableSequence === "number" &&
      Number.isInteger(firstAvailableSequence) &&
      firstAvailableSequence >= 0
    ) {
      return {
        kind: "history_refresh_required",
        reason,
        chatId: stringField(payload, "chat_id"),
        requestedAfterSequence,
        firstAvailableSequence,
        raw: payload,
      };
    }
  }
  if (type === "conversation_history_mutated") {
    const action = stringField(payload, "action");
    if (action === "variant_selected") {
      const turnId = stringField(payload, "turn_id");
      const selectedVariantId = stringField(payload, "selected_variant_id");
      if (turnId && selectedVariantId) {
        return {
          kind: "history_mutated",
          action,
          turnId,
          selectedVariantId,
          raw: payload,
        };
      }
    }
    if (action === "rolled_back") {
      const beforeTurnId = stringField(payload, "before_turn_id");
      if (beforeTurnId) {
        return {
          kind: "history_mutated",
          action,
          beforeTurnId,
          selectedTurnId: stringField(payload, "selected_turn_id"),
          selectedVariantId: stringField(payload, "selected_variant_id"),
          raw: payload,
        };
      }
    }
  }

  if (type === "start") {
    return { kind: "response_started", raw: payload };
  }
  if (type === "text_delta") {
    const messageId = stringField(payload, "message_id");
    if (!messageId) {
      return {
        kind: "parse_error",
        error: new Error("text_delta is missing canonical message_id"),
        raw,
      };
    }
    return {
      kind: "text_delta",
      messageId,
      delta: stringField(payload, "delta") || "",
      raw: payload,
    };
  }
  if (type === "thinking_delta") {
    const messageId = stringField(payload, "message_id");
    if (!messageId) {
      return {
        kind: "parse_error",
        error: new Error("thinking_delta is missing canonical message_id"),
        raw,
      };
    }
    return {
      kind: "thinking_delta",
      messageId,
      delta: stringField(payload, "delta") || "",
      raw: payload,
    };
  }
  if (type === "model_call_reset") {
    const modelCallId = stringField(payload, "model_call_id");
    if (!modelCallId) {
      return {
        kind: "parse_error",
        error: new Error("model_call_reset is missing model_call_id"),
        raw,
      };
    }
    return {
      kind: "model_call_reset",
      modelCallId,
      raw: payload,
    };
  }
  if (type === "tool_call_delta") {
    const index = numberField(payload, "index");
    const messageId = stringField(payload, "message_id");
    const id = stringField(payload, "id");
    if (!messageId || !id) {
      return {
        kind: "parse_error",
        error: new Error(
          "tool_call_delta is missing canonical message_id or tool_call_id",
        ),
        raw,
      };
    }
    return {
      kind: "tool_call_delta",
      messageId,
      id,
      index,
      name: stringField(payload, "name"),
      argumentsDelta: stringField(payload, "arguments_delta") || "",
      raw: payload,
    };
  }
  if (type === "tool_call") {
    const messageId = stringField(payload, "message_id");
    const id = stringField(payload, "id");
    if (!messageId || !id) {
      return {
        kind: "parse_error",
        error: new Error(
          "tool_call is missing canonical message_id or tool_call_id",
        ),
        raw,
      };
    }
    return {
      kind: "tool_call",
      messageId,
      id,
      name: stringField(payload, "name") || "Tool",
      arguments: stringField(payload, "arguments") || "",
      raw: payload,
    };
  }
  if (type === "tool_result_delta") {
    const messageId = stringField(payload, "message_id");
    const id = stringField(payload, "id");
    if (!messageId || !id) {
      return {
        kind: "parse_error",
        error: new Error(
          "tool_result_delta is missing canonical message_id or tool_call_id",
        ),
        raw,
      };
    }
    return {
      kind: "tool_result_delta",
      messageId,
      id,
      name: stringField(payload, "name"),
      outputDelta: stringField(payload, "output_delta") || "",
      raw: payload,
    };
  }
  if (type === "tool_result") {
    const messageId = stringField(payload, "message_id");
    const id = stringField(payload, "id");
    if (!messageId || !id) {
      return {
        kind: "parse_error",
        error: new Error(
          "tool_result is missing canonical message_id or tool_call_id",
        ),
        raw,
      };
    }
    return {
      kind: "tool_result",
      messageId,
      id,
      name: stringField(payload, "name"),
      content: payload.content,
      raw: payload,
    };
  }
  if (type === "user_message") {
    try {
      return {
        kind: "user_message",
        message: streamUserMessage(payload),
        raw: payload,
      };
    } catch (error) {
      return {
        kind: "parse_error",
        error: error instanceof Error ? error : new Error(String(error)),
        raw,
      };
    }
  }
  if (type === "usage") {
    const usage = usageFromPayload(payload);
    if (usage) {
      return { kind: "usage", usage, raw: payload };
    }
  }
  if (type === "tool_approval_request") {
    return {
      kind: "message_updated",
      message: approvalRequestMessage(payload),
      raw: payload,
    };
  }
  if (type === "ask_user_request" || type === "ask_user") {
    return {
      kind: "message_updated",
      message: askUserRequestMessage(payload),
      raw: payload,
    };
  }
  if (type === "device_delivery_accepted") {
    return {
      kind: "device_delivery_accepted",
      chatId: stringField(payload, "chat_id") || "",
      deviceDeliveryId: stringField(payload, "device_delivery_id") || "",
      targetDeviceId: stringField(payload, "target_device_id") || "",
      deviceDeliverySequence: numberField(payload, "device_delivery_sequence"),
      acceptedAt: stringField(payload, "accepted_at"),
      raw: payload,
    };
  }
  if (type === "turn_terminal") {
    return {
      kind: "turn_terminal",
      turnId: stringField(payload, "turn_id") || "",
      status: terminalStatus(stringField(payload, "status")),
      deviceDeliveryId: stringField(payload, "device_delivery_id"),
      authorityDeviceId: stringField(payload, "authority_device_id"),
      raw: payload,
    };
  }
  if (type === "context_compaction") {
    return {
      kind: "context_compaction",
      status: stringField(payload, "status") || "",
      mode: stringField(payload, "mode"),
      message: stringField(payload, "message"),
      raw: payload,
    };
  }
  if (type === "done") {
    const finishReason = stringField(payload, "finish_reason");
    if (finishReason === "interjected") {
      return {
        kind: "interrupted",
        output,
        reason: finishReason,
        raw: payload,
      };
    }
    if (finishReason === "remote_delivery_failed") {
      return {
        kind: "failed",
        error: finishReason,
        output,
        raw: payload,
      };
    }
    return { kind: "completed", output, raw: payload };
  }
  if (type === "error") {
    return {
      kind: "failed",
      error: stringField(payload, "detail") || payload,
      output,
      raw: payload,
    };
  }
  if (type === "interrupted") {
    return {
      kind: "interrupted",
      output,
      reason: stringField(payload, "reason"),
      raw: payload,
    };
  }

  if (payload.object === "message") {
    try {
      return {
        kind: "message_updated",
        message: toOutputMessage(payload as RawOutputMessage),
        raw: payload,
      };
    } catch (error) {
      return {
        kind: "parse_error",
        error: error instanceof Error ? error : new Error(String(error)),
        raw,
      };
    }
  }

  if (payload.object === "content") {
    const msgId = stringField(payload, "msg_id") || "";
    if (msgId) {
      const contentIndex = optionalNumberField(payload, "index") ?? 0;
      return {
        kind: "content_updated",
        msgId,
        contentType: stringField(payload, "type") || "text",
        index: contentIndex,
        isDelta: booleanField(payload, "delta"),
        status: stringField(payload, "status"),
        text: stringField(payload, "text"),
        data: payload.data,
        raw: payload,
      };
    }
  }

  if (
    payload.type === "runtime_note" &&
    typeof payload.runtime_note === "object"
  ) {
    try {
      return {
        kind: "runtime_note",
        output: toOutputMessage(payload as RawOutputMessage),
        raw: payload,
      };
    } catch (error) {
      return {
        kind: "parse_error",
        error: error instanceof Error ? error : new Error(String(error)),
        raw,
      };
    }
  }

  return { kind: "response_started", raw: payload };
}

export function foldConversationStreamEvent(
  state: ConversationStreamFold,
  event: ConversationStreamEvent,
): ConversationStreamFold {
  if (state.sawTerminal) return state;

  switch (event.kind) {
    case "response_started":
      return { ...state, status: "generating" };
    case "output_updated":
      return {
        ...state,
        status: "generating",
        output: event.output.length > 0 ? event.output : state.output,
      };
    case "message_updated":
      if (event.message.type === "tool_approval_request") {
        return state;
      }
      return {
        ...state,
        status:
          event.message.status === "completed" ? state.status : "generating",
        output: upsertOutputMessage(state.output, event.message),
      };
    case "content_updated":
      if (event.contentType === "text") {
        return {
          ...state,
          status: "generating",
          output: upsertTextContent(state.output, {
            msgId: event.msgId,
            outputType: outputTypeFromContentContext(state.output, event.msgId),
            index: event.index,
            text: event.text || "",
            isDelta: event.isDelta,
            status: event.status,
          }),
        };
      }
      if (event.contentType === "data") {
        return {
          ...state,
          status: "generating",
          output: upsertDataContent(state.output, {
            msgId: event.msgId,
            data: event.data,
            isDelta: event.isDelta,
            status: event.status,
          }),
        };
      }
      return state;
    case "text_delta":
      return {
        ...state,
        status: "generating",
        output: appendAssistantTextDelta(
          state.output,
          event.messageId,
          event.delta,
        ),
      };
    case "thinking_delta":
      return {
        ...state,
        status: "generating",
        output: appendThinkingDelta(state.output, event.messageId, event.delta),
      };
    case "model_call_reset": {
      const messageIds = new Set([
        `msg-${event.modelCallId}-text`,
        `msg-${event.modelCallId}-reasoning`,
      ]);
      return {
        ...state,
        status: "generating",
        output: state.output.filter(
          (message) =>
            !messageIds.has(message.id) &&
            !message.id.startsWith(`msg-${event.modelCallId}-text-`) &&
            !(
              message.type === "plugin_call" && message.status === "in_progress"
            ),
        ),
      };
    }
    case "tool_call_delta":
      return {
        ...state,
        status: "generating",
        output: upsertToolCall(state.output, {
          messageId: event.messageId,
          callId: event.id,
          name: event.name,
          argumentsDelta: event.argumentsDelta,
        }),
      };
    case "tool_call":
      return {
        ...state,
        status: "generating",
        output: upsertToolCall(state.output, {
          messageId: event.messageId,
          callId: event.id,
          name: event.name,
          arguments: event.arguments,
        }),
      };
    case "tool_result_delta":
      return {
        ...state,
        status: "generating",
        output: upsertToolResult(state.output, {
          messageId: event.messageId,
          callId: event.id,
          name: event.name,
          outputDelta: event.outputDelta,
        }),
      };
    case "tool_result":
      return {
        ...state,
        status: "generating",
        output: upsertToolResult(state.output, {
          messageId: event.messageId,
          callId: event.id,
          name: event.name,
          content: event.content,
        }),
      };
    case "runtime_note":
      if (!isVisibleRuntimeNote(event.output.runtime_note)) {
        return state;
      }
      return {
        ...state,
        status: state.status === "idle" ? "generating" : state.status,
        output: [...state.output, event.output],
      };
    case "device_delivery_accepted":
      return state;
    case "usage":
      return {
        ...state,
        usage: mergeStreamUsage(state.usage, event.usage),
      };
    case "turn_terminal":
      if (event.status === "failed") {
        return {
          ...state,
          status: "failed",
          output: markOutputStatus(state.output, "failed"),
          error: event.raw,
          sawTerminal: true,
          terminalTurnId: event.turnId,
          usage: state.usage,
        };
      }
      if (event.status === "interjected") {
        return {
          ...state,
          status: "interrupted",
          output: withTurnInterjectedMarker(
            markOutputStatus(state.output, "interrupted"),
          ),
          error: null,
          sawTerminal: true,
          terminalTurnId: event.turnId,
          usage: state.usage,
        };
      }
      if (event.status === "interrupted") {
        return {
          ...state,
          status: "interrupted",
          output: withTurnAbortedMarker(
            markOutputStatus(state.output, "interrupted"),
          ),
          error: null,
          sawTerminal: true,
          terminalTurnId: event.turnId,
          usage: state.usage,
        };
      }
      return {
        ...state,
        status: "completed",
        output: normalizeTerminalOutput(state.output),
        error: null,
        sawTerminal: true,
        terminalTurnId: event.turnId,
        usage: state.usage,
      };
    case "user_message":
      return state;
    case "context_compaction":
      return state;
    case "conversation_control":
      return state;
    case "history_watermark_reached":
      return state;
    case "history_refresh_required":
      return state;
    case "history_mutated":
      return state;
    case "stream_replay":
      return event.events.reduce(foldConversationStreamEvent, state);
    case "completed":
      return {
        ...state,
        status: state.status === "idle" ? "generating" : state.status,
        output:
          event.output.length > 0
            ? normalizeTerminalOutput(event.output)
            : normalizeTerminalOutput(state.output),
        error: null,
      };
    case "failed":
      return {
        ...state,
        status: "failed",
        output: markOutputStatus(
          event.output.length > 0 ? event.output : state.output,
          "failed",
        ),
        error:
          event.error === "remote_delivery_failed" && state.error
            ? state.error
            : event.error,
      };
    case "interrupted":
      return {
        ...state,
        status: "interrupted",
        output:
          event.reason === "interjected"
            ? withTurnInterjectedMarker(
                markOutputStatus(
                  event.output.length > 0 ? event.output : state.output,
                  "interrupted",
                ),
              )
            : withTurnAbortedMarker(
                markOutputStatus(
                  event.output.length > 0 ? event.output : state.output,
                  "interrupted",
                ),
              ),
        error: null,
      };
    case "parse_error":
      return {
        ...state,
        status: "failed",
        output: markOutputStatus(state.output, "failed"),
        error: event.error,
        sawTerminal: true,
      };
    case "eof_without_terminal":
      return state;
  }
}

export function finishConversationStream(
  state: ConversationStreamFold,
): ConversationStreamFold {
  return foldConversationStreamEvent(state, { kind: "eof_without_terminal" });
}

export function interruptConversationStream(
  state: ConversationStreamFold,
): ConversationStreamFold {
  return foldConversationStreamEvent(state, {
    kind: "interrupted",
    output: [],
  });
}
