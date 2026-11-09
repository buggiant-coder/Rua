export function canonicalHistoryFixture<T>(history: T): T {
  if (!history || typeof history !== "object" || Array.isArray(history)) {
    return history;
  }
  const record = history as Record<string, unknown>;
  if (!Array.isArray(record.messages)) return history;
  return {
    ...record,
    messages: record.messages.map((message, index) =>
      canonicalHistoryMessage(message, `test-history-${index}`),
    ),
  } as T;
}

function canonicalHistoryMessage(value: unknown, fallbackId: string): unknown {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return value;
  }
  const message = { ...(value as Record<string, unknown>) };
  message.id ||= fallbackId;
  if (Array.isArray(message.assistant_response_variants)) {
    message.assistant_response_variants =
      message.assistant_response_variants.map((variant, variantIndex) => {
        if (!variant || typeof variant !== "object" || Array.isArray(variant)) {
          return variant;
        }
        const variantRecord = { ...(variant as Record<string, unknown>) };
        if (Array.isArray(variantRecord.output)) {
          variantRecord.output = variantRecord.output.map(
            (output, outputIndex) =>
              canonicalHistoryMessage(
                output,
                `${fallbackId}-variant-${variantIndex}-${outputIndex}`,
              ),
          );
        }
        return variantRecord;
      });
  }
  return message;
}

export function createCanonicalStreamFixture() {
  const toolIdsByIndex = new Map<number, string>();
  let textSequence = 0;
  let reasoningSequence = 0;
  let messageSequence = 0;

  const nextTextId = () => `msg-test-text-${textSequence}`;
  const nextReasoningId = () => `msg-test-reasoning-${reasoningSequence}`;
  const advanceTurn = () => {
    textSequence += 1;
    reasoningSequence += 1;
    toolIdsByIndex.clear();
  };

  const normalize = (value: unknown): unknown => {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      return value;
    }
    const payload = { ...(value as Record<string, unknown>) };
    if (payload.type === "stream_replay" && Array.isArray(payload.events)) {
      payload.events = payload.events.map((event) =>
        normalize(typeof event === "string" ? JSON.parse(event) : event),
      );
      return payload;
    }
    if (payload.type === "user_message") {
      if (
        payload.message &&
        typeof payload.message === "object" &&
        !Array.isArray(payload.message)
      ) {
        payload.message = canonicalHistoryMessage(
          payload.message,
          `msg-test-user-${messageSequence++}`,
        );
      }
      advanceTurn();
      return payload;
    }
    if (payload.type === "turn_terminal") {
      advanceTurn();
      return payload;
    }
    if (payload.type === "text_delta") {
      payload.message_id ||= nextTextId();
    } else if (payload.type === "thinking_delta") {
      payload.message_id ||= nextReasoningId();
    } else if (payload.type === "tool_call_delta") {
      const index =
        typeof payload.index === "number" && Number.isFinite(payload.index)
          ? payload.index
          : 0;
      const id =
        (typeof payload.id === "string" && payload.id) ||
        toolIdsByIndex.get(index);
      if (id) {
        toolIdsByIndex.set(index, id);
        payload.id = id;
        payload.message_id ||= `msg-tc-${id}`;
      }
    } else if (payload.type === "tool_call") {
      const id = typeof payload.id === "string" ? payload.id : "";
      if (id) payload.message_id ||= `msg-tc-${id}`;
    } else if (
      payload.type === "tool_result" ||
      payload.type === "tool_result_delta"
    ) {
      const id = typeof payload.id === "string" ? payload.id : "";
      if (id) payload.message_id ||= `msg-tr-${id}`;
    } else if (
      payload.type === "runtime_note" &&
      typeof payload.runtime_note === "object"
    ) {
      payload.id ||= `msg-test-runtime-note-${messageSequence++}`;
    }

    if (payload.object === "message") {
      payload.id ||= `msg-test-output-${messageSequence++}`;
    }
    if (payload.object === "response" && Array.isArray(payload.output)) {
      payload.output = payload.output.map((message) =>
        canonicalHistoryMessage(
          message,
          `msg-test-output-${messageSequence++}`,
        ),
      );
    }
    return payload;
  };

  return { normalize };
}
