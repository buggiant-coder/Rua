export type ChatBlockCollapsePreferences = {
  aggregateExpanded: boolean;
  thinkingExpanded: boolean;
  toolExpanded: boolean;
};

export type ChatBlockPreferenceKey = keyof ChatBlockCollapsePreferences;

const STORAGE_KEY = "rua:chat-block-collapse-preferences";

export const DEFAULT_CHAT_BLOCK_PREFERENCES: ChatBlockCollapsePreferences = {
  aggregateExpanded: false,
  thinkingExpanded: false,
  toolExpanded: false,
};

function normalizePreferences(value: unknown): ChatBlockCollapsePreferences {
  const record =
    value && typeof value === "object"
      ? (value as Partial<Record<ChatBlockPreferenceKey, unknown>>)
      : {};
  return {
    aggregateExpanded:
      typeof record.aggregateExpanded === "boolean"
        ? record.aggregateExpanded
        : DEFAULT_CHAT_BLOCK_PREFERENCES.aggregateExpanded,
    thinkingExpanded:
      typeof record.thinkingExpanded === "boolean"
        ? record.thinkingExpanded
        : DEFAULT_CHAT_BLOCK_PREFERENCES.thinkingExpanded,
    toolExpanded:
      typeof record.toolExpanded === "boolean"
        ? record.toolExpanded
        : DEFAULT_CHAT_BLOCK_PREFERENCES.toolExpanded,
  };
}

export function readChatBlockPreferences(): ChatBlockCollapsePreferences {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_CHAT_BLOCK_PREFERENCES;
    return normalizePreferences(JSON.parse(raw));
  } catch {
    return DEFAULT_CHAT_BLOCK_PREFERENCES;
  }
}

export function writeChatBlockPreferences(
  preferences: ChatBlockCollapsePreferences,
) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
  } catch {}
}
