import type { ChatSpec, ChatUpdateRequest } from "@/api/types";
import type { ConversationAddress } from "@/pages/Chat/conversationAddress";
import { conversationPath } from "@/pages/Chat/conversationAddress";

const newConversationActions = new Map<string, () => void>();

/** The page owns creation, including inheritance and transition cleanup. */
export function registerNewConversationAction(
  address: ConversationAddress,
  action: () => void,
): () => void {
  const key = conversationPath(address);
  newConversationActions.set(key, action);
  return () => {
    if (newConversationActions.get(key) === action)
      newConversationActions.delete(key);
  };
}

export function requestNewConversation(address: ConversationAddress): boolean {
  const action = newConversationActions.get(conversationPath(address));
  if (!action) return false;
  action();
  return true;
}

type Listener = (address?: ConversationAddress, background?: boolean) => void;
const listeners = new Set<Listener>();

/** Refresh list projections after a Chat or Project mutation, never per token. */
export function invalidateConversationNavigation(
  address?: ConversationAddress,
  background = false,
) {
  for (const listener of listeners) listener(address, background);
}

export function subscribeConversationNavigation(
  listener: Listener,
): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export type ConversationMetadataChange = {
  phase: "optimistic" | "commit" | "rollback";
  token: number;
  fields: (keyof ChatSpec)[];
};
type MetadataListener = (
  address: ConversationAddress,
  chat: ChatSpec,
  change?: ConversationMetadataChange,
) => void;
let metadataToken = 0;
const metadataListeners = new Set<MetadataListener>();

/** Keep list projections and the current header in sync with metadata edits. */
export function publishConversationMetadata(
  address: ConversationAddress,
  chat: ChatSpec,
) {
  for (const listener of metadataListeners) listener(address, chat);
  invalidateConversationNavigation(address);
}

/** Apply pending fields immediately; only their matching write may confirm or undo them. */
export function beginConversationMetadataUpdate(
  address: ConversationAddress,
  chat: ChatSpec,
  updates: ChatUpdateRequest,
) {
  const token = ++metadataToken;
  const fields = Object.keys(updates) as (keyof ChatSpec)[];
  const emit = (next: ChatSpec, phase: ConversationMetadataChange["phase"]) => {
    for (const listener of metadataListeners)
      listener(address, next, { phase, token, fields });
  };
  emit({ ...chat, ...updates }, "optimistic");
  return {
    commit(updated: ChatSpec) {
      emit(updated, "commit");
      invalidateConversationNavigation(address);
    },
    rollback() {
      emit(chat, "rollback");
    },
  };
}

export function subscribeConversationMetadata(
  listener: MetadataListener,
): () => void {
  metadataListeners.add(listener);
  return () => {
    metadataListeners.delete(listener);
  };
}

// The current page retains ownership of its scoped fallback and draft inheritance.
type DeletedListener = (
  address: ConversationAddress,
  remaining: ChatSpec[],
) => boolean;
const deletedListeners = new Set<DeletedListener>();

export function subscribeConversationDeleted(
  listener: DeletedListener,
): () => void {
  deletedListeners.add(listener);
  return () => {
    deletedListeners.delete(listener);
  };
}

export function publishConversationDeleted(
  address: ConversationAddress,
  remaining: ChatSpec[],
): boolean {
  for (const listener of deletedListeners) {
    if (listener(address, remaining)) return true;
  }
  return false;
}
