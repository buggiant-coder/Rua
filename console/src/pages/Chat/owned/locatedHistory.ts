import type { ChatHistory } from "@/api/types";

export function joinHistoryWindow(
  current: ChatHistory,
  additional: ChatHistory,
): ChatHistory {
  const positions = new Map(
    current.messages.map((message, index) => [message.id, index]),
  );
  const additionalFirst =
    (additional.oldest_turn_ordinal ?? 0) <
    (current.oldest_turn_ordinal ?? Infinity);
  const messages: ChatHistory["messages"] = [];
  let pending: ChatHistory["messages"] = [];
  let cursor = 0;
  let overlap = false;
  for (const message of additional.messages) {
    const position = positions.get(message.id);
    if (position === undefined) {
      pending.push(message);
    } else if (position >= cursor) {
      overlap = true;
      messages.push(
        ...current.messages.slice(cursor, position),
        ...pending,
        current.messages[position],
      );
      pending = [];
      cursor = position + 1;
    }
  }
  if (!overlap) {
    messages.push(
      ...(additionalFirst ? additional.messages : current.messages),
      ...(additionalFirst ? current.messages : additional.messages),
    );
  } else messages.push(...current.messages.slice(cursor), ...pending);
  return {
    ...current,
    messages,
    oldest_turn_ordinal: additionalFirst
      ? additional.oldest_turn_ordinal
      : current.oldest_turn_ordinal,
    newest_turn_ordinal: Math.max(
      additional.newest_turn_ordinal ?? 0,
      current.newest_turn_ordinal ?? 0,
    ),
    has_more_before: additionalFirst
      ? additional.has_more_before
      : current.has_more_before,
  };
}

export function historyWindowGap(
  current: ChatHistory | null,
  located: ChatHistory | null,
) {
  if (!current || !located) return null;
  const [earlier, later] =
    (current.oldest_turn_ordinal ?? 0) < (located.oldest_turn_ordinal ?? 0)
      ? [current, located]
      : [located, current];
  if (
    later.oldest_turn_ordinal == null ||
    earlier.newest_turn_ordinal == null ||
    earlier.newest_turn_ordinal + 1 >= later.oldest_turn_ordinal
  )
    return null;
  return {
    beforeMessageId:
      typeof later.messages[0]?.id === "string" ? later.messages[0].id : "",
    count: later.oldest_turn_ordinal - earlier.newest_turn_ordinal - 1,
    beforeTurnOrdinal: later.oldest_turn_ordinal,
  };
}

export function preserveLoadedHistory(
  current: ChatHistory | null,
  incoming: ChatHistory,
): ChatHistory {
  if (
    !current ||
    !incoming.has_more_before ||
    (current.oldest_turn_ordinal ?? Infinity) >=
      (incoming.oldest_turn_ordinal ?? 0) ||
    (current.newest_turn_ordinal ?? Infinity) >
      (incoming.newest_turn_ordinal ?? 0)
  )
    return incoming;
  const first = incoming.messages[0];
  if (!first) return incoming;
  const index = current.messages.findIndex(
    (message) => message.id === first.id,
  );
  if (index <= 0) return incoming;
  return {
    ...incoming,
    messages: [...current.messages.slice(0, index), ...incoming.messages],
    oldest_turn_ordinal: current.oldest_turn_ordinal,
    has_more_before: current.has_more_before,
  };
}
