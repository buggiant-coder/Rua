import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import api from "../../../api";
import { projectChatsApi } from "@/api/modules/projectChats";
import { invalidateConversationNavigation } from "@/layouts/conversationNavigationEvents";
import type { ChatUpdateRequest } from "../../../api/types";
import type { Session } from "./components/constants";
import { useAgentStore } from "../../../stores/agentStore";
import { useTranslation } from "react-i18next";
import { toast } from "@/ui";

export const SESSIONS_PAGE_SIZE = 10;

export interface SessionsInitialData {
  agentId: string;
  sessions: Session[];
  total: number;
}

export function useSessions(
  initialData?: SessionsInitialData,
  projectId?: string,
) {
  const { t } = useTranslation();
  const { selectedAgent } = useAgentStore();
  const seed =
    !projectId && initialData?.agentId === selectedAgent
      ? initialData
      : undefined;
  const [sessions, setSessions] = useState<Session[]>(seed?.sessions ?? []);
  const [loading, setLoading] = useState(seed === undefined);
  const [total, setTotal] = useState(seed?.total ?? 0);
  const [page, setPage] = useState(1);
  const requestId = useRef(0);
  const invalidateNavigation = (chatId: string) => {
    invalidateConversationNavigation(
      projectId
        ? { scope: "project", projectId, chatId }
        : { scope: "agent", agentId: selectedAgent, chatId },
    );
  };

  const pinnedCount = useMemo(() => {
    return sessions.filter((session) => session.pinned).length;
  }, [sessions]);
  const unpinnedTotal = Math.max(0, total - pinnedCount);

  const fetchSessions = useCallback(
    async (nextPage = 1) => {
      const currentRequest = ++requestId.current;
      setLoading(true);
      try {
        const params = {
          offset: (nextPage - 1) * SESSIONS_PAGE_SIZE,
          limit: SESSIONS_PAGE_SIZE,
        };
        const response = projectId
          ? await projectChatsApi.listChats(projectId, params)
          : await api.listChats(params, selectedAgent);
        if (currentRequest === requestId.current && response) {
          setPage(nextPage);
          setTotal(response.total);
          setSessions(response.data as Session[]);
        }
      } catch (error) {
        if (currentRequest !== requestId.current) return;
        console.error("❌ Failed to load sessions:", error);
        setSessions([]);
        setTotal(0);
        setPage(1);
      } finally {
        if (currentRequest === requestId.current) setLoading(false);
      }
    },
    [selectedAgent, projectId],
  );

  const refreshAfterDelete = useCallback(
    async (deletedCount: number) => {
      const nextTotal = Math.max(0, total - deletedCount);
      const maxPage = Math.max(1, Math.ceil(nextTotal / SESSIONS_PAGE_SIZE));
      await fetchSessions(Math.min(page, maxPage));
    },
    [fetchSessions, page, total],
  );

  useEffect(() => {
    setPage(1);
    setSessions(seed?.sessions ?? []);
    setTotal(seed?.total ?? 0);
    if (seed) {
      setLoading(false);
    } else {
      void fetchSessions();
    }
    return () => {
      requestId.current += 1;
    };
  }, [fetchSessions, seed]);

  const updateSession = async (chatId: string, values: ChatUpdateRequest) => {
    try {
      const result = projectId
        ? await projectChatsApi.updateChat(projectId, chatId, {
            name: values.name,
          })
        : await api.updateChat(chatId, values, selectedAgent);
      setSessions((items) =>
        items.map((s) => (s.id === chatId ? (result as Session) : s)),
      );
      toast.success(t("sessions.saveSuccess"));
      return true;
    } catch (error) {
      console.error("❌ Failed to save session:", error);
      toast.error(t("sessions.saveFailed"));
      return false;
    }
  };

  const deleteSession = async (chatId: string) => {
    try {
      if (projectId) await projectChatsApi.deleteChat(projectId, chatId);
      else await api.deleteChat(chatId, selectedAgent);
      invalidateNavigation(chatId);
      await refreshAfterDelete(1);
      toast.success(t("sessions.deleteSuccess"));
      return true;
    } catch (error) {
      console.error("❌ Failed to delete session:", error);
      toast.error(t("sessions.deleteFailed"));
      return false;
    }
  };

  const batchDeleteSessions = async (chatIds: string[]) => {
    try {
      let deletedCount: number;
      if (projectId) {
        const results = await Promise.allSettled(
          chatIds.map((chatId) =>
            projectChatsApi.deleteChat(projectId, chatId),
          ),
        );
        deletedCount = results.filter(
          (result) => result.status === "fulfilled",
        ).length;
        const deletedIndex = results.findIndex(
          (result) => result.status === "fulfilled",
        );
        if (deletedIndex !== -1) invalidateNavigation(chatIds[deletedIndex]);
        await refreshAfterDelete(deletedCount);
        if (deletedCount !== chatIds.length) {
          toast.error(t("sessions.batchDeleteFailed"));
          return false;
        }
      } else {
        const result = await api.batchDeleteChats(chatIds, selectedAgent);
        deletedCount = result.deleted_count ?? chatIds.length;
        if (deletedCount > 0 && chatIds.length > 0)
          invalidateNavigation(chatIds[0]);
        await refreshAfterDelete(deletedCount);
      }
      toast.success(
        t("sessions.batchDeleteSuccess", { count: chatIds.length }),
      );
      return true;
    } catch (error) {
      console.error("❌ Failed to batch delete sessions:", error);
      toast.error(t("sessions.batchDeleteFailed"));
      return false;
    }
  };

  return {
    sessions,
    loading,
    page,
    pageSize: SESSIONS_PAGE_SIZE,
    total,
    unpinnedTotal,
    fetchSessions,
    updateSession,
    deleteSession,
    batchDeleteSessions,
  };
}
