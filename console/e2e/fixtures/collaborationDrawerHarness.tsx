import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import i18n from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import ChatHistoryDrawer from "../../src/pages/Chat/ChatHistoryDrawer";
import type { ConversationListView } from "../../src/api/types/chat";
import zh from "../../src/locales/zh.json";
import en from "../../src/locales/en.json";
import "../../src/styles/theme.css";
import "../../src/styles/layout.css";
import projectStyles from "../../src/pages/Projects/ProjectChat/index.module.less";
import ChatListResizeHandle from "../../src/pages/Chat/ChatListResizeHandle";

const params = new URLSearchParams(location.search);
if (params.has("mobile")) document.documentElement.classList.add("mobile-mode");
const instance = i18n.createInstance();
await instance.use(initReactI18next).init({
  lng: params.get("lang") ?? "zh",
  fallbackLng: "en",
  resources: { zh: { translation: zh }, en: { translation: en } },
});
const noop = () => {};

function Harness() {
  const projectMode = params.has("project");
  const agentMode = params.has("agent");
  const mobile = params.has("mobile");
  const [archived, setArchived] = useState(false);
  const [opened, setOpened] = useState(true);
  const [chatArchived, setChatArchived] = useState(false);
  const [view, setView] = useState<ConversationListView>(
    projectMode || agentMode ? "user" : "collaboration",
  );
  const [search, setSearch] = useState("");
  const [filters, setFilters] = useState({
    sourceChatId: "",
    executor: "",
    status: "",
  });
  const drawer = (
    <ChatHistoryDrawer
      inline={(projectMode || agentMode) && !mobile}
      projectArchive={
        projectMode
          ? {
              archived,
              disabled: false,
              onToggle: () => setArchived(!archived),
              onChange: () => setChatArchived(!chatArchived),
              onClear: () => {},
            }
          : undefined
      }
      open
      title={instance.t("projects.chat.title")}
      currentChatId="review"
      conversationView={view}
      onConversationViewChange={setView}
      collaborationFilters={filters}
      onCollaborationFiltersChange={setFilters}
      chats={
        projectMode && archived !== chatArchived
          ? []
          : [
              {
                id: "review",
                channel: "console",
                user_id: "default",
                name: "检查项目变更与回归风险",
                pinned: !chatArchived,
                archived: chatArchived,
                tag: "red",
                status: "running",
                created_at: null,
                updated_at: null,
                collaboration:
                  projectMode || agentMode
                    ? undefined
                    : {
                        source: { chat_id: "source" },
                        executor: { kind: "acp", runner: "codex" },
                        executor_agent_id: "default",
                        source_available: true,
                        configuration: {
                          desired_source_epoch: "1",
                          applied_source_epoch: "1",
                          status: "ready",
                        },
                      },
              },
            ]
      }
      loading={false}
      searchValue={search}
      searchLabel={instance.t("projects.chat.searchPlaceholder")}
      searchPlaceholder={instance.t("projects.chat.searchPlaceholder")}
      sort={{ sort_by: "updated_at", sort_order: "desc" }}
      tagFilter="red"
      emptyLabel="暂无协作对话"
      createLabel="新建项目对话"
      loadMoreLabel="加载更多"
      loadingMoreLabel="加载中"
      onClose={noop}
      onCreate={projectMode ? undefined : noop}
      onSearchChange={setSearch}
      onSortChange={noop}
      onTagFilterChange={noop}
      onSelect={noop}
      onRename={noop}
      onTogglePin={noop}
      onCycleTag={noop}
      onDelete={noop}
      deleteLabel={() => "删除协作对话"}
      formatDate={() => "9月23日 13:53"}
      titleForChat={(chat) => chat.name ?? chat.id}
    />
  );
  if (agentMode)
    return (
      <main
        className="rua-owned-chat-stage"
        data-list-open={!mobile && opened ? "true" : undefined}
        style={{ height: "100vh" }}
      >
        <article data-testid="conversation">
          普通对话
          <button onClick={() => setOpened(!opened)}>Toggle list</button>
        </article>
        {!mobile ? <ChatListResizeHandle hidden={!opened} /> : null}
        {opened ? drawer : null}
      </main>
    );
  return projectMode ? (
    <main
      className={`${projectStyles.stage} ${
        !mobile && opened ? projectStyles.stageChats : ""
      }`}
      style={{ height: "100vh" }}
    >
      <article data-testid="conversation">
        聊天正文不会被桌面列表遮挡
        <button onClick={() => setOpened(!opened)}>Toggle list</button>
      </article>
      {!mobile ? <ChatListResizeHandle hidden={!opened} /> : null}
      {mobile ? (
        drawer
      ) : (
        <div
          className={projectStyles.chatPanel}
          data-open={opened}
          aria-hidden={!opened}
          {...(!opened ? { inert: "" } : {})}
        >
          {drawer}
        </div>
      )}
    </main>
  ) : (
    drawer
  );
}

createRoot(document.getElementById("root")!).render(
  <I18nextProvider i18n={instance}>
    <Harness />
  </I18nextProvider>,
);
