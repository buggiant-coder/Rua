import type { ReactNode, UIEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  Archive,
  ArchiveRestore,
  Edit3,
  ListFilter,
  Pin,
  PinOff,
  Search,
  Trash2,
} from "lucide-react";

import type { ChatSpec, ChatTag, ConversationListView } from "@/api/types";
import type {
  ChatListSort,
  ChatSortField,
  SortOrder,
} from "@/api/types/chatListSort";
import { Btn, Drawer, Select, Spinner, Tabs, TextInput, Tooltip } from "@/ui";
import { usePullPastBottom } from "@/hooks/usePullPastBottom";
import Dropdown from "@/ui/Dropdown";
import { getChannelLabel } from "../Control/Channels/components/constants";
import { getChannelIconUrl } from "../Control/Channels/components/channelIcons";
import "./owned-chat-page.css";

type ChatHistoryDrawerProps = {
  inline?: boolean;
  projectArchive?: {
    archived: boolean;
    disabled: boolean;
    onToggle: () => void;
    onChange: (chat: ChatSpec) => void | Promise<void>;
    onClear?: () => void | Promise<void>;
  };
  open: boolean;
  title: string;
  currentChatId?: string;
  conversationView?: ConversationListView;
  onConversationViewChange?: (view: ConversationListView) => void;
  collaborationFilters?: {
    sourceChatId: string;
    executor: string;
    status: string;
  };
  onCollaborationFiltersChange?: (filters: {
    sourceChatId: string;
    executor: string;
    status: string;
  }) => void;
  chats: ChatSpec[];
  loading: boolean;
  loadingMore?: boolean;
  hasMore?: boolean;
  searchValue: string;
  searchLabel: string;
  searchPlaceholder: string;
  sort: ChatListSort;
  tagFilter: ChatTag | null;
  sortDisabled?: boolean;
  emptyLabel: string;
  createLabel: string;
  loadMoreLabel: string;
  loadingMoreLabel: string;
  createDisabled?: boolean;
  searchContent?: ReactNode;
  isActionDisabled?: (chat: ChatSpec) => boolean;
  renameLabel?: (chat: ChatSpec) => string;
  pinLabel?: (chat: ChatSpec) => string;
  deleteLabel?: (chat: ChatSpec) => string;
  onClose: () => void;
  onCreate?: () => void;
  onSearchChange: (value: string) => void;
  onSortChange: (sort: ChatListSort) => void;
  onTagFilterChange: (tag: ChatTag | null) => void;
  onSelect: (chat: ChatSpec) => void;
  onRename: (chat: ChatSpec) => void | Promise<void>;
  onTogglePin: (chat: ChatSpec) => void | Promise<void>;
  onCycleTag: (chat: ChatSpec) => void | Promise<void>;
  onDelete: (chat: ChatSpec) => void | Promise<void>;
  onLoadMore?: () => void;
  onScroll?: (event: UIEvent<HTMLDivElement>) => void;
  formatDate: (raw: string | null | undefined) => string;
  titleForChat: (chat: ChatSpec) => string;
};

export default function ChatHistoryDrawer({
  inline = false,
  projectArchive,
  open,
  title,
  currentChatId,
  conversationView = "user",
  onConversationViewChange = () => undefined,
  collaborationFilters = { sourceChatId: "", executor: "", status: "" },
  onCollaborationFiltersChange = () => undefined,
  chats,
  loading,
  loadingMore = false,
  hasMore = false,
  searchValue,
  searchLabel,
  searchPlaceholder,
  sort,
  tagFilter,
  sortDisabled = false,
  emptyLabel,
  createLabel,
  loadMoreLabel,
  loadingMoreLabel,
  createDisabled = false,
  searchContent,
  isActionDisabled,
  renameLabel,
  pinLabel,
  deleteLabel,
  onClose,
  onCreate,
  onSearchChange,
  onSortChange,
  onTagFilterChange,
  onSelect,
  onRename,
  onTogglePin,
  onCycleTag,
  onDelete,
  onLoadMore,
  onScroll,
  formatDate,
  titleForChat,
}: ChatHistoryDrawerProps) {
  const { t } = useTranslation();
  const pullPastBottom = usePullPastBottom({
    enabled: hasMore && !loading && !loadingMore && Boolean(onLoadMore),
    onLoadMore: () => onLoadMore?.(),
  });
  const pinnedChats = chats.filter((chat) => chat.pinned);
  const unpinnedChats = chats.filter((chat) => !chat.pinned);
  const tagOptions: Array<{ value: ChatTag | null; label: string }> = [
    { value: null, label: t("chat.filterAll") },
    { value: "none", label: t("chat.tag.none") },
    { value: "red", label: t("chat.tag.red") },
    { value: "yellow", label: t("chat.tag.yellow") },
    { value: "green", label: t("chat.tag.green") },
  ];
  const sortFields: Array<{ value: ChatSortField; label: string }> = [
    { value: "title", label: t("chat.sortFieldTitle") },
    { value: "created_at", label: t("chat.sortFieldCreated") },
    { value: "updated_at", label: t("chat.sortFieldActivity") },
  ];
  const sortOrders: Array<{ value: SortOrder; label: string }> = [
    { value: "asc", label: t("chat.sortAscending") },
    { value: "desc", label: t("chat.sortDescending") },
  ];
  const collaborationFiltersActive = Boolean(
    collaborationFilters.sourceChatId.trim() ||
      collaborationFilters.executor.trim() ||
      collaborationFilters.status,
  );

  const content = (
    <>
      <div className="rua-owned-chat-drawer-controls">
        <Tabs
          className="rua-owned-chat-view-tabs"
          activeKey={projectArchive?.archived ? "archive" : conversationView}
          onChange={(view) => {
            if (view === "archive") {
              if (!projectArchive?.archived) projectArchive?.onToggle();
            } else {
              if (projectArchive?.archived) projectArchive.onToggle();
              onConversationViewChange(view as ConversationListView);
            }
          }}
          items={[
            {
              key: "user",
              label: t("chat.myConversations"),
            },
            {
              key: "collaboration",
              label: t("chat.collaborationConversations"),
            },
            ...(projectArchive
              ? [{ key: "archive", label: t("projects.chat.archiveList") }]
              : []),
          ]}
        />
        {onCreate &&
        !projectArchive?.archived &&
        conversationView === "user" ? (
          <Btn
            className="rua-owned-chat-create-button"
            disabled={createDisabled}
            onClick={onCreate}
            size="md"
          >
            {createLabel}
          </Btn>
        ) : null}
      </div>
      <div className="rua-owned-chat-drawer-tools">
        <div className="rua-owned-chat-search-input-wrap">
          <Search
            size={14}
            className="rua-owned-chat-search-input-icon"
            aria-hidden="true"
          />
          <TextInput
            value={searchValue}
            aria-label={searchLabel}
            placeholder={searchPlaceholder}
            onChange={(event) => onSearchChange(event.target.value)}
            className="rua-owned-chat-search-input"
          />
        </div>
        <Dropdown
          placement="bottom-end"
          overlayClassName="rua-owned-chat-filter-dropdown"
          dropdownRender={() => (
            <div className="rua-owned-chat-filter-panel">
              {!projectArchive?.archived &&
                (conversationView === "collaboration" ? (
                  <div className="rua-owned-chat-collaboration-filters">
                    <TextInput
                      value={collaborationFilters.sourceChatId}
                      aria-label={t("chat.filterSource")}
                      placeholder={t("chat.filterSource")}
                      onChange={(event) =>
                        onCollaborationFiltersChange({
                          ...collaborationFilters,
                          sourceChatId: event.target.value,
                        })
                      }
                    />
                    <TextInput
                      value={collaborationFilters.executor}
                      aria-label={t("chat.filterExecutor")}
                      placeholder={t("chat.filterExecutor")}
                      onChange={(event) =>
                        onCollaborationFiltersChange({
                          ...collaborationFilters,
                          executor: event.target.value,
                        })
                      }
                    />
                    <Select
                      value={collaborationFilters.status}
                      aria-label={t("chat.filterStatus")}
                      options={[
                        { value: "", label: t("chat.statusAll") },
                        { value: "idle", label: t("chat.statusIdle") },
                        { value: "running", label: t("chat.statusRunning") },
                      ]}
                      onChange={(status) =>
                        onCollaborationFiltersChange({
                          ...collaborationFilters,
                          status: status ?? "",
                        })
                      }
                    />
                  </div>
                ) : (
                  <div className="rua-owned-chat-filter-section">
                    <div className="rua-owned-chat-filter-label">
                      {t("chat.filterTag")}
                    </div>
                    <div className="rua-owned-chat-tag-filter-options">
                      {tagOptions.map((option) => (
                        <button
                          key={option.value ?? "all"}
                          type="button"
                          className="rua-owned-chat-filter-option"
                          data-selected={tagFilter === option.value}
                          aria-pressed={tagFilter === option.value}
                          onClick={() => onTagFilterChange(option.value)}
                        >
                          {option.value !== null ? (
                            <span
                              className={`rua-owned-chat-tag-dot rua-owned-chat-tag-dot--${option.value}`}
                              aria-hidden="true"
                            />
                          ) : null}
                          {option.label}
                        </button>
                      ))}
                    </div>
                  </div>
                ))}
              {!projectArchive?.archived && (
                <div className="rua-owned-chat-filter-divider" />
              )}
              <div className="rua-owned-chat-filter-section">
                <div className="rua-owned-chat-filter-label">
                  {t("chat.sortField")}
                </div>
                <div className="rua-owned-chat-filter-options">
                  {sortFields.map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      className="rua-owned-chat-filter-option"
                      data-selected={sort.sort_by === option.value}
                      aria-pressed={sort.sort_by === option.value}
                      onClick={() =>
                        onSortChange({ ...sort, sort_by: option.value })
                      }
                    >
                      {option.label}
                    </button>
                  ))}
                </div>
              </div>
              <div className="rua-owned-chat-filter-section">
                <div className="rua-owned-chat-filter-label">
                  {t("chat.sortDirection")}
                </div>
                <div className="rua-owned-chat-filter-options">
                  {sortOrders.map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      className="rua-owned-chat-filter-option"
                      data-selected={sort.sort_order === option.value}
                      aria-pressed={sort.sort_order === option.value}
                      onClick={() =>
                        onSortChange({ ...sort, sort_order: option.value })
                      }
                    >
                      {option.label}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          )}
        >
          <button
            type="button"
            className="rua-owned-chat-sort-button"
            disabled={sortDisabled}
            aria-label={t("chat.filterAndSort")}
            title={
              sortDisabled ? t("chat.sortDisabledDuringSearch") : undefined
            }
          >
            <ListFilter size={16} aria-hidden="true" />
            {conversationView === "user" && tagFilter !== null ? (
              <span
                className={`rua-owned-chat-filter-indicator rua-owned-chat-tag-dot--${tagFilter}`}
                aria-hidden="true"
              />
            ) : null}
            {conversationView === "collaboration" &&
            collaborationFiltersActive ? (
              <span
                className="rua-owned-chat-filter-indicator"
                aria-hidden="true"
              />
            ) : null}
          </button>
        </Dropdown>
        {projectArchive?.archived && projectArchive.onClear ? (
          <Tooltip title={t("projects.chat.clearArchive")}>
            <Btn
              variant="ghost"
              icon={<Trash2 size={16} />}
              aria-label={t("projects.chat.clearArchive")}
              disabled={projectArchive.disabled}
              onClick={() => void projectArchive.onClear?.()}
            />
          </Tooltip>
        ) : null}
      </div>
      <div
        className="rua-owned-chat-list-wrap"
        data-rua-chat-session-list
        onScroll={(event) => {
          pullPastBottom.onScroll(event);
          onScroll?.(event);
        }}
        onWheel={pullPastBottom.onWheel}
        onTouchStart={pullPastBottom.onTouchStart}
        onTouchMove={pullPastBottom.onTouchMove}
        onTouchEnd={pullPastBottom.onTouchEnd}
        onTouchCancel={pullPastBottom.onTouchEnd}
      >
        {searchContent ? (
          searchContent
        ) : loading && chats.length === 0 ? (
          <div className="rua-owned-chat-search-spinner">
            <Spinner size="sm" />
          </div>
        ) : chats.length === 0 ? (
          <div className="rua-owned-chat-empty-list">{emptyLabel}</div>
        ) : (
          <>
            {conversationView === "collaboration" &&
            !projectArchive?.archived ? (
              chats.map((chat) => (
                <CollaborationChatRow
                  projectArchive={projectArchive}
                  key={chat.id}
                  chat={chat}
                  active={chat.id === currentChatId}
                  actionsDisabled={isActionDisabled?.(chat) ?? false}
                  deleteLabel={deleteLabel?.(chat)}
                  onSelect={onSelect}
                  onDelete={onDelete}
                  formatDate={formatDate}
                  titleForChat={titleForChat}
                />
              ))
            ) : (
              <>
                {pinnedChats.length > 0 ? (
                  <div className="rua-owned-chat-pinned-section">
                    {pinnedChats.map((chat) => (
                      <ChatHistoryListRow
                        projectArchive={projectArchive}
                        key={chat.id}
                        chat={chat}
                        active={chat.id === currentChatId}
                        actionsDisabled={isActionDisabled?.(chat) ?? false}
                        renameLabel={renameLabel?.(chat)}
                        pinLabel={pinLabel?.(chat)}
                        deleteLabel={deleteLabel?.(chat)}
                        onSelect={onSelect}
                        onRename={onRename}
                        onTogglePin={onTogglePin}
                        onCycleTag={onCycleTag}
                        onDelete={onDelete}
                        formatDate={formatDate}
                        titleForChat={titleForChat}
                      />
                    ))}
                    <div className="rua-owned-chat-pinned-divider" />
                  </div>
                ) : null}
                {unpinnedChats.map((chat) => (
                  <ChatHistoryListRow
                    projectArchive={projectArchive}
                    key={chat.id}
                    chat={chat}
                    active={chat.id === currentChatId}
                    actionsDisabled={isActionDisabled?.(chat) ?? false}
                    renameLabel={renameLabel?.(chat)}
                    pinLabel={pinLabel?.(chat)}
                    deleteLabel={deleteLabel?.(chat)}
                    onSelect={onSelect}
                    onRename={onRename}
                    onTogglePin={onTogglePin}
                    onCycleTag={onCycleTag}
                    onDelete={onDelete}
                    formatDate={formatDate}
                    titleForChat={titleForChat}
                  />
                ))}
              </>
            )}
            {hasMore && onLoadMore ? (
              <div className="rua-owned-chat-load-more">
                <button
                  type="button"
                  className="rua-owned-chat-load-more-button"
                  onClick={onLoadMore}
                  disabled={loadingMore}
                >
                  {loadingMore ? <Spinner size="sm" /> : null}
                  <span>{loadingMore ? loadingMoreLabel : loadMoreLabel}</span>
                </button>
              </div>
            ) : null}
          </>
        )}
      </div>
    </>
  );
  if (inline)
    return open ? (
      <aside className="rua-owned-chat-side-panel" aria-label={title}>
        {content}
      </aside>
    ) : null;
  return (
    <Drawer
      open={open}
      title={title}
      onClose={onClose}
      width={420}
      className="rua-owned-session-drawer"
    >
      {content}
    </Drawer>
  );
}

type ChatHistoryListRowProps = {
  projectArchive?: ChatHistoryDrawerProps["projectArchive"];
  chat: ChatSpec;
  active: boolean;
  actionsDisabled: boolean;
  renameLabel?: string;
  pinLabel?: string;
  deleteLabel?: string;
  onSelect: (chat: ChatSpec) => void;
  onRename: (chat: ChatSpec) => void | Promise<void>;
  onTogglePin: (chat: ChatSpec) => void | Promise<void>;
  onCycleTag: (chat: ChatSpec) => void | Promise<void>;
  onDelete: (chat: ChatSpec) => void | Promise<void>;
  formatDate: (raw: string | null | undefined) => string;
  titleForChat: (chat: ChatSpec) => string;
};

type CollaborationChatRowProps = Pick<
  ChatHistoryListRowProps,
  | "chat"
  | "active"
  | "actionsDisabled"
  | "deleteLabel"
  | "onSelect"
  | "onDelete"
  | "formatDate"
  | "titleForChat"
  | "projectArchive"
>;

function CollaborationChatRow({
  projectArchive,
  chat,
  active,
  actionsDisabled,
  deleteLabel,
  onSelect,
  onDelete,
  formatDate,
  titleForChat,
}: CollaborationChatRowProps) {
  const { t } = useTranslation();
  const relation = chat.collaboration;
  const executor =
    relation?.executor.kind === "acp"
      ? t("chat.collaborationExecutorAcp", {
          runner: relation.executor.runner,
        })
      : t("chat.collaborationExecutorAgent", {
          agent: relation?.executor_agent_id || chat.user_id,
        });
  const status =
    chat.status === "running" ? t("chat.statusRunning") : t("chat.statusIdle");
  const sourceAvailable = relation?.source_available !== false;

  return (
    <div
      className={`rua-owned-chat-list-item rua-owned-collaboration-row${
        active ? " rua-owned-chat-list-item--active" : ""
      }`}
    >
      <button
        type="button"
        className="rua-owned-chat-list-item-main"
        onClick={() => onSelect(chat)}
        aria-label={titleForChat(chat)}
      >
        <span className="rua-owned-chat-list-title">
          {titleForChat(chat)}
        </span>
        <span className="rua-owned-collaboration-meta">
          <span>{executor}</span>
          <span
            className={`rua-owned-collaboration-status is-${
              chat.status ?? "idle"
            }`}
          >
            {status}
          </span>
          <span className={sourceAvailable ? "is-available" : "is-unavailable"}>
            {sourceAvailable
              ? t("chat.collaborationSourceAvailable")
              : t("chat.collaborationSourceUnavailable")}
          </span>
          <span>{formatDate(chat.updated_at ?? chat.created_at)}</span>
        </span>
      </button>
      <div className="rua-owned-chat-list-actions">
        {projectArchive ? (
          <Btn
            variant="ghost"
            size="sm"
            icon={<Archive />}
            aria-label={t("projects.chat.archive")}
            disabled={actionsDisabled}
            onClick={() => void projectArchive.onChange(chat)}
          />
        ) : null}
        {deleteLabel ? (
          <Tooltip title={deleteLabel}>
            <Btn
              variant="ghost"
              size="sm"
              className="rua-owned-chat-row-action"
              aria-label={deleteLabel}
              icon={<Trash2 size={16} aria-hidden="true" />}
              disabled={actionsDisabled}
              onClick={(event) => {
                event.stopPropagation();
                void onDelete(chat);
              }}
            />
          </Tooltip>
        ) : null}
      </div>
    </div>
  );
}

function ChatHistoryListRow({
  projectArchive,
  chat,
  active,
  actionsDisabled,
  renameLabel,
  pinLabel,
  deleteLabel,
  onSelect,
  onRename,
  onTogglePin,
  onCycleTag,
  onDelete,
  formatDate,
  titleForChat,
}: ChatHistoryListRowProps) {
  const { t } = useTranslation();
  const channelKey = chat.channel?.trim() || "console";
  const channelLabel = getChannelLabel(channelKey, t);
  const channelIcon = getChannelIconUrl(channelKey);
  const tag = chat.tag ?? "none";
  const tagLabel = t("chat.tagCycle", { current: t(`chat.tag.${tag}`) });
  const collaboration = chat.collaboration;

  return (
    <div
      className={[
        "rua-owned-chat-list-item",
        "rua-owned-chat-list-item--standard",
        active && "rua-owned-chat-list-item--active",
      ]
        .filter(Boolean)
        .join(" ")}
      data-rua-chat-session-id={chat.id}
      role="link"
      tabIndex={0}
      aria-current={active ? "page" : undefined}
      onClick={() => onSelect(chat)}
      onKeyDown={(event) => {
        if (event.target === event.currentTarget && event.key === "Enter") {
          onSelect(chat);
        }
      }}
    >
      <div className="rua-owned-chat-list-item-main">
        <span className="rua-owned-chat-list-title">
          {titleForChat(chat)}
        </span>
        {collaboration ? (
          <span className="rua-owned-chat-collaboration-meta">
            <span>
              {collaboration.executor.kind === "acp"
                ? `ACP · ${collaboration.executor.runner}`
                : `Agent · ${collaboration.executor_agent_id}`}
            </span>
            <span>{chat.status || "idle"}</span>
            <span>
              {collaboration.source_available === false
                ? t("chat.sourceUnavailable", {
                    defaultValue: "Source unavailable",
                  })
                : t("chat.sourceAvailable", {
                    defaultValue: "Source available",
                  })}
            </span>
          </span>
        ) : null}
      </div>
      <span className="rua-owned-chat-list-meta">
        {formatDate(chat.updated_at || chat.created_at)}
      </span>
      <div className="rua-owned-chat-list-actions">
        {!projectArchive?.archived ? (
          <Btn
            variant="ghost"
            size="sm"
            icon={chat.pinned ? <Pin /> : <PinOff />}
            aria-label={
              pinLabel ||
              t(chat.pinned ? "chat.unpinTooltip" : "chat.pinTooltip")
            }
            disabled={actionsDisabled}
            onClick={(event) => {
              event.stopPropagation();
              void onTogglePin(chat);
            }}
          />
        ) : null}
        {projectArchive ? (
          <>
            <Btn
              variant="ghost"
              size="sm"
              icon={projectArchive.archived ? <ArchiveRestore /> : <Archive />}
              aria-label={t(
                projectArchive.archived
                  ? "projects.chat.restore"
                  : "projects.chat.archive",
              )}
              disabled={actionsDisabled}
              onClick={(event) => {
                event.stopPropagation();
                void projectArchive.onChange(chat);
              }}
            />
          </>
        ) : null}
        <span
          className="rua-owned-chat-channel-tag"
          title={channelLabel}
          aria-label={channelLabel}
        >
          <img
            className="rua-owned-chat-channel-icon"
            src={channelIcon}
            alt=""
            aria-hidden="true"
          />
          {!projectArchive ? (
            <span className="rua-owned-chat-channel-text">
              {channelLabel}
            </span>
          ) : null}
        </span>
        <button
          type="button"
          className="rua-owned-chat-tag-action"
          aria-label={tagLabel}
          aria-pressed={tag !== "none"}
          disabled={actionsDisabled}
          onClick={(event) => {
            event.stopPropagation();
            void onCycleTag(chat);
          }}
        >
          <span
            className={`rua-owned-chat-tag-dot rua-owned-chat-tag-dot--${tag}`}
            aria-hidden="true"
          />
        </button>
        <Btn
          variant="ghost"
          size="sm"
          icon={<Edit3 />}
          aria-label={renameLabel || t("common.rename")}
          className="rua-owned-chat-row-action"
          disabled={actionsDisabled}
          onClick={(event) => {
            event.stopPropagation();
            void onRename(chat);
          }}
        />
        {!chat.pinned ? (
          <Btn
            variant="ghost"
            size="sm"
            icon={<Trash2 />}
            aria-label={deleteLabel || t("common.delete")}
            className="rua-owned-chat-row-action rua-owned-chat-delete-action"
            disabled={actionsDisabled}
            onClick={(event) => {
              event.stopPropagation();
              void onDelete(chat);
            }}
          />
        ) : null}
      </div>
    </div>
  );
}
