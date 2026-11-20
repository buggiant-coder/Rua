import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type RefObject,
} from "react";
import { useTranslation } from "react-i18next";
import { chatApi } from "@/api/modules/chat";
import type {
  ChatAgentState,
  ChatAgentSwitchBlocker,
  ChatHistory,
} from "@/api/types";
import { invalidateConversationNavigation } from "@/layouts/conversationNavigationEvents";
import { useAgentStore } from "@/stores/agentStore";
import { toast } from "@/ui";
import { saveDraftChatContext } from "./chatContext";

const blockerKeys: Record<ChatAgentSwitchBlocker, string> = {
  active_turn: "activeTurn",
  queued_input: "queuedInput",
  unresolved_approval: "unresolvedApproval",
  unanswered_prompt: "unansweredPrompt",
  active_acp_binding: "activeAcp",
  archived_chat: "archivedChat",
  archived_project: "archivedProject",
  fixed_collaboration_executor: "fixedCollaborationExecutor",
};

function stateFromHistory(history?: ChatHistory | null): ChatAgentState | null {
  return (
    history?.control?.chat_agent_state ??
    history?.meta?.chat_agent_state ??
    null
  );
}

function visibleBlocker(
  history?: ChatHistory | null,
): ChatAgentSwitchBlocker | undefined {
  if (history?.meta?.collaboration) return "fixed_collaboration_executor";
  if (history?.control?.active_turn ?? history?.active_turn)
    return "active_turn";
  if (!history?.control && history?.status === "running") return "active_turn";
  if (history?.control?.queue?.entries.length) return "queued_input";
  if (history?.control?.acp?.active ?? Boolean(history?.meta?.acp_binding))
    return "active_acp_binding";
}

export function useChatAgentSelection(
  chatId: string,
  projectId: string | null,
  initialAgentId: string,
  initialHistory: ChatHistory | null | undefined,
  lazyRef: RefObject<boolean>,
  history: ChatHistory | null,
) {
  const { t } = useTranslation();
  const agents = useAgentStore((s) => s.agents);
  const catalogLoading = useAgentStore(
    (s) => s.catalogLoading || (!s.catalogLoaded && !s.catalogError),
  );
  const ensureCatalogLoaded = useAgentStore((s) => s.ensureCatalogLoaded);
  const [state, setState] = useState(() => stateFromHistory(initialHistory));
  const latestState = useRef(state);
  const [historyBlocker, setHistoryBlocker] = useState(() =>
    visibleBlocker(initialHistory),
  );
  const observedControlRevision = useRef(initialHistory?.control?.revision);
  const [headerAgent, setHeaderAgent] = useState(initialAgentId);
  const [switching, setSwitching] = useState(false);
  const [checking, setChecking] = useState(false);
  const [readError, setReadError] = useState("");
  const readSequence = useRef(0);
  const selectedAgentId = state?.selection.agent_id ?? headerAgent;
  useEffect(() => {
    void ensureCatalogLoaded().catch(() =>
      toast.error(t("projects.chat.agentLoadFailed")),
    );
  }, [ensureCatalogLoaded, t]);

  const acceptState = useCallback(
    (next: ChatAgentState, replaceEqual = true) => {
      const previous = latestState.current;
      if (
        previous &&
        (next.selection.revision < previous.selection.revision ||
          (!replaceEqual &&
            next.selection.revision === previous.selection.revision))
      )
        return;
      latestState.current = next;
      setState(next);
      if (
        (previous?.selection.agent_id ?? initialAgentId) !==
        next.selection.agent_id
      ) {
        invalidateConversationNavigation({ chatId }, false, projectId);
      }
    },
    [chatId, initialAgentId, projectId],
  );
  const observeHistory = useCallback(
    (next: ChatHistory) => {
      const selection = stateFromHistory(next);
      if (next.control) observedControlRevision.current = next.control.revision;
      setHistoryBlocker(visibleBlocker(next));
      if (selection)
        acceptState(selection, Boolean(next.control?.chat_agent_state));
      else if (next.agent_id) setHeaderAgent(next.agent_id);
    },
    [acceptState],
  );

  const refresh = useCallback(async () => {
    if (lazyRef.current) return;
    const sequence = ++readSequence.current;
    const controlAtRead = observedControlRevision.current;
    setChecking(true);
    setReadError("");
    try {
      const next = await chatApi.getAgentState(chatId);
      if (sequence === readSequence.current) {
        const unchanged = observedControlRevision.current === controlAtRead;
        acceptState(next, unchanged);
        if (unchanged) setHistoryBlocker(undefined);
      }
    } catch (error) {
      if (sequence === readSequence.current)
        setReadError(
          error instanceof Error
            ? error.message
            : t("projects.chat.agentLoadFailed"),
        );
    } finally {
      if (sequence === readSequence.current) setChecking(false);
    }
  }, [acceptState, chatId, lazyRef, t]);

  const select = async (agentId: string) => {
    if (lazyRef.current) {
      saveDraftChatContext(chatId, { agentId, projectId });
      useAgentStore.getState().setSelectedAgent(agentId);
      setHeaderAgent(agentId);
      return;
    }
    setSwitching(true);
    try {
      const next = await chatApi.switchAgent(chatId, selectedAgentId, agentId);
      acceptState(next);
      useAgentStore.getState().setSelectedAgent(next.selection.agent_id);
      toast.success(t("projects.chat.agentSwitched"));
    } catch (error) {
      const detail =
        error instanceof Error
          ? error.message
          : t("projects.chat.agentSwitchFailed");
      const reasons = Object.entries(blockerKeys)
        .filter(([wireName]) => detail.includes(wireName))
        .map(([, key]) => t(`projects.chat.switchBlocker.${key}`));
      toast.error(reasons.length ? reasons.join(". ") : detail);
      await refresh();
    } finally {
      setSwitching(false);
    }
  };
  const configured = agents.some(
    (agent) => agent.id === selectedAgentId && agent.enabled,
  );
  const fixedExecutor =
    Boolean(history?.meta?.collaboration) ||
    Boolean(state?.blockers.includes("fixed_collaboration_executor"));
  const blocker = fixedExecutor
    ? "fixed_collaboration_executor"
    : historyBlocker ?? state?.blockers[0];
  const switchBlockedReason =
    blocker && blocker in blockerKeys
      ? t(`projects.chat.switchBlocker.${blockerKeys[blocker]}`)
      : readError || undefined;
  return {
    selectedAgentId,
    fixedExecutor,
    observeHistory,
    restoreDraftAgent: setHeaderAgent,
    submitDisabledReason: catalogLoading
      ? t("projects.chat.resolvingAgent")
      : !configured || state?.selected_agent_available === false
      ? t("projects.chat.selectAvailableAgent")
      : undefined,
    selectorProps: {
      agents,
      selectedAgentId,
      switching,
      loading: catalogLoading || checking,
      switchable:
        !checking &&
        !readError &&
        !blocker &&
        state?.switchable !== false &&
        !catalogLoading,
      switchBlockedReason,
      confirmSelection: !lazyRef.current,
      onSelect: select,
      onOpen: refresh,
    },
  };
}
