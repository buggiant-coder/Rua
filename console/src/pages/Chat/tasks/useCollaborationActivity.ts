import { useCallback, useEffect, useMemo, useState } from "react";
import { chatApi } from "@/api/modules/chat";
import type { ConversationApiTarget } from "@/api/modules/conversation";
import type {
  ChatSpec,
  ConversationBackgroundWorkView,
} from "@/api/types/chat";
import type { TaskChatAddress } from "@/api/types/task";

export interface CollaborationActivity {
  key: string;
  address: TaskChatAddress;
  chat?: ChatSpec;
  status: "running" | "idle";
  pending: boolean;
}

export function collaborationAddressKey(address: TaskChatAddress): string {
  return address.chat_id;
}

async function listSourceCollaborations(
  _target: ConversationApiTarget,
  chatId: string,
) {
  const chats: ChatSpec[] = [];
  let offset = 0;
  while (true) {
    const page = await chatApi.listChats({
      view: "collaboration",
      source_chat_id: chatId,
      offset,
      limit: 200,
      sort_by: "updated_at",
      sort_order: "desc",
    });
    chats.push(...page.data);
    offset += page.data.length;
    if (page.data.length === 0 || offset >= page.total) break;
  }
  return chats
    .filter((chat) => chat.collaboration?.source.chat_id === chatId)
    .map((chat) => ({ chat, address: { chat_id: chat.id } }));
}

export function useCollaborationActivity({
  open,
  target,
  chatId,
  backgroundWork,
}: {
  open: boolean;
  target: ConversationApiTarget;
  chatId?: string;
  backgroundWork?: ConversationBackgroundWorkView | null;
}) {
  const scopeKey = JSON.stringify([target, chatId]);
  const [snapshot, setSnapshot] = useState<{
    key: string;
    rows: Awaited<ReturnType<typeof listSourceCollaborations>>;
  }>({ key: "", rows: [] });
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const reload = useCallback(() => setRefresh((value) => value + 1), []);
  const backgroundKey = JSON.stringify(backgroundWork);

  useEffect(() => {
    let stopped = false;
    if (!open || !chatId) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    const load = async () => {
      if (inFlight || document.visibilityState === "hidden") return;
      inFlight = true;
      setLoading(true);
      try {
        const rows = await listSourceCollaborations(target, chatId);
        if (stopped) return;
        setSnapshot({ key: scopeKey, rows });
        setError(null);
      } catch (cause) {
        if (!stopped)
          setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        inFlight = false;
        if (!stopped) {
          setLoading(false);
          timer = setTimeout(() => void load(), 5000);
        }
      }
    };
    const onVisibility = () => {
      clearTimeout(timer);
      if (document.visibilityState !== "hidden") void load();
    };
    setError(null);
    void load();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [open, target, chatId, scopeKey, refresh, backgroundKey]);

  const collaborations = useMemo(() => {
    const rows = new Map<string, CollaborationActivity>();
    for (const { chat, address } of snapshot.key === scopeKey
      ? snapshot.rows
      : []) {
      const key = collaborationAddressKey(address);
      rows.set(key, {
        key,
        chat,
        address,
        status: chat.status && chat.status !== "idle" ? "running" : "idle",
        pending: Boolean(chat.status && chat.status !== "idle"),
      });
    }
    for (const item of backgroundWork?.items ?? []) {
      if (item.kind !== "collaboration" || item.status !== "running") continue;
      const key = collaborationAddressKey(item.chat);
      rows.set(key, {
        ...rows.get(key),
        key,
        address: item.chat,
        status: "running",
        pending: true,
      });
    }
    return [...rows.values()].sort(
      (a, b) =>
        (b.chat?.updated_at ?? "").localeCompare(a.chat?.updated_at ?? "") ||
        a.key.localeCompare(b.key),
    );
  }, [snapshot, scopeKey, backgroundWork]);

  return {
    collaborations,
    loading,
    error: error ?? backgroundWork?.error ?? null,
    reload,
  };
}
