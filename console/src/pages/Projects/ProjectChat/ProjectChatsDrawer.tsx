import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { projectChatsApi } from "@/api/modules/projectChats";
import type { ChatSpec, ChatTag, ConversationListView } from "@/api/types";
import {
  DEFAULT_CHAT_LIST_SORT,
  type ChatListSort,
} from "@/api/types/chatListSort";
import ChatHistoryDrawer from "@/pages/Chat/ChatHistoryDrawer";
import { nextChatTag } from "@/pages/Chat/chatTag";
import ChatSearchResults from "@/pages/Chat/ChatSearchResults";
import { Modal, TextInput, toast } from "@/ui";

const PAGE_SIZE = 20;

type ProjectChatsDrawerProps = {
  inline?: boolean;
  projectId: string;
  currentChatId: string;
  currentChatRunning: boolean;
  readOnly?: boolean;
  onClose: () => void;
  onSelectChat: (chat: ChatSpec) => void;
  onChatUpdated: (chat: ChatSpec) => void;
  onChatDeleted: (chat: ChatSpec, remainingKnownChats: ChatSpec[]) => void;
};

export default function ProjectChatsDrawer({
  inline = false,
  projectId,
  currentChatId,
  currentChatRunning,
  readOnly = false,
  onClose,
  onSelectChat,
  onChatUpdated,
  onChatDeleted,
}: ProjectChatsDrawerProps) {
  const { t } = useTranslation();
  const [searchInput, setSearchInput] = useState("");
  const [archived, setArchived] = useState(false);
  const [clearing, setClearing] = useState(false);
  const clearingRef = useRef(false);
  const [conversationView, setConversationView] =
    useState<ConversationListView>("user");
  const [collaborationFilters, setCollaborationFilters] = useState({
    sourceChatId: "",
    executor: "",
    status: "",
  });
  const [sort, setSort] = useState<ChatListSort>(DEFAULT_CHAT_LIST_SORT);
  const [tagFilter, setTagFilter] = useState<ChatTag | null>(null);
  const [chats, setChats] = useState<ChatSpec[]>([]);
  const [total, setTotal] = useState(0);
  const [nextOffset, setNextOffset] = useState(0);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingChatIds, setPendingChatIds] = useState<Set<string>>(
    () => new Set(),
  );
  const requestVersionRef = useRef(0);
  const knownChatsRef = useRef(new Map<string, ChatSpec>());

  const loadChats = useCallback(
    async (
      offset: number,
      limit: number,
      append: boolean,
      quiet = false,
    ): Promise<boolean> => {
      const requestVersion = ++requestVersionRef.current;
      if (append) {
        setLoadingMore(true);
      } else if (!quiet) {
        setLoading(true);
      }
      setError(null);

      try {
        const result = await projectChatsApi.listChats(projectId, {
          offset,
          limit,
          archived,
          view: archived ? "all" : conversationView,
          ...(!archived &&
          conversationView === "collaboration" &&
          collaborationFilters.sourceChatId.trim()
            ? { source_chat_id: collaborationFilters.sourceChatId.trim() }
            : {}),
          ...(!archived &&
          conversationView === "collaboration" &&
          collaborationFilters.executor.trim()
            ? { executor: collaborationFilters.executor.trim() }
            : {}),
          ...(!archived &&
          conversationView === "collaboration" &&
          collaborationFilters.status
            ? { status: collaborationFilters.status }
            : {}),
          ...((archived || conversationView === "collaboration") &&
          searchInput.trim()
            ? { search: searchInput.trim() }
            : {}),
          ...sort,
          ...(!archived && conversationView === "user" && tagFilter
            ? { tag: tagFilter }
            : {}),
        });
        if (requestVersion !== requestVersionRef.current) return false;

        for (const chat of result.data) {
          knownChatsRef.current.set(chat.id, chat);
        }
        setChats((current) =>
          append ? appendUniqueChats(current, result.data) : result.data,
        );
        setTotal(result.total);
        setNextOffset(
          offset + result.data.filter((chat) => !chat.pinned).length,
        );
        return true;
      } catch (loadError) {
        if (requestVersion !== requestVersionRef.current) return false;
        setError(errorMessage(loadError, t("projects.chat.loadFailed")));
        return false;
      } finally {
        if (requestVersion === requestVersionRef.current) {
          setLoading(false);
          setLoadingMore(false);
        }
      }
    },
    [
      archived,
      collaborationFilters,
      conversationView,
      projectId,
      searchInput,
      sort,
      t,
      tagFilter,
    ],
  );

  useEffect(() => {
    setChats([]);
    setTotal(0);
    setNextOffset(0);
    void loadChats(0, PAGE_SIZE, false);
    return () => {
      requestVersionRef.current += 1;
    };
  }, [loadChats]);

  const searchChats = useCallback(
    (query: string) => projectChatsApi.searchChats(projectId, query),
    [projectId],
  );

  const setChatPending = useCallback((chatId: string, pending: boolean) => {
    setPendingChatIds((current) => {
      const next = new Set(current);
      if (pending) next.add(chatId);
      else next.delete(chatId);
      return next;
    });
  }, []);

  const renameChat = useCallback(
    async (chat: ChatSpec) => {
      let nextName = chat.name || "";
      const confirmed = await Modal.confirm({
        title: t("projects.chat.rename"),
        content: (
          <TextInput
            aria-label={t("projects.chat.name")}
            allowClear
            autoFocus
            clearLabel={t("common.clear")}
            defaultValue={nextName}
            onChange={(event) => {
              nextName = event.target.value;
            }}
            onClear={() => {
              nextName = "";
            }}
          />
        ),
        okText: t("common.save"),
        cancelText: t("common.cancel"),
      });
      const trimmed = nextName.trim();
      if (!confirmed || !trimmed || trimmed === (chat.name || "").trim()) {
        return;
      }

      const optimistic = { ...chat, name: trimmed };
      knownChatsRef.current.set(chat.id, optimistic);
      setChats((current) =>
        current.map((item) => (item.id === chat.id ? optimistic : item)),
      );
      setChatPending(chat.id, true);
      try {
        const updated = await projectChatsApi.updateChat(projectId, chat.id, {
          name: trimmed,
        });
        knownChatsRef.current.set(updated.id, updated);
        const leavesCurrentFilter =
          tagFilter !== null && updated.tag !== tagFilter;
        setChats((current) =>
          leavesCurrentFilter
            ? current.filter((item) => item.id !== updated.id)
            : current.map((item) => (item.id === updated.id ? updated : item)),
        );
        if (leavesCurrentFilter) {
          setTotal((current) => Math.max(0, current - 1));
          setNextOffset((current) => Math.max(0, current - 1));
        }
        onChatUpdated(updated);
      } catch (updateError) {
        knownChatsRef.current.set(chat.id, chat);
        setChats((current) =>
          current.map((item) =>
            item.id === chat.id && item.name === optimistic.name
              ? { ...item, name: chat.name }
              : item,
          ),
        );
        toast.error(errorMessage(updateError, t("projects.chat.renameFailed")));
      } finally {
        setChatPending(chat.id, false);
      }
    },
    [onChatUpdated, projectId, setChatPending, t, tagFilter],
  );

  const togglePin = useCallback(
    async (chat: ChatSpec) => {
      const optimistic = {
        ...chat,
        pinned: !chat.pinned,
        pin_order: chat.pinned ? 0 : Date.now(),
      };
      knownChatsRef.current.set(chat.id, optimistic);
      setChats((current) =>
        current.map((item) => (item.id === chat.id ? optimistic : item)),
      );
      setChatPending(chat.id, true);
      try {
        const updated = await projectChatsApi.updateChat(projectId, chat.id, {
          pinned: optimistic.pinned,
          pin_order: optimistic.pin_order,
        });
        knownChatsRef.current.set(updated.id, updated);
        setChats((current) =>
          current.map((item) => (item.id === updated.id ? updated : item)),
        );
        onChatUpdated(updated);
      } catch (updateError) {
        knownChatsRef.current.set(chat.id, chat);
        setChats((current) =>
          current.map((item) =>
            item.id === chat.id &&
            item.pinned === optimistic.pinned &&
            item.pin_order === optimistic.pin_order
              ? {
                  ...item,
                  pinned: chat.pinned,
                  pin_order: chat.pin_order,
                }
              : item,
          ),
        );
        toast.error(errorMessage(updateError, t("projects.chat.updateFailed")));
      } finally {
        setChatPending(chat.id, false);
      }
    },
    [onChatUpdated, projectId, setChatPending, t],
  );

  const cycleTag = useCallback(
    async (chat: ChatSpec) => {
      const optimistic = { ...chat, tag: nextChatTag(chat.tag) };
      knownChatsRef.current.set(chat.id, optimistic);
      setChats((current) =>
        current.map((item) => (item.id === chat.id ? optimistic : item)),
      );
      setChatPending(chat.id, true);
      try {
        const updated = await projectChatsApi.updateChat(projectId, chat.id, {
          tag: optimistic.tag,
        });
        knownChatsRef.current.set(updated.id, updated);
        setChats((current) =>
          current.map((item) => (item.id === updated.id ? updated : item)),
        );
        onChatUpdated(updated);
      } catch (updateError) {
        knownChatsRef.current.set(chat.id, chat);
        setChats((current) =>
          current.map((item) =>
            item.id === chat.id && item.tag === optimistic.tag ? chat : item,
          ),
        );
        toast.error(errorMessage(updateError, t("projects.chat.updateFailed")));
      } finally {
        setChatPending(chat.id, false);
      }
    },
    [onChatUpdated, projectId, setChatPending, t],
  );

  const deleteChat = useCallback(
    async (chat: ChatSpec) => {
      const title = chatTitle(chat);
      const busy =
        chat.status === "running" ||
        (chat.id === currentChatId && currentChatRunning);
      const confirmed = await Modal.confirm({
        title: busy
          ? t("projects.chat.deleteRunningTitle")
          : t("projects.chat.deleteTitle"),
        content: busy
          ? t("projects.chat.busyDeleteConfirm", { title })
          : t("projects.chat.deleteConfirm", { title }),
        okText: busy ? t("projects.chat.stopAndDelete") : t("common.delete"),
        cancelText: t("common.cancel"),
        danger: true,
      });
      if (!confirmed) return;

      setChatPending(chat.id, true);
      try {
        try {
          await projectChatsApi.deleteChat(projectId, chat.id, busy);
        } catch (deleteError) {
          if (busy || !isBusyDeleteConflict(deleteError)) throw deleteError;
          const forceConfirmed = await Modal.confirm({
            title: t("projects.chat.becameBusy"),
            content: t("projects.chat.busyDeleteConfirm", { title }),
            okText: t("projects.chat.stopAndDelete"),
            cancelText: t("common.cancel"),
            danger: true,
          });
          if (!forceConfirmed) return;
          await projectChatsApi.deleteChat(projectId, chat.id, true);
        }
        knownChatsRef.current.delete(chat.id);
        let remainingKnownChats = [...knownChatsRef.current.values()].filter(
          (chat) => !chat.archived,
        );
        if (chat.id === currentChatId && remainingKnownChats.length === 0) {
          try {
            const fallback = await projectChatsApi.listChats(projectId, {
              offset: 0,
              limit: PAGE_SIZE,
            });
            remainingKnownChats = fallback.data.filter(
              (candidate) => candidate.id !== chat.id,
            );
          } catch {
            // The deleted Chat still disappears even if fallback discovery is unavailable.
          }
        }
        setChats((current) => current.filter((item) => item.id !== chat.id));
        setTotal((current) => Math.max(0, current - 1));
        setNextOffset((current) =>
          Math.max(0, current - (chat.pinned ? 0 : 1)),
        );
        onChatDeleted(chat, remainingKnownChats);
      } catch (deleteError) {
        toast.error(errorMessage(deleteError, t("projects.chat.deleteFailed")));
      } finally {
        setChatPending(chat.id, false);
      }
    },
    [
      currentChatId,
      currentChatRunning,
      onChatDeleted,
      projectId,
      setChatPending,
      t,
    ],
  );

  const isSearchActive =
    !archived && conversationView === "user" && searchInput.trim().length > 0;
  const hasMore =
    nextOffset + chats.filter((chat) => chat.pinned).length < total;

  const changeArchive = async (chat: ChatSpec) => {
    if (readOnly || clearingRef.current || pendingChatIds.has(chat.id)) return;
    setChatPending(chat.id, true);
    try {
      const updated = await projectChatsApi.updateChat(projectId, chat.id, {
        archived: !archived,
      });
      knownChatsRef.current.set(chat.id, updated);
      onChatUpdated(updated);
      await loadChats(0, Math.max(PAGE_SIZE, nextOffset), false, true);
    } catch (error) {
      toast.error(errorMessage(error, t("projects.chat.updateFailed")));
    } finally {
      setChatPending(chat.id, false);
    }
  };

  const clearArchive = async () => {
    if (readOnly || clearingRef.current) return;
    clearingRef.current = true;
    setClearing(true);
    try {
      if (
        !(await Modal.confirm({
          title: t("projects.chat.clearArchive"),
          content: t("projects.chat.clearArchiveConfirm"),
          okText: t("common.delete"),
          cancelText: t("common.cancel"),
          danger: true,
        }))
      )
        return;
      const result = await projectChatsApi.clearArchive(projectId, {
        view: "all",
      });
      for (const id of result.deleted) knownChatsRef.current.delete(id);
      if (result.deleted.includes(currentChatId)) {
        const active = await projectChatsApi.listChats(projectId, {
          archived: false,
          offset: 0,
          limit: PAGE_SIZE,
        });
        onChatDeleted({ id: currentChatId } as ChatSpec, active.data);
      }
      await loadChats(0, PAGE_SIZE, false);
      if (result.skipped.length)
        toast.error(
          t("projects.chat.clearArchivePartial", {
            count: result.skipped.length,
          }),
        );
    } catch (error) {
      toast.error(errorMessage(error, t("projects.chat.deleteFailed")));
    } finally {
      clearingRef.current = false;
      setClearing(false);
    }
  };
  return (
    <ChatHistoryDrawer
      inline={inline}
      projectArchive={{
        archived,
        disabled: readOnly || clearing || loading,
        onToggle: () => {
          setArchived((value) => !value);
          setSearchInput("");
        },
        onChange: changeArchive,
        onClear: clearArchive,
      }}
      open
      title={t("projects.chat.title")}
      currentChatId={currentChatId}
      conversationView={conversationView}
      onConversationViewChange={(view) => {
        setConversationView(view);
        setSearchInput("");
        setChats([]);
        setTotal(0);
        setNextOffset(0);
      }}
      collaborationFilters={collaborationFilters}
      onCollaborationFiltersChange={(filters) => {
        setCollaborationFilters(filters);
        setChats([]);
        setTotal(0);
        setNextOffset(0);
      }}
      chats={chats}
      loading={loading}
      loadingMore={loadingMore}
      hasMore={hasMore}
      searchValue={searchInput}
      searchContent={
        isSearchActive ? (
          <ChatSearchResults
            ownerKey={"project:" + projectId}
            query={searchInput}
            search={searchChats}
            onResultClick={(chatId, title) =>
              onSelectChat({ id: chatId, name: title || chatId } as ChatSpec)
            }
          />
        ) : undefined
      }
      searchLabel={t("projects.chat.search")}
      searchPlaceholder={t("projects.chat.searchPlaceholder")}
      sort={sort}
      tagFilter={tagFilter}
      sortDisabled={
        conversationView === "user" && searchInput.trim().length > 0
      }
      emptyLabel={error || t("projects.chat.noSaved")}
      createLabel={t("projects.chat.new")}
      createDisabled={
        readOnly || archived || conversationView === "collaboration"
      }
      loadMoreLabel={t("projects.chat.loadMore")}
      loadingMoreLabel={t("projects.chat.loading")}
      isActionDisabled={(chat) =>
        readOnly || clearing || pendingChatIds.has(chat.id)
      }
      renameLabel={(chat) =>
        t("projects.chat.renameAction", { title: chatTitle(chat) })
      }
      pinLabel={(chat) =>
        t(
          chat.pinned ? "projects.chat.unpinAction" : "projects.chat.pinAction",
          { title: chatTitle(chat) },
        )
      }
      deleteLabel={(chat) =>
        t("projects.chat.deleteAction", { title: chatTitle(chat) })
      }
      onClose={onClose}
      onSearchChange={setSearchInput}
      onSortChange={setSort}
      onTagFilterChange={setTagFilter}
      onSelect={onSelectChat}
      onRename={renameChat}
      onTogglePin={togglePin}
      onCycleTag={cycleTag}
      onDelete={deleteChat}
      onLoadMore={() => void loadChats(nextOffset, PAGE_SIZE, true)}
      formatDate={(raw) =>
        formatChatActivity(raw, t("projects.chat.noActivity"))
      }
      titleForChat={chatTitle}
    />
  );
}

function appendUniqueChats(
  current: ChatSpec[],
  incoming: ChatSpec[],
): ChatSpec[] {
  const known = new Set(current.map((chat) => chat.id));
  return [...current, ...incoming.filter((chat) => !known.has(chat.id))];
}

function chatTitle(chat: ChatSpec): string {
  return chat.name?.trim() || chat.id;
}

function formatChatActivity(
  raw: string | null | undefined,
  noActivityLabel: string,
): string {
  if (!raw) return noActivityLabel;
  const date = new Date(raw);
  if (Number.isNaN(date.getTime())) return noActivityLabel;
  return date.toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

function isBusyDeleteConflict(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  const message = error.message.toLocaleLowerCase();
  return message.includes("busy") || message.includes("running");
}
