import { Bot } from "lucide-react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { agentChatPath, projectChatPath } from "../conversationAddress";
import type { CollaborationActivity } from "./useCollaborationActivity";

function useCollaborationLabel(item: CollaborationActivity) {
  const { t } = useTranslation();
  if (item.status === "idle") return t("chat.tasks.collaboration.ended");
  return t("chat.backgroundWork.status.running");
}

export function CollaborationActivityRow({
  item,
  active,
  onSelect,
}: {
  item: CollaborationActivity;
  active: boolean;
  onSelect: () => void;
}) {
  const { t } = useTranslation();
  const status = useCollaborationLabel(item);
  return (
    <button
      type="button"
      className={`rua-task-list-row${active ? " is-active" : ""}`}
      aria-current={active ? "true" : undefined}
      onClick={onSelect}
    >
      <span className="rua-task-list-row-head">
        <span className="rua-task-field-label">{status}</span>
        {item.chat?.updated_at && (
          <time>{new Date(item.chat.updated_at).toLocaleString()}</time>
        )}
      </span>
      <strong className="rua-task-list-row-title">
        {item.chat?.name || item.address.chat_id}
      </strong>
      <span className="rua-task-list-row-meta">
        <Bot size={14} />
        {t("chat.tasks.collaboration.kind")} ·{" "}
        {item.chat?.collaboration?.executor.kind === "acp"
          ? item.chat.collaboration.executor.runner
          : item.chat?.collaboration?.executor_agent_id ??
            (item.address.scope.kind === "agent"
              ? item.address.scope.agent_id
              : "Agent")}
      </span>
    </button>
  );
}

export function CollaborationActivityDetail({
  item,
  onOpen,
}: {
  item: CollaborationActivity;
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  const status = useCollaborationLabel(item);
  const scope = item.address.scope;
  const href =
    scope.kind === "project"
      ? projectChatPath(scope.project_id, item.address.chat_id)
      : agentChatPath(scope.agent_id, item.address.chat_id);
  return (
    <section
      className="rua-collaboration-detail"
      aria-label={t("chat.tasks.collaboration.kind")}
    >
      <h3>
        <Bot size={20} /> {t("chat.tasks.collaboration.kind")}
      </h3>
      <strong>{item.chat?.name || item.address.chat_id}</strong>
      <p>{status}</p>
      <code>{item.address.chat_id}</code>
      {item.chat?.collaboration && (
        <p>
          {t("chat.tasks.collaboration.executor")}:{" "}
          {item.chat.collaboration.executor.kind === "acp"
            ? item.chat.collaboration.executor.runner
            : item.chat.collaboration.executor_agent_id}
        </p>
      )}
      <p className="rua-task-field-label">
        {t("chat.tasks.collaboration.hint")}
      </p>
      <Link to={href} onClick={onOpen}>
        {t("chat.tasks.collaboration.open")}
      </Link>
    </section>
  );
}
