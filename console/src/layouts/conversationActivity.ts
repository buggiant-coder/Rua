import { useSyncExternalStore } from "react";
import { subscribeJsonLines } from "@/api/jsonLineSubscription";
import {
  conversationPath,
  type ConversationAddress,
} from "@/pages/Chat/conversationAddress";
import { invalidateConversationNavigation } from "./conversationNavigationEvents";

export type ConversationCompletion = {
  turn_id: string;
  completed_at: string;
  token: string;
};
export type ConversationActivity = {
  address: ConversationAddress;
  running: boolean;
  completion?: ConversationCompletion;
};
type Entry = {
  running: boolean;
  completion?: ConversationCompletion;
  readToken?: string;
};
export type ConversationActivityStatus = {
  running: boolean;
  unread: boolean;
};

const STORAGE_PREFIX = "rua.conversation-activity.v1:";
const STATUSES: ConversationActivityStatus[] = [
  { running: false, unread: false },
  { running: false, unread: true },
  { running: true, unread: false },
  { running: true, unread: true },
];

function isCompletion(value: unknown): value is ConversationCompletion {
  if (!value || typeof value !== "object") return false;
  const item = value as ConversationCompletion;
  return (
    typeof item.turn_id === "string" &&
    typeof item.token === "string" &&
    item.token.length > 0 &&
    typeof item.completed_at === "string" &&
    Number.isFinite(Date.parse(item.completed_at))
  );
}

function newerCompletion(
  current: ConversationCompletion | undefined,
  next: ConversationCompletion | undefined,
): ConversationCompletion | undefined {
  if (!next || current?.token === next.token) return current;
  if (!current) return next;
  const difference =
    Date.parse(next.completed_at) - Date.parse(current.completed_at);
  if (difference !== 0) return difference > 0 ? next : current;
  // Preserve the server's sub-millisecond precision for rapid regeneration.
  const fraction = (timestamp: string) =>
    (timestamp.match(/\.(\d+)(?:Z|[+-]\d\d:\d\d)$/)?.[1] ?? "").padEnd(9, "0");
  return fraction(next.completed_at) > fraction(current.completed_at)
    ? next
    : current;
}

function parseStored(
  value: string | null,
): Pick<Entry, "completion" | "readToken"> | null {
  try {
    const entry = JSON.parse(value ?? "null");
    return entry && isCompletion(entry.completion)
      ? {
          completion: entry.completion,
          readToken:
            typeof entry.readToken === "string" ? entry.readToken : undefined,
        }
      : null;
  } catch {
    return null;
  }
}

/** Only successful completion facts create unread state; list metadata never does. */
export class ConversationActivityStore {
  private entries = new Map<string, Entry>();
  private listeners = new Set<() => void>();
  private viewed: string | null = null;
  private storage: Storage | null;
  private isForeground: () => boolean;
  private onBoundary?: (address?: ConversationAddress) => void;

  constructor(
    storage: Storage | null,
    isForeground: () => boolean,
    onBoundary?: (address?: ConversationAddress) => void,
  ) {
    this.storage = storage;
    this.isForeground = isForeground;
    this.onBoundary = onBoundary;
    try {
      if (!storage) return;
      for (let index = 0; index < storage.length; index += 1) {
        const key = storage.key(index);
        if (key?.startsWith(STORAGE_PREFIX)) {
          const entry = parseStored(storage.getItem(key));
          if (entry)
            this.entries.set(key.slice(STORAGE_PREFIX.length), {
              ...entry,
              running: false,
            });
        }
      }
    } catch {
      // Browsers may deny storage; activity still works for the current session.
    }
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  status(address: ConversationAddress): ConversationActivityStatus {
    const entry = this.entries.get(conversationPath(address));
    const unread = Boolean(
      entry?.completion && entry.completion.token !== entry.readToken,
    );
    return STATUSES[(entry?.running ? 2 : 0) + (unread ? 1 : 0)];
  }

  setViewed(address: ConversationAddress | null): void {
    this.viewed = address ? conversationPath(address) : null;
    this.markViewedRead();
  }

  markViewedRead(): void {
    if (!this.viewed || !this.isForeground()) return;
    const entry = this.entries.get(this.viewed);
    if (!entry?.completion || entry.readToken === entry.completion.token)
      return;
    this.entries.set(this.viewed, {
      ...entry,
      readToken: entry.completion.token,
    });
    this.persist(this.viewed);
    this.notify();
  }

  applyActivity(activity: ConversationActivity): void {
    if (this.apply(activity)) {
      this.notify();
      this.onBoundary?.(activity.address);
    }
  }

  applySnapshot(activities: ConversationActivity[]): void {
    const running = new Set(
      activities
        .filter((item) => item.running)
        .map((item) => conversationPath(item.address)),
    );
    let changed = false;
    for (const [key, entry] of this.entries) {
      if (entry.running && !running.has(key)) {
        this.entries.set(key, { ...entry, running: false });
        changed = true;
      }
    }
    for (const activity of activities)
      changed = this.apply(activity) || changed;
    if (changed) {
      this.notify();
      this.onBoundary?.();
    }
  }

  clear(address: ConversationAddress): void {
    const path = conversationPath(address);
    const completion = this.entries.get(path)?.completion;
    // Retain the read watermark so a reconnect cannot resurrect a deleted chat's dot.
    this.entries.set(path, {
      running: false,
      completion,
      readToken: completion?.token,
    });
    if (completion) this.persist(path);
    this.notify();
  }

  mergeStorage(key: string | null, value: string | null): void {
    if (!key?.startsWith(STORAGE_PREFIX)) return;
    const path = key.slice(STORAGE_PREFIX.length);
    if (value === null) {
      this.entries.delete(path);
      this.notify();
      return;
    }
    const incoming = parseStored(value);
    if (!incoming) return;
    const current = this.entries.get(path);
    const completion = newerCompletion(
      current?.completion,
      incoming.completion,
    );
    const readToken = [current?.readToken, incoming.readToken].find(
      (token) => token === completion?.token,
    );
    this.entries.set(path, {
      running: current?.running ?? false,
      completion,
      readToken,
    });
    this.markViewedRead();
    // Restore the merged watermark if another tab wrote an older completion.
    const merged = this.serialized(path);
    if (merged !== value) this.persist(path);
    this.notify();
  }

  private apply(activity: ConversationActivity): boolean {
    const path = conversationPath(activity.address);
    const current = this.entries.get(path);
    const completion = newerCompletion(
      current?.completion,
      activity.completion,
    );
    const readToken =
      this.viewed === path && this.isForeground()
        ? completion?.token
        : current?.readToken;
    const changed =
      current?.running !== activity.running ||
      completion !== current?.completion ||
      readToken !== current?.readToken;
    this.entries.set(path, {
      running: activity.running,
      completion,
      readToken,
    });
    if (completion !== current?.completion || readToken !== current?.readToken)
      this.persist(path);
    return changed;
  }

  private serialized(path: string): string {
    const entry = this.entries.get(path);
    return JSON.stringify({
      completion: entry?.completion,
      readToken: entry?.readToken,
    });
  }

  private persist(path: string): void {
    try {
      this.storage?.setItem(STORAGE_PREFIX + path, this.serialized(path));
    } catch {
      /* Session state remains valid. */
    }
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }
}

function decodeActivity(value: unknown): ConversationActivity | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  if (
    typeof item.running !== "boolean" ||
    !item.address ||
    typeof item.address !== "object"
  )
    return null;
  const { scope, chat_id } = item.address as {
    scope?: Record<string, unknown>;
    chat_id?: unknown;
  };
  if (typeof chat_id !== "string" || !scope) return null;
  let address: ConversationAddress;
  if (scope.kind === "agent" && typeof scope.agent_id === "string") {
    address = { scope: "agent", agentId: scope.agent_id, chatId: chat_id };
  } else if (scope.kind === "project" && typeof scope.project_id === "string") {
    address = {
      scope: "project",
      projectId: scope.project_id,
      chatId: chat_id,
    };
  } else return null;
  return {
    address,
    running: item.running,
    completion: isCompletion(item.completion) ? item.completion : undefined,
  };
}

let store: ConversationActivityStore | undefined;
function getStore(): ConversationActivityStore {
  if (!store) {
    let storage: Storage | null = null;
    try {
      storage = window.localStorage;
    } catch {
      /* Storage can be unavailable. */
    }
    store = new ConversationActivityStore(
      storage,
      () => document.visibilityState === "visible" && document.hasFocus(),
      invalidateConversationNavigation,
    );
  }
  return store;
}

export function setViewedConversation(
  address: ConversationAddress | null,
): void {
  getStore().setViewed(address);
}

export function clearConversationActivity(address: ConversationAddress): void {
  getStore().clear(address);
}

export function useConversationActivityStatus(
  address: ConversationAddress,
): ConversationActivityStatus {
  const activityStore = getStore();
  return useSyncExternalStore(
    activityStore.subscribe,
    () => activityStore.status(address),
    () => STATUSES[0],
  );
}

let consumers = 0;
let stopTransport: (() => void) | undefined;

/** One application-wide connection survives route and sidebar group changes. */
export function startConversationActivity(): () => void {
  consumers += 1;
  if (consumers === 1) stopTransport = connectActivity(getStore());
  let released = false;
  return () => {
    if (released) return;
    released = true;
    consumers -= 1;
    if (consumers === 0) {
      stopTransport?.();
      stopTransport = undefined;
    }
  };
}

function connectActivity(activityStore: ConversationActivityStore): () => void {
  const consume = (message: unknown) => {
    if (!message || typeof message !== "object") return;
    try {
      const payload = message as Record<string, unknown>;
      if (payload.type === "snapshot" && Array.isArray(payload.activities)) {
        activityStore.applySnapshot(
          payload.activities
            .map(decodeActivity)
            .filter(
              (
                item: ConversationActivity | null,
              ): item is ConversationActivity => item !== null,
            ),
        );
      } else if (payload.type === "activity") {
        const activity = decodeActivity(payload);
        if (activity) activityStore.applyActivity(activity);
      }
    } catch {
      /* Ignore malformed frames without dropping later activity. */
    }
  };
  const subscription = subscribeJsonLines({
    path: "/platform/conversation-activity",
    onMessage: consume,
  });
  const handleVisibility = () => {
    if (document.visibilityState !== "visible") return;
    activityStore.markViewedRead();
    subscription.reconnect();
  };
  const handleFocus = () => activityStore.markViewedRead();
  const handleStorage = (event: StorageEvent) =>
    activityStore.mergeStorage(event.key, event.newValue);
  document.addEventListener("visibilitychange", handleVisibility);
  window.addEventListener("focus", handleFocus);
  window.addEventListener("storage", handleStorage);
  activityStore.markViewedRead();
  return () => {
    subscription.stop();
    document.removeEventListener("visibilitychange", handleVisibility);
    window.removeEventListener("focus", handleFocus);
    window.removeEventListener("storage", handleStorage);
  };
}
