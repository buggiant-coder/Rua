import { useCallback, useEffect, useRef, useState } from "react";

import { chatApi } from "@/api/modules/chat";
import { projectChatsApi } from "@/api/modules/projectChats";
import type { ChatSpec } from "@/api/types";
import { DEFAULT_CHAT_LIST_SORT } from "@/api/types/chatListSort";
import type { ConversationAddress } from "@/pages/Chat/conversationAddress";
import {
  subscribeConversationNavigation,
  subscribeConversationMetadata,
} from "../conversationNavigationEvents";

export type NavigationScope =
  | { kind: "agent"; id: string }
  | { kind: "project"; id: string };

export const PAGE_SIZE = 20;
const INITIAL_PAGE_SIZE = 5;

export function scopeKey(scope: NavigationScope): string {
  return `${scope.kind}:${scope.id}`;
}

export function scopeAddress(
  scope: NavigationScope,
  chatId: string,
): ConversationAddress {
  return scope.kind === "agent"
    ? { scope: "agent", agentId: scope.id, chatId }
    : { scope: "project", projectId: scope.id, chatId };
}

const metadataFields = [
  "name",
  "tag",
  "pinned",
  "pin_order",
  "archived",
  "updated_at",
] as const;
type MetadataField = (typeof metadataFields)[number];
type FieldUpdate = {
  token: number;
  pending: boolean;
  value: unknown;
  previous?: FieldUpdate;
};
type MetadataOverlay = {
  chat: ChatSpec;
  fields: Map<MetadataField, FieldUpdate>;
};

function activity(chat: ChatSpec) {
  return Date.parse(chat.updated_at || chat.created_at || "") || 0;
}
function sortChats(chats: ChatSpec[]) {
  return chats.sort(
    (a, b) =>
      Number(Boolean(b.pinned)) - Number(Boolean(a.pinned)) ||
      (a.pinned && b.pinned ? (a.pin_order ?? 0) - (b.pin_order ?? 0) : 0) ||
      activity(b) - activity(a) ||
      a.id.localeCompare(b.id),
  );
}
function applyOverlay(chat: ChatSpec, overlay?: MetadataOverlay): ChatSpec {
  if (!overlay) return chat;
  return {
    ...chat,
    ...Object.fromEntries(
      [...overlay.fields].map(([key, field]) => [key, field.value]),
    ),
  };
}

export function useConversationGroup(
  scope: NavigationScope,
  expanded: boolean,
) {
  const [chats, setChats] = useState<ChatSpec[]>([]);
  const [hasMore, setHasMore] = useState(false);
  // Pending/acknowledged fields belong to this list projection until a list read catches up.
  const overlays = useRef(new Map<string, MetadataOverlay>());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const removeChat = useCallback((chatId: string) => {
    overlays.current.delete(chatId);
    setChats((current) => current.filter((chat) => chat.id !== chatId));
  }, []);
  const requestRef = useRef(0);
  const loadedRef = useRef(false);
  const staleRef = useRef(true);
  const loadingRef = useRef(false);
  const offsetRef = useRef(0);
  const { kind, id } = scope;

  const load = useCallback(
    async (append = false) => {
      if (append && loadingRef.current) return;
      const request = ++requestRef.current;
      loadingRef.current = true;
      setLoading(true);
      setError(null);
      const membershipChanged = [...overlays.current.values()].some(
        (overlay) =>
          overlay.fields.has("pinned") || overlay.fields.has("archived"),
      );
      // Pin/unpin changes the ordinary-chat offset. Rebuild its prefix until acknowledged.
      const appendPage = append && !membershipChanged;
      const offset = appendPage ? offsetRef.current : 0;
      const params = {
        offset,
        limit: appendPage
          ? PAGE_SIZE
          : Math.max(
              INITIAL_PAGE_SIZE,
              offsetRef.current + (append ? PAGE_SIZE : 0),
            ),
        view: "user" as const,
        archived: false,
        ...DEFAULT_CHAT_LIST_SORT,
      };
      try {
        const incomingPages = new Map<string, ChatSpec>();
        let next = offset;
        let total = 0;
        let pageHasMore = false;
        do {
          const pageParams = {
            ...params,
            offset: next,
            limit: Math.min(200, offset + params.limit - next),
          };
          const page =
            kind === "project"
              ? await projectChatsApi.listChats(id, pageParams)
              : await chatApi.listChats(pageParams, id);
          if (request !== requestRef.current) return;
          for (const chat of page.data) incomingPages.set(chat.id, chat);
          const ordinaryCount = page.data.filter((chat) => !chat.pinned).length;
          next += ordinaryCount;
          total = page.total;
          pageHasMore =
            next + page.data.filter((chat) => chat.pinned).length < total;
          if (ordinaryCount < pageParams.limit) break;
        } while (!appendPage && next < offset + params.limit && pageHasMore);
        const result = { data: [...incomingPages.values()], total };
        const incoming = new Map(result.data.map((chat) => [chat.id, chat]));
        for (const [chatId, overlay] of overlays.current) {
          const fetched = incoming.get(chatId);
          if (!fetched) {
            if (
              !appendPage &&
              ![...overlay.fields.values()].some((field) => field.pending)
            )
              overlays.current.delete(chatId);
            continue;
          }
          for (const [field, update] of overlay.fields) {
            if (
              !update.pending &&
              (Object.is(fetched[field], update.value) ||
                activity(fetched) > activity(overlay.chat))
            )
              overlay.fields.delete(field);
          }
          if (overlay.fields.size === 0) overlays.current.delete(chatId);
        }
        setChats((current) => {
          const merged = new Map(
            (appendPage ? current : []).map((chat) => [chat.id, chat]),
          );
          for (const chat of result.data) merged.set(chat.id, chat);
          for (const [chatId, overlay] of overlays.current) {
            merged.set(
              chatId,
              applyOverlay(
                merged.get(chatId) ??
                  current.find((chat) => chat.id === chatId) ??
                  overlay.chat,
                overlay,
              ),
            );
          }
          return sortChats(
            [...merged.values()].filter((chat) => !chat.archived),
          );
        });
        offsetRef.current = next;
        // Count the server page, not optimistic pin membership, to decide whether more exists.
        setHasMore(pageHasMore);
        loadedRef.current = true;
        staleRef.current = false;
      } catch (cause) {
        if (request !== requestRef.current) return;
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        if (request === requestRef.current) {
          loadingRef.current = false;
          setLoading(false);
        }
      }
    },
    [id, kind],
  );

  useEffect(
    () =>
      subscribeConversationMetadata((address, chat, change) => {
        if (
          address.scope !== kind ||
          (address.scope === "agent" ? address.agentId : address.projectId) !==
            id
        )
          return;
        const overlay = overlays.current.get(chat.id) ?? {
          chat,
          fields: new Map<MetadataField, FieldUpdate>(),
        };
        const fields = change
          ? metadataFields.filter((field) => change.fields.includes(field))
          : metadataFields;
        const accepted: MetadataField[] = [];
        for (const field of fields) {
          const previous = overlay.fields.get(field);
          if (
            change &&
            change.phase !== "optimistic" &&
            previous?.token !== change.token
          )
            continue;
          accepted.push(field);
          if (change?.phase === "rollback") {
            if (previous?.previous)
              overlay.fields.set(field, previous.previous);
            else overlay.fields.delete(field);
          } else {
            overlay.fields.set(field, {
              token: change?.token ?? 0,
              pending: change?.phase === "optimistic",
              value: chat[field],
              previous: change?.phase === "optimistic" ? previous : undefined,
            });
          }
        }
        if (
          change?.phase === "commit" &&
          accepted.length > 0 &&
          activity(chat) >= activity(overlay.chat)
        ) {
          overlay.fields.set("updated_at", {
            token: change.token,
            pending: false,
            value: chat.updated_at,
          });
          accepted.push("updated_at");
        }
        const patch = Object.fromEntries(
          accepted.map((field) => [field, chat[field]]),
        );
        overlay.chat = { ...overlay.chat, ...patch };
        if (overlay.fields.size > 0) overlays.current.set(chat.id, overlay);
        else overlays.current.delete(chat.id);
        setChats((current) => {
          const existing = current.find((item) => item.id === chat.id);
          if (!existing && change?.phase !== "rollback") return current;
          const updated = applyOverlay(
            { ...(existing ?? chat), ...patch },
            overlay,
          );
          const remaining = current.filter((item) => item.id !== chat.id);
          return sortChats(
            updated.archived ? remaining : [...remaining, updated],
          );
        });
      }),
    [id, kind],
  );

  useEffect(() => {
    return subscribeConversationNavigation((address, background) => {
      if (
        address &&
        !(
          address.scope === kind &&
          (address.scope === "agent" ? address.agentId : address.projectId) ===
            id
        )
      )
        return;
      if (background && loadingRef.current) return;
      staleRef.current = true;
      requestRef.current += 1;
      loadingRef.current = false;
      setRevision((value) => value + 1);
    });
  }, [id, kind]);

  useEffect(() => {
    if (expanded && staleRef.current) void load();
    else if (!expanded) setLoading(false);
    return () => {
      requestRef.current += 1;
      if (loadingRef.current) staleRef.current = true;
      loadingRef.current = false;
    };
  }, [expanded, load, revision]);

  return {
    chats,
    removeChat,
    loaded: loadedRef.current,
    loading,
    error,
    hasMore,
    reload: () => void load(),
    loadMore: () => void load(true),
  };
}
