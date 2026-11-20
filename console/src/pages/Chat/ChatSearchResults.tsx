import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";

import type { ChatSearchResponse, ChatSearchResult } from "@/api/types";
import { Spinner, toast } from "@/ui";

type ChatSearchResultsProps = {
  query: string;
  search: (query: string, offset: number) => Promise<ChatSearchResponse>;
  onResultClick: (chatId: string, title?: string) => void;
};

function highlightSnippet(snippet: string, query: string): ReactNode {
  const trimmed = query.trim();
  if (!trimmed) return snippet;
  const escaped = trimmed.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const matcher = new RegExp(`(${escaped})`, "gi");
  return snippet.split(matcher).map((part, index) =>
    part.toLowerCase() === trimmed.toLowerCase() ? (
      <mark key={`${part}-${index}`} className="rua-owned-search-mark">
        {part}
      </mark>
    ) : (
      part
    ),
  );
}

export function openChatSearchResult(
  result: ChatSearchResult,
  query: string,
  navigate: (chatId: string, title?: string) => void,
) {
  try {
    sessionStorage.setItem(
      "rua_search_scroll_target",
      JSON.stringify({
        cardIndex: result.card_index,
        messageId: result.message_id,
        turnId: result.turn_id,
        conversationId: result.chat_id,
        ownerKey: "chat",
        query,
      }),
    );
  } catch {}
  navigate(result.chat_id, result.chat_name?.trim() || undefined);
  window.dispatchEvent(new Event("rua-search-scroll"));
}

export default function ChatSearchResults({
  query,
  search,
  onResultClick,
}: ChatSearchResultsProps) {
  const { t } = useTranslation();
  const [results, setResults] = useState<ChatSearchResult[]>([]);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const seqRef = useRef(0);
  const trimmed = query.trim();
  const load = useCallback(
    async (offset: number) => {
      const seq = ++seqRef.current;
      setLoading(true);
      try {
        const page = await search(trimmed, offset);
        if (seq !== seqRef.current) return;
        setResults((current) =>
          offset ? [...current, ...page.results] : page.results,
        );
        setNextOffset(page.next_offset);
        setTotal(page.total);
      } catch (error) {
        if (seq === seqRef.current)
          toast.error(error instanceof Error ? error.message : String(error));
      } finally {
        if (seq === seqRef.current) setLoading(false);
      }
    },
    [search, trimmed],
  );

  useEffect(() => {
    setResults([]);
    setNextOffset(null);
    setLoading(Boolean(trimmed));
    if (!trimmed) return;
    const timer = window.setTimeout(() => void load(0), 300);
    return () => {
      window.clearTimeout(timer);
      seqRef.current += 1;
    };
  }, [load, trimmed]);

  if (!trimmed) return null;
  if (loading && results.length === 0) {
    return (
      <div className="rua-owned-chat-search-spinner">
        <Spinner size="sm" />
      </div>
    );
  }
  if (results.length === 0) {
    return (
      <div className="rua-owned-chat-search-empty">
        {t("chat.noSearchResults")}
      </div>
    );
  }
  return (
    <div className="rua-owned-chat-search-results">
      <div className="rua-owned-chat-search-count">
        {t("chat.searchResultCount", { count: total })}
      </div>
      {results.map((result, index) => (
        <button
          key={`${result.chat_id}-${result.card_index}-${index}`}
          type="button"
          className="rua-owned-chat-search-result"
          onClick={() => openChatSearchResult(result, trimmed, onResultClick)}
        >
          <div className="rua-owned-chat-search-result-header">
            <span className="rua-owned-chat-search-result-title">
              {result.chat_name || result.chat_id}
            </span>
            <span className="rua-owned-chat-search-result-role">
              {result.role}
            </span>
          </div>
          <div className="rua-owned-chat-search-snippet">
            {highlightSnippet(result.snippet, trimmed)}
          </div>
        </button>
      ))}
      {nextOffset != null && (
        <button
          type="button"
          disabled={loading}
          onClick={() => void load(nextOffset)}
        >
          {t("projects.chat.loadMore")}
        </button>
      )}
    </div>
  );
}
