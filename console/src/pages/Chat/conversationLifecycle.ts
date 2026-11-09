import type { ModelSlot } from "@/api/types";

const LAST_CHAT_PREFIX = "rua_last_chat_id_";
const RECENT_CHAT_PREFIX = "rua_recent_chat_ids_";
const LAST_CONVERSATION_MODEL_KEY = "rua_last_conversation_model";

export function rememberConversationModel(model: ModelSlot): void {
  if (!model.provider_id || !model.model) return;
  try {
    localStorage.setItem(LAST_CONVERSATION_MODEL_KEY, JSON.stringify(model));
  } catch {}
}

const DRAFT_CHAT_PREFIX = "rua_draft_chat_id_";
export type DraftAcpSelection = {
  runner: string;
  cwd?: string;
  resume_session_id?: string;
};
const draftAcpKey = (scope: string, id: string) =>
  `rua_draft_chat_acp_${scope}:${id}`;

export function loadDraftConversationAcp(
  scope: string,
  id: string,
): DraftAcpSelection | null {
  try {
    const value = JSON.parse(
      sessionStorage.getItem(draftAcpKey(scope, id)) || "null",
    );
    return value && typeof value.runner === "string" && value.runner.trim()
      ? value
      : null;
  } catch {
    return null;
  }
}

export function saveDraftConversationAcp(
  scope: string,
  id: string,
  selection: DraftAcpSelection | null,
): void {
  try {
    if (selection)
      sessionStorage.setItem(draftAcpKey(scope, id), JSON.stringify(selection));
    else sessionStorage.removeItem(draftAcpKey(scope, id));
  } catch {
    /* same lifetime as existing draft choices */
  }
}
const DRAFT_CHAT_MODEL_PREFIX = "rua_draft_chat_model_";
const DRAFT_CHAT_DEVICE_PREFIX = "rua_draft_chat_device_";
const DRAFT_CHAT_CREATION_PREFIX = "rua_draft_chat_creation_";

export function draftCreationStarted(scope: string, id: string): boolean {
  try {
    return (
      sessionStorage.getItem(`${DRAFT_CHAT_CREATION_PREFIX}${scope}:${id}`) ===
      "true"
    );
  } catch {
    return false;
  }
}

export function markDraftCreationStarted(
  scope: string,
  id: string,
  started: boolean,
): void {
  try {
    const key = `${DRAFT_CHAT_CREATION_PREFIX}${scope}:${id}`;
    if (started) sessionStorage.setItem(key, "true");
    else sessionStorage.removeItem(key);
  } catch {
    /* storage unavailable */
  }
}

export function loadDraftConversationDevice(
  scope: string,
  id: string,
): string | null {
  try {
    return sessionStorage.getItem(`${DRAFT_CHAT_DEVICE_PREFIX}${scope}:${id}`);
  } catch {
    return null;
  }
}

export function saveDraftConversationDevice(
  scope: string,
  id: string,
  device: string | null,
): void {
  const key = `${DRAFT_CHAT_DEVICE_PREFIX}${scope}:${id}`;
  try {
    if (device) sessionStorage.setItem(key, device);
    else sessionStorage.removeItem(key);
  } catch {
    /* storage unavailable */
  }
}
const RECENT_CHAT_LIMIT = 20;

export type ConversationScope = string;

export function lastConversationKey(scope: ConversationScope): string {
  return `${LAST_CHAT_PREFIX}${scope}`;
}

export function recentConversationsKey(scope: ConversationScope): string {
  return `${RECENT_CHAT_PREFIX}${scope}`;
}

export function draftConversationKey(scope: ConversationScope): string {
  return `${DRAFT_CHAT_PREFIX}${scope}`;
}

export function draftConversationModelKey(
  scope: ConversationScope,
  conversationId: string,
): string {
  return `${DRAFT_CHAT_MODEL_PREFIX}${scope}:${conversationId}`;
}

export function loadLastConversationId(
  scope: ConversationScope,
): string | null {
  if (!scope) return null;
  try {
    return localStorage.getItem(lastConversationKey(scope))?.trim() || null;
  } catch {
    return null;
  }
}

export function loadRecentConversationIds(scope: ConversationScope): string[] {
  if (!scope) return [];
  try {
    const parsed = JSON.parse(
      localStorage.getItem(recentConversationsKey(scope)) || "[]",
    );
    return Array.isArray(parsed)
      ? parsed.filter((value): value is string => typeof value === "string")
      : [];
  } catch {
    return [];
  }
}

export function rememberConversation(
  scope: ConversationScope,
  conversationId: string,
): void {
  if (!scope || !conversationId) return;
  try {
    localStorage.setItem(lastConversationKey(scope), conversationId);
    const recent = loadRecentConversationIds(scope).filter(
      (id) => id !== conversationId,
    );
    localStorage.setItem(
      recentConversationsKey(scope),
      JSON.stringify([conversationId, ...recent].slice(0, RECENT_CHAT_LIMIT)),
    );
  } catch {}
}

export function forgetConversation(
  scope: ConversationScope,
  conversationId: string,
): void {
  if (!scope || !conversationId) return;
  try {
    if (localStorage.getItem(lastConversationKey(scope)) === conversationId) {
      localStorage.removeItem(lastConversationKey(scope));
    }
    const recent = loadRecentConversationIds(scope).filter(
      (id) => id !== conversationId,
    );
    localStorage.setItem(recentConversationsKey(scope), JSON.stringify(recent));
  } catch {}
}

export function loadDraftConversationId(
  scope: ConversationScope,
): string | null {
  if (!scope) return null;
  try {
    return localStorage.getItem(draftConversationKey(scope))?.trim() || null;
  } catch {
    return null;
  }
}

export function isDraftConversation(
  scope: ConversationScope,
  conversationId: string,
): boolean {
  return loadDraftConversationId(scope) === conversationId;
}

export function saveDraftConversationId(
  scope: ConversationScope,
  conversationId: string,
): void {
  if (!scope || !conversationId) return;
  try {
    localStorage.setItem(draftConversationKey(scope), conversationId);
  } catch {}
}

export function clearDraftConversation(
  scope: ConversationScope,
  conversationId?: string,
): void {
  if (!scope) return;
  const previous = loadDraftConversationId(scope);
  if (previous && (!conversationId || previous === conversationId)) {
    saveDraftConversationDevice(scope, previous, null);
    markDraftCreationStarted(scope, previous, false);
  }
  let clearedId = conversationId;
  try {
    const current = localStorage.getItem(draftConversationKey(scope));
    if (conversationId && current !== conversationId) return;
    clearedId = conversationId || current || undefined;
    localStorage.removeItem(draftConversationKey(scope));
  } catch {}
  if (clearedId) clearDraftConversationModel(scope, clearedId);
  if (clearedId) saveDraftConversationAcp(scope, clearedId, null);
}

export function clearConversationScope(scope: ConversationScope): void {
  if (!scope) return;
  const draftId = loadDraftConversationId(scope);
  try {
    localStorage.removeItem(lastConversationKey(scope));
    localStorage.removeItem(recentConversationsKey(scope));
  } catch {}
  clearDraftConversation(scope, draftId || undefined);
}

export function loadDraftConversationModel(
  scope: ConversationScope,
  conversationId: string,
): ModelSlot | null {
  if (!scope || !conversationId) return null;
  try {
    const raw = sessionStorage.getItem(
      draftConversationModelKey(scope, conversationId),
    );
    if (!raw) return null;
    const model = JSON.parse(raw) as Partial<ModelSlot>;
    return typeof model.provider_id === "string" &&
      typeof model.model === "string"
      ? (model as ModelSlot)
      : null;
  } catch {
    return null;
  }
}

export function saveDraftConversationModel(
  scope: ConversationScope,
  conversationId: string,
  model: ModelSlot,
): void {
  if (!scope || !conversationId) return;
  try {
    sessionStorage.setItem(
      draftConversationModelKey(scope, conversationId),
      JSON.stringify(model),
    );
  } catch {}
}

export function clearDraftConversationModel(
  scope: ConversationScope,
  conversationId: string,
): void {
  if (!scope || !conversationId) return;
  try {
    sessionStorage.removeItem(draftConversationModelKey(scope, conversationId));
  } catch {}
}

export function resolveOrCreateDraftConversationId(
  scope: ConversationScope,
): string {
  const existing = loadDraftConversationId(scope);
  const id = existing || crypto.randomUUID();
  if (!existing) saveDraftConversationId(scope, id);
  if (!loadDraftConversationModel(scope, id)) {
    try {
      const model = JSON.parse(
        localStorage.getItem(LAST_CONVERSATION_MODEL_KEY) || "null",
      );
      if (
        typeof model?.provider_id === "string" &&
        model.provider_id &&
        typeof model?.model === "string" &&
        model.model
      ) {
        saveDraftConversationModel(scope, id, model);
      }
    } catch {}
  }
  return id;
}

export function resolveConversationEntry(scope: ConversationScope): string {
  return (
    loadLastConversationId(scope) ||
    loadRecentConversationIds(scope)[0] ||
    resolveOrCreateDraftConversationId(scope)
  );
}

export function materializeConversation(
  scope: ConversationScope,
  conversationId: string,
): void {
  clearDraftConversation(scope, conversationId);
  rememberConversation(scope, conversationId);
}
