import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { chatApi } from "@/api/modules/chat";
import { projectChatsApi } from "@/api/modules/projectChats";
import type { ChatSpec } from "@/api/types";
import {
  conversationPath,
  type ConversationAddress,
} from "@/pages/Chat/conversationAddress";
import {
  subscribeConversationMetadata,
  subscribeConversationNavigation,
} from "../conversationNavigationEvents";
import ConversationRow from "./ConversationRow";
import type { NavigationScope } from "./useConversationGroups";
import styles from "./index.module.less";

type Props = {
  scope: NavigationScope;
  current: ConversationAddress;
  empty: boolean;
  getCurrentAddress: () => ConversationAddress | null;
  onManage: () => void;
  onNavigate: (path: string) => void;
};

/** Display the verified open archive entry without changing the active list. */
export default function CurrentArchivedConversation({
  scope,
  current,
  empty,
  ...actions
}: Props) {
  const { t } = useTranslation();
  const [chat, setChat] = useState<ChatSpec | null>(null);
  const { kind, id } = scope;
  const { chatId } = current;
  const path = conversationPath(current);
  useEffect(() => {
    let cancelled = false;
    let request = 0;
    let controller: AbortController | undefined;
    const accept = (value: ChatSpec) => {
      setChat(value.id === chatId && value.archived === true ? value : null);
    };
    const load = () => {
      controller?.abort();
      controller = new AbortController();
      const revision = ++request;
      const result =
        kind === "project"
          ? projectChatsApi.getMetadata(id, chatId, {
              signal: controller.signal,
            })
          : chatApi.getMetadata(chatId, id, { signal: controller.signal });
      void result
        .then((value) => {
          if (!cancelled && revision === request) accept(value);
        })
        .catch(() => {
          if (!cancelled && revision === request) setChat(null);
        });
    };
    load();
    const unsubscribeMetadata = subscribeConversationMetadata(
      (address, value) => {
        if (conversationPath(address) !== path) return;
        request += 1;
        controller?.abort();
        accept(value);
      },
    );
    const unsubscribeNavigation = subscribeConversationNavigation((address) => {
      if (!address || conversationPath(address) === path) load();
    });
    return () => {
      cancelled = true;
      controller?.abort();
      unsubscribeMetadata();
      unsubscribeNavigation();
    };
  }, [chatId, id, kind, path]);
  return chat ? (
    <ConversationRow
      scope={scope}
      chat={chat}
      current={current}
      {...actions}
      onRemoved={() => setChat(null)}
    />
  ) : empty ? (
    <p className={styles.status}>{t("navigation.noConversations")}</p>
  ) : null;
}
