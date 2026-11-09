import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertCircle,
  ChevronDown,
  ListTodo,
  Bot,
  CircleCheck,
  CircleDashed,
  Hourglass,
  Inbox,
  PauseCircle,
  ShieldAlert,
  TriangleAlert,
  Wrench,
} from "lucide-react";
import type {
  ConversationBackgroundWorkItem,
  ConversationBackgroundWorkView,
} from "@/api/types";
import type { TaskChatAddress } from "@/api/types/task";

type BackgroundWorkStatusProps = {
  onOpenTasks?: () => void;
  /** Fact from the conversation control snapshot; never inferred from messages. */
  view?: ConversationBackgroundWorkView | null;
  /** True while a Turn is active: show the parallel count instead of the wait list. */
  turnActive: boolean;
  /** Present when the item's chat is reachable through existing navigation. */
  onOpenChat?: (chat: TaskChatAddress) => void;
  /** Chat currently rendered; items pointing at it are not navigable. */
  currentChatId?: string | null;
};

/** Statuses the display differentiates explicitly; anything else shows its raw token. */
const DIFFERENTIATED_STATUSES = new Set([
  "waiting_approval",
  "result_ready",
  "delivery_failed",
]);

type BackgroundWorkNote = {
  tone: "suppressed" | "error";
  label?: string;
  detail?: string;
};

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 8)}...${id.slice(-4)}` : id;
}

function statusTone(status: string): string {
  if (status === "waiting_approval") return "approval";
  if (status === "result_ready") return "ready";
  if (status === "delivery_failed") return "failed";
  return "pending";
}

export default function BackgroundWorkStatus({
  onOpenTasks,
  view,
  turnActive,
  onOpenChat,
  currentChatId,
}: BackgroundWorkStatusProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const tx = (key: string, fallback: string) => {
    const translated = t(key);
    return translated === key ? fallback : translated;
  };
  if (!view) return null;
  const items = view.items ?? [];
  const suppressed = view.suppressed === true;
  const error =
    typeof view.error === "string" && view.error ? view.error : null;
  if (items.length === 0 && !error) return null;

  const waiting = !turnActive && items.length > 0;
  const statusLabel = (status: string) => {
    if (status === "waiting_approval")
      return tx("chat.backgroundWork.status.waitingApproval", "Needs approval");
    if (status === "result_ready")
      return tx("chat.backgroundWork.status.resultReady", "Result ready");
    if (status === "delivery_failed")
      return tx("chat.backgroundWork.status.deliveryFailed", "Delivery failed");
    return tx(`chat.backgroundWork.status.${status}`, status);
  };
  const kindLabel = (kind: ConversationBackgroundWorkItem["kind"]) =>
    kind === "collaboration"
      ? tx("chat.backgroundWork.kind.collaboration", "Collaboration")
      : tx("chat.backgroundWork.kind.task", "Task");
  const statusIcon = (item: ConversationBackgroundWorkItem) => {
    switch (item.status) {
      case "waiting_approval":
        return <ShieldAlert size={14} />;
      case "result_ready":
        return <CircleCheck size={14} />;
      case "delivery_failed":
        return <TriangleAlert size={14} />;
      default:
        return <CircleDashed size={14} />;
    }
  };
  const waitingLabel = tx(
    "chat.backgroundWork.waiting",
    "Waiting for background work",
  );
  const parallelLabel = tx(
    "chat.backgroundWork.parallel",
    "Background work running in parallel",
  );
  const suppressedLabel = tx(
    "chat.backgroundWork.suppressed",
    "Auto-resume stopped",
  );
  const unavailableLabel = tx(
    "chat.backgroundWork.unavailable",
    "Background work unavailable",
  );
  const hasItems = items.length > 0;
  // An empty work list is visible only when the snapshot reports an error.
  const fact: "waiting" | "parallel" | "unavailable" = hasItems
    ? waiting
      ? "waiting"
      : "parallel"
    : "unavailable";
  const headline =
    fact === "waiting"
      ? waitingLabel
      : fact === "parallel"
      ? parallelLabel
      : unavailableLabel;
  const notes: BackgroundWorkNote[] = [];
  if (suppressed) {
    notes.push({ tone: "suppressed", label: suppressedLabel });
  }
  if (error && fact !== "unavailable") {
    notes.push({ tone: "error", label: unavailableLabel, detail: error });
  } else if (error) {
    notes.push({ tone: "error", detail: error });
  }
  const headlineIcon =
    fact === "waiting" ? (
      <Hourglass size={15} />
    ) : fact === "parallel" ? (
      <Inbox size={15} />
    ) : (
      <AlertCircle size={15} />
    );
  // Announced text: the headline plus every note that is not already the
  // headline, so suppressed/unavailable facts reach assistive tech too.
  const liveLabel = [
    hasItems
      ? t("chat.backgroundWork.summary", {
          defaultValue: `${headline} (${items.length})`,
          label: headline,
          count: items.length,
        })
      : headline,
    ...notes.map((note) =>
      [note.label, note.detail].filter(Boolean).join(": "),
    ),
  ]
    .filter(Boolean)
    .join(", ");

  return (
    <aside
      className={[
        "rua-owned-background-work",
        waiting && "rua-owned-background-work--waiting",
        error && "rua-owned-background-work--error",
      ]
        .filter(Boolean)
        .join(" ")}
      data-rua-background-work={fact}
      data-rua-background-work-count={items.length}
    >
      <div className="rua-owned-background-work-row">
        {/* The live region covers the announced facts only; interactive controls
          sit outside it so updates never re-announce them. */}
        <div
          className="rua-owned-background-work-summary"
          role="status"
          aria-label={liveLabel}
        >
          <span className="rua-owned-background-work-icon" aria-hidden="true">
            {headlineIcon}
          </span>
          <span
            className="rua-owned-background-work-headline"
            title={liveLabel}
          >
            {liveLabel}
          </span>
          {expanded &&
            notes.map((note, index) => (
              <span
                key={`${note.tone}-${index}`}
                className={`rua-owned-background-work-note is-${note.tone}`}
              >
                {note.tone === "suppressed" ? (
                  <PauseCircle size={14} aria-hidden="true" />
                ) : (
                  <AlertCircle size={14} aria-hidden="true" />
                )}
                {note.label && <span>{note.label}</span>}
                {note.detail && (
                  <span className="rua-owned-background-work-note-detail">
                    {note.detail}
                  </span>
                )}
              </span>
            ))}
        </div>
        {items.some((item) => item.kind === "task") && onOpenTasks && (
          <button
            type="button"
            className="rua-owned-background-work-action"
            onClick={onOpenTasks}
            aria-label={tx(
              "chat.backgroundWork.openTasks",
              "View background Tasks",
            )}
            title={tx("chat.backgroundWork.openTasks", "View background Tasks")}
          >
            <ListTodo size={15} aria-hidden="true" />
          </button>
        )}
        {hasItems && (
          <button
            type="button"
            className="rua-owned-background-work-action"
            aria-expanded={expanded}
            aria-label={tx(
              "chat.backgroundWork.details",
              "Background work details",
            )}
            title={tx("chat.backgroundWork.details", "Background work details")}
            onClick={() => setExpanded(!expanded)}
          >
            <ChevronDown
              size={15}
              aria-hidden="true"
              style={{ transform: expanded ? "rotate(180deg)" : undefined }}
            />
          </button>
        )}
      </div>
      {hasItems && expanded && (
        <ul
          className="rua-owned-background-work-list"
          aria-label={tx("chat.backgroundWork.list", "Background work items")}
        >
          {items.map((item) => {
            const navigable = Boolean(
              onOpenChat &&
                item.kind === "collaboration" &&
                item.chat?.chat_id &&
                item.chat.chat_id !== currentChatId,
            );
            const kind = kindLabel(item.kind);
            const status = statusLabel(item.status);
            const openLabel = tx(
              "chat.backgroundWork.open",
              "Open conversation",
            );
            const content = (
              <>
                <span
                  className="rua-owned-background-work-item-icon"
                  aria-hidden="true"
                >
                  {statusIcon(item)}
                </span>
                <span className="rua-owned-background-work-item-kind">
                  {item.kind === "collaboration" ? (
                    <Bot size={13} aria-hidden="true" />
                  ) : (
                    <Wrench size={13} aria-hidden="true" />
                  )}
                  {kind}
                </span>
                <span
                  className="rua-owned-background-work-item-id"
                  title={item.id}
                >
                  {shortId(item.id)}
                </span>
                <span
                  className={`rua-owned-background-work-item-status is-${statusTone(
                    item.status,
                  )}`}
                  data-status={item.status}
                  data-differentiated={DIFFERENTIATED_STATUSES.has(item.status)}
                >
                  {status}
                </span>
              </>
            );
            return (
              <li
                key={`${item.kind}:${item.id}`}
                className="rua-owned-background-work-item-row"
              >
                {navigable ? (
                  <button
                    type="button"
                    className="rua-owned-background-work-item rua-owned-background-work-item--link"
                    title={openLabel}
                    aria-label={`${kind} ${item.id}, ${status}, ${openLabel}`}
                    onClick={() => onOpenChat?.(item.chat)}
                  >
                    {content}
                  </button>
                ) : (
                  <div className="rua-owned-background-work-item">
                    {content}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
}
