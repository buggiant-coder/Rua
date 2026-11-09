import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import type { ChatSearchResult } from "@/api/types";
import { Spinner, toast } from "@/ui";

type ChatSearchResultsProps = {
  ownerKey: string;
  query: string;
  search: (query: string) => Promise<{ results: ChatSearchResult[] }>;
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

export default function ChatSearchResults({
  ownerKey,
  query,
  search,
  onResultClick,
}: ChatSearchResultsProps) {
  const { t } = useTranslation();
  const [results, setResults] = useState<ChatSearchResult[]>([]);
  const [loading, setLoading] = useState(false);
  const seqRef = useRef(0);

  useEffect(() => {
    const trimmed = query.trim();
    const seq = ++seqRef.current;
    if (!trimmed) {
      setResults([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    const timer = window.setTimeout(() => {
      search(trimmed)
        .then((response) => {
          if (seq === seqRef.current) setResults(response.results || []);
        })
        .catch((error) => {
          if (seq !== seqRef.current) return;
          toast.error(error instanceof Error ? error.message : String(error));
          setResults([]);
        })
        .finally(() => {
          if (seq === seqRef.current) setLoading(false);
        });
    }, 300);
    return () => window.clearTimeout(timer);
  }, [query, search]);

  const trimmed = query.trim();
  if (!trimmed) return null;
  if (loading) {
    return (
      <div className="rua-owned-chat-search-spinner">
        <Spinner size="sm" />
      </div>
    );
  }
  if (results.length === 0) {
    return (
      <div className="rua-owned-chat-search-empty">
        {t("chat.search.noResults")}
      </div>
    );
  }
  return (
    <div className="rua-owned-chat-search-results">
      <div className="rua-owned-chat-search-count">
        {t("chat.search.resultCount", { count: results.length })}
      </div>
      {results.map((result, index) => (
        <button
          key={`${result.chat_id}-${result.card_index}-${index}`}
          type="button"
          className="rua-owned-chat-search-result"
          onClick={() => {
            sessionStorage.setItem(
              "rua_search_scroll_target",
              JSON.stringify({
                cardIndex: result.card_index,
                query: trimmed,
                messageId: result.message_id,
                turnId: result.turn_id,
                conversationId: result.chat_id,
                ownerKey,
              }),
            );
            onResultClick(
              result.chat_id,
              result.chat_name?.trim() || undefined,
            );
            window.dispatchEvent(new Event("rua-search-scroll"));
          }}
        >
          <div className="rua-owned-chat-search-result-header">
            <span className="rua-owned-chat-search-result-title">
              {result.chat_name || result.chat_id}
            </span>
            <span className="rua-owned-chat-search-result-role">
              {result.role === "user" ? t("chat.user") : t("chat.assistant")}
            </span>
          </div>
          <div className="rua-owned-chat-search-snippet">
            {highlightSnippet(result.snippet, trimmed)}
          </div>
        </button>
      ))}
    </div>
  );
}
