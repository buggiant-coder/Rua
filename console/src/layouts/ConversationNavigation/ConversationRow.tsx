import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Archive,
  LoaderCircle,
  Check,
  MoreHorizontal,
  Pin,
  PinOff,
  Pencil,
  Trash2,
} from "lucide-react";
import { chatApi } from "@/api/modules/chat";
import { projectChatsApi } from "@/api/modules/projectChats";
import type { ChatSpec, ChatTag, ChatUpdateRequest } from "@/api/types";
import {
  conversationPath,
  type ConversationAddress,
} from "@/pages/Chat/conversationAddress";
import {
  clearDraftChatId,
  forgetChatId,
  resolveOrCreateDraftChatId,
} from "@/pages/Chat/chatNavigation";
import {
  createProjectChat,
  discardProjectChatLocalState,
} from "@/pages/Projects/projectChatNavigation";
import * as drafts from "@/pages/Chat/drafts";
import { Dropdown, Modal, TextInput, toast } from "@/ui";
import {
  invalidateConversationNavigation,
  beginConversationMetadataUpdate,
  publishConversationDeleted,
} from "../conversationNavigationEvents";
import { scopeAddress, type NavigationScope } from "./useConversationGroups";
import styles from "./index.module.less";
import {
  clearConversationActivity,
  useConversationActivityStatus,
} from "../conversationActivity";

const tags: ChatTag[] = ["none", "red", "yellow", "green"];

type Props = {
  scope: NavigationScope;
  chat: ChatSpec;
  current: ConversationAddress | null;
  getCurrentAddress: () => ConversationAddress | null;
  onManage: () => void;
  onNavigate: (path: string) => void;
  onRemoved: (id: string) => void;
};

export default function ConversationRow({
  scope,
  chat,
  current,
  getCurrentAddress,
  onManage,
  onNavigate,
  onRemoved,
}: Props) {
  const { t } = useTranslation();
  const [pending, setPending] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const pendingRef = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const title = chat.name?.trim() || t("navigation.untitled");
  const address = scopeAddress(scope, chat.id);
  const path = conversationPath(address);
  const activityStatus = useConversationActivityStatus(address);
  const active = current !== null && conversationPath(current) === path;
  const tag = chat.tag ?? "none";
  const markerLabel = [
    chat.pinned ? t("projects.chat.pinned") : "",
    tag !== "none" ? t(`chat.tag.${tag}`) : "",
  ]
    .filter(Boolean)
    .join(" · ");
  const begin = () => {
    pendingRef.current = true;
    setPending(true);
  };
  const finish = () => {
    pendingRef.current = false;
    if (mounted.current) setPending(false);
  };

  const update = async (updates: ChatUpdateRequest) => {
    if (pendingRef.current) return;
    begin();
    const mutation = beginConversationMetadataUpdate(address, chat, updates);
    try {
      const updated =
        scope.kind === "project"
          ? await projectChatsApi.updateChat(scope.id, chat.id, updates)
          : await chatApi.updateChat(chat.id, updates, scope.id);
      mutation.commit(updated);
    } catch (error) {
      mutation.rollback();
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      finish();
    }
  };

  const rename = async () => {
    let name = chat.name || "";
    const confirmed = await Modal.confirm({
      title: t("common.rename"),
      content: (
        <TextInput
          aria-label={t("chat.renameChatInput")}
          autoFocus
          defaultValue={name}
          onChange={(event) => {
            name = event.target.value;
          }}
        />
      ),
      okText: t("common.save"),
      cancelText: t("common.cancel"),
    });
    if (confirmed && name.trim() && name.trim() !== chat.name?.trim())
      await update({ name: name.trim() });
  };

  const remove = async () => {
    if (chat.pinned || pendingRef.current) return;
    const busy = activityStatus.running || chat.status === "running";
    const confirmed = await Modal.confirm({
      title: busy ? t("navigation.deleteRunningTitle") : t("common.delete"),
      content: busy
        ? t("navigation.deleteRunningConfirm", { title })
        : t("chat.deleteConfirm", { name: title }),
      okText: busy ? t("projects.chat.stopAndDelete") : t("common.delete"),
      cancelText: t("common.cancel"),
      danger: true,
    });
    if (!confirmed || pendingRef.current) return;
    begin();
    try {
      if (scope.kind === "project") {
        try {
          await projectChatsApi.deleteChat(scope.id, chat.id, busy);
        } catch (error) {
          const message =
            error instanceof Error ? error.message.toLowerCase() : "";
          if (busy || !/busy|running|active or pending work/.test(message))
            throw error;
          if (
            !(await Modal.confirm({
              title: t("projects.chat.becameBusy"),
              content: t("projects.chat.busyDeleteConfirm", { title }),
              okText: t("projects.chat.stopAndDelete"),
              cancelText: t("common.cancel"),
              danger: true,
            }))
          )
            return;
          await projectChatsApi.deleteChat(scope.id, chat.id, true);
        }
        discardProjectChatLocalState(scope.id, chat.id);
      } else {
        await chatApi.deleteChat(chat.id, scope.id);
        forgetChatId(scope.id, chat.id);
        clearDraftChatId(scope.id, chat.id);
        drafts.clear(`${scope.id}:${chat.id}`);
      }
      clearConversationActivity(address);
      onRemoved(chat.id);
      invalidateConversationNavigation(address);
      const stillCurrent = () => {
        const latest = getCurrentAddress();
        return latest !== null && conversationPath(latest) === path;
      };
      if (!stillCurrent()) return;
      let remaining: ChatSpec[] = [];
      try {
        const params = {
          offset: 0,
          limit: 20,
          archived: false,
          view: "user" as const,
        };
        const result =
          scope.kind === "project"
            ? await projectChatsApi.listChats(scope.id, params)
            : await chatApi.listChats(params, scope.id);
        remaining = result.data.filter(
          (item) => item.id !== chat.id && !item.archived,
        );
      } catch {
        /* A deleted conversation must remain deleted even if fallback lookup fails. */
      }
      if (!stillCurrent()) return;
      if (publishConversationDeleted(address, remaining)) return;
      const latest = remaining.reduce<ChatSpec | null>(
        (best, item) =>
          !best || activity(item) > activity(best) ? item : best,
        null,
      );
      const nextId =
        latest?.id ??
        (scope.kind === "project"
          ? createProjectChat(scope.id)
          : resolveOrCreateDraftChatId(scope.id));
      onNavigate(conversationPath(scopeAddress(scope, nextId)));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      finish();
    }
  };

  return (
    <div
      className={`${styles.chatRow} ${active ? styles.active : ""}`}
      data-chat-id={chat.id}
    >
      <button
        type="button"
        className={styles.chatLink}
        aria-current={active ? "page" : undefined}
        title={title}
        onClick={() => onNavigate(path)}
      >
        <span
          className={styles.chatMarker}
          data-chat-marker
          data-tag={tag}
          data-pinned={Boolean(chat.pinned)}
          role={markerLabel ? "img" : undefined}
          aria-label={markerLabel || undefined}
          aria-hidden={!markerLabel}
        >
          {chat.pinned ? (
            <Pin
              size={16}
              fill={tag !== "none" ? "currentColor" : "none"}
              strokeWidth={2.2}
            />
          ) : tag !== "none" ? (
            <span className={styles.colorDot} data-color-dot />
          ) : null}
        </span>
        <span className={styles.chatTitle}>{title}</span>
        {chat.archived && (
          <span className={styles.archivedBadge}>{t("projects.archived")}</span>
        )}
      </button>
      <Dropdown
        open={menuOpen}
        onOpenChange={setMenuOpen}
        overlayClassName={styles.rowMenu}
        dropdownRender={(menu) =>
          chat.archived ? (
            menu
          ) : (
            <>
              <div className={styles.tagPicker}>
                <span>{t("chat.filterTag")}</span>
                <div role="group" aria-label={t("chat.filterTag")}>
                  {tags.map((value) => (
                    <button
                      key={value}
                      type="button"
                      className={styles.tagChoice}
                      data-tag={value}
                      aria-label={t(`chat.tag.${value}`)}
                      title={t(`chat.tag.${value}`)}
                      aria-pressed={tag === value}
                      disabled={pending}
                      onClick={() => {
                        setMenuOpen(false);
                        void update({ tag: value });
                      }}
                    >
                      {tag === value ? <Check size={12} /> : null}
                    </button>
                  ))}
                </div>
              </div>
              {menu}
            </>
          )
        }
        items={
          chat.archived
            ? [
                {
                  key: "manage",
                  label: t("navigation.manageConversations"),
                  onClick: onManage,
                },
              ]
            : [
                {
                  key: "pin",
                  label: t(
                    chat.pinned ? "projects.chat.unpin" : "projects.chat.pin",
                  ),
                  icon: chat.pinned ? <PinOff size={15} /> : <Pin size={15} />,
                  disabled: pending,
                  onClick: () =>
                    void update({
                      pinned: !chat.pinned,
                      pin_order: chat.pinned ? 0 : Date.now(),
                    }),
                },
                {
                  key: "rename",
                  label: t("common.rename"),
                  icon: <Pencil size={15} />,
                  disabled: pending,
                  onClick: () => void rename(),
                },
                {
                  key: "archive",
                  label: t("projects.chat.archive"),
                  icon: <Archive size={15} />,
                  disabled: pending,
                  onClick: () => void update({ archived: true }),
                },
                {
                  key: "manage",
                  label: t("navigation.manageConversations"),
                  onClick: onManage,
                },
                { type: "divider" },
                {
                  key: "delete",
                  label: chat.pinned
                    ? t("navigation.unpinBeforeDelete")
                    : t("common.delete"),
                  icon: <Trash2 size={15} />,
                  danger: true,
                  disabled: pending || Boolean(chat.pinned),
                  onClick: () => void remove(),
                },
              ]
        }
      >
        <button
          type="button"
          className={`${styles.iconButton} ${styles.chatActions}`}
          aria-label={t("navigation.conversationActions", { name: title })}
        >
          <MoreHorizontal size={15} />
        </button>
      </Dropdown>
      {(activityStatus.running || activityStatus.unread) && (
        <span
          className={styles.activityStatus}
          role="img"
          aria-label={t(
            activityStatus.running
              ? "navigation.generating"
              : "navigation.unreadReply",
          )}
          title={t(
            activityStatus.running
              ? "navigation.generating"
              : "navigation.unreadReply",
          )}
          data-conversation-status={
            activityStatus.running ? "running" : "unread"
          }
        >
          {activityStatus.running ? (
            <LoaderCircle
              size={15}
              className={styles.activitySpinner}
              aria-hidden="true"
            />
          ) : (
            <span className={styles.unreadDot} aria-hidden="true" />
          )}
        </span>
      )}
    </div>
  );
}

function activity(chat: ChatSpec) {
  const value = Date.parse(chat.updated_at || chat.created_at || "");
  return Number.isNaN(value) ? 0 : value;
}
