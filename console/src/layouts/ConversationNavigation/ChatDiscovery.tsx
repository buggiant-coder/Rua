import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { chatApi, type ChatDiscoveryParams } from "@/api/modules/chat";
import type { ChatSearchResult, ChatSpec } from "@/api/types";
import type { ProjectDescriptor } from "@/api/types/project";
import {
  chatPath,
  type ConversationAddress,
} from "@/pages/Chat/conversationAddress";
import { useAgentStore } from "@/stores/agentStore";
import { getAgentDisplayName } from "@/utils/agentDisplayName";
import { subscribeConversationNavigation } from "../conversationNavigationEvents";
import ConversationRow from "./ConversationRow";
import { openChatSearchResult } from "@/pages/Chat/ChatSearchResults";
import styles from "./index.module.less";

const PAGE_SIZE = 20;
type Result = ChatSpec | ChatSearchResult;

export default function ChatDiscovery({
  projects,
  current,
  getCurrentAddress,
  onNavigate,
  onClose,
}: {
  projects: ProjectDescriptor[];
  current: ConversationAddress | null;
  getCurrentAddress: () => ConversationAddress | null;
  onNavigate: (path: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const agents = useAgentStore((state) => state.agents);
  const activeProjects = useMemo(
    () =>
      new Set(
        projects
          .filter((item) => item.lifecycle === "active")
          .map((item) => item.project_id),
      ),
    [projects],
  );
  const [project, setProject] = useState("all");
  const [agentId, setAgentId] = useState("");
  const [archived, setArchived] = useState(false);
  const [input, setInput] = useState("");
  const query = input.trim();
  const projectId = project.startsWith("project:")
    ? project.slice("project:".length)
    : undefined;
  const standalone = project === "standalone";
  const [results, setResults] = useState<Result[]>([]);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const request = useRef(0);
  const pending = useRef(false);
  const requestedEnd = useRef(PAGE_SIZE);
  const refreshQueued = useRef(false);
  const retry = useRef({ offset: 0, limit: PAGE_SIZE });

  const load = useCallback(
    async function loadPage(
      offset: number,
      limit = PAGE_SIZE,
      background = false,
    ) {
      if (background && pending.current) {
        refreshQueued.current = true;
        return;
      }
      refreshQueued.current = false;
      requestedEnd.current = offset + limit;
      const revision = ++request.current;
      pending.current = true;
      retry.current = { offset, limit };
      if (!background) setLoading(true);
      setError("");
      const filters: ChatDiscoveryParams = { view: "user", archived };
      if (agentId) filters.agent_id = agentId;
      if (projectId) filters.project_id = projectId;
      if (standalone) filters.without_project = true;
      try {
        const rows: Result[] = [];
        let cursor = offset;
        let next: number | null;
        do {
          const params = {
            ...filters,
            offset: cursor,
            limit: Math.min(200, offset + limit - cursor),
          };
          if (query) {
            const page = await chatApi.searchChats(query, params);
            rows.push(...page.results);
            next = page.next_offset;
          } else {
            const page = await chatApi.listChats(params);
            rows.push(...page.data);
            next =
              page.data.length > 0 && cursor + page.data.length < page.total
                ? cursor + page.data.length
                : null;
          }
          if (request.current !== revision) return;
          if (
            next == null ||
            next <= cursor ||
            next >= offset + limit ||
            limit === PAGE_SIZE
          )
            break;
          cursor = next;
        } while (true);
        setResults((previous) => {
          const values = new Map<string, Result>();
          for (const row of [...(offset ? previous : []), ...rows]) {
            const key =
              "chat_id" in row
                ? JSON.stringify([
                    row.chat_id,
                    row.message_id ?? row.card_index,
                  ])
                : row.id;
            values.set(key, row);
          }
          return [...values.values()];
        });
        setNextOffset(next);
      } catch (cause) {
        if (request.current === revision)
          setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        if (request.current === revision) {
          pending.current = false;
          setLoading(false);
          if (refreshQueued.current)
            void loadPage(0, requestedEnd.current, true);
        }
      }
    },
    [agentId, archived, projectId, query, standalone],
  );

  useEffect(() => {
    requestedEnd.current = PAGE_SIZE;
    refreshQueued.current = false;
    pending.current = false;
    setResults([]);
    setNextOffset(null);
    setError("");
    setLoading(true);
    const timer = window.setTimeout(() => void load(0), query ? 300 : 0);
    const unsubscribe = subscribeConversationNavigation(
      (_address, background, changedProject) => {
        if (
          changedProject !== undefined &&
          ((standalone && changedProject !== null) ||
            (projectId && changedProject !== projectId))
        )
          return;
        void load(0, requestedEnd.current, background);
      },
    );
    return () => {
      window.clearTimeout(timer);
      request.current += 1;
      unsubscribe();
    };
  }, [load, projectId, query, standalone]);

  return (
    <section role="region" aria-label={t("navigation.discovery")}>
      <div className={styles.searchControls}>
        <div className={styles.searchScopeRow}>
          <input
            type="search"
            autoFocus
            aria-label={t("navigation.search")}
            placeholder={t("navigation.search")}
            value={input}
            onChange={(event) => setInput(event.target.value)}
          />
          <button
            type="button"
            className={styles.iconButton}
            aria-label={t("navigation.closeSearch")}
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </div>
        <div className={styles.discoveryFilters}>
          <select
            aria-label={t("navigation.projectFilter")}
            value={project}
            onChange={(event) => setProject(event.target.value)}
          >
            <option value="all">{t("navigation.allProjects")}</option>
            <option value="standalone">
              {t("navigation.independentChats")}
            </option>
            {projects.map((item) => (
              <option
                key={item.project_id}
                value={`project:${item.project_id}`}
              >
                {item.display_name}
                {item.lifecycle === "archived"
                  ? ` (${t("projects.archived")})`
                  : ""}
              </option>
            ))}
          </select>
          <select
            aria-label={t("navigation.agentFilter")}
            value={agentId}
            onChange={(event) => setAgentId(event.target.value)}
          >
            <option value="">{t("navigation.allAgents")}</option>
            {agents.map((agent) => (
              <option key={agent.id} value={agent.id}>
                {getAgentDisplayName(agent)}
              </option>
            ))}
          </select>
          <select
            aria-label={t("navigation.archiveFilter")}
            value={archived ? "archived" : "active"}
            onChange={(event) => setArchived(event.target.value === "archived")}
          >
            <option value="active">{t("projects.active")}</option>
            <option value="archived">{t("projects.archived")}</option>
          </select>
        </div>
      </div>
      <div className={styles.searchResults}>
        {results.map((row) =>
          "chat_id" in row ? (
            <button
              type="button"
              key={JSON.stringify([
                row.chat_id,
                row.message_id ?? row.card_index,
              ])}
              className={styles.searchResult}
              onClick={() =>
                openChatSearchResult(row, query, (id) =>
                  onNavigate(chatPath(id)),
                )
              }
            >
              <span>{row.chat_name || t("navigation.untitled")}</span>
              <small>{row.snippet}</small>
            </button>
          ) : (
            <ConversationRow
              key={row.id}
              scope={
                row.project_id
                  ? { kind: "project", id: row.project_id }
                  : { kind: "standalone" }
              }
              chat={row}
              readOnly={Boolean(
                row.project_id && !activeProjects.has(row.project_id),
              )}
              current={current}
              getCurrentAddress={getCurrentAddress}
              onNavigate={onNavigate}
              onManage={() => onNavigate(`${chatPath(row.id)}?conversations=1`)}
              onRemoved={(id) =>
                setResults((previous) =>
                  previous.filter((hit) => "chat_id" in hit || hit.id !== id),
                )
              }
            />
          ),
        )}
        {loading && (
          <p className={styles.status} role="status">
            {t("common.loading")}
          </p>
        )}
        {error && (
          <div className={styles.error} role="alert">
            <span title={error}>{t("navigation.loadFailed")}</span>
            <button
              type="button"
              onClick={() =>
                void load(retry.current.offset, retry.current.limit)
              }
            >
              {t("common.retry")}
            </button>
          </div>
        )}
        {!loading && !error && results.length === 0 && (
          <p className={styles.status}>{t("chat.noSearchResults")}</p>
        )}
        {!loading && !error && nextOffset != null && (
          <button
            type="button"
            className={styles.loadMore}
            onClick={() => {
              if (pending.current) void load(0, nextOffset + PAGE_SIZE);
              else void load(nextOffset);
            }}
          >
            {t("projects.chat.loadMore")}
          </button>
        )}
      </div>
    </section>
  );
}
