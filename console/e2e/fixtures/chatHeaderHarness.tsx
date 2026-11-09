import { useState } from "react";
import { createRoot } from "react-dom/client";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import MainLayout from "../../src/layouts/MainLayout";
import {
  ListTodo,
  MessageSquarePlus,
  MessagesSquare,
  PanelRight,
} from "lucide-react";
import i18n from "../../src/i18n";
import ChatHeader from "../../src/pages/Chat/ChatHeader";
import AgentBadge from "../../src/components/AgentBadge";
import ProjectAgentSelector from "../../src/pages/Projects/ProjectChat/ProjectAgentSelector";
import ProjectSelector from "../../src/components/ProjectSelector";
import { useAgentStore } from "../../src/stores/agentStore";
import "../../src/styles/theme.css";
import "../../src/styles/layout.css";
import "../../src/pages/Chat/owned-chat-page.css";

const params = new URLSearchParams(location.search);
const mode = params.get("mode");
localStorage.setItem("rua_sider_collapsed", "true");
if (mode === "mobile") document.documentElement.classList.add("mobile-mode");
if (params.has("dark")) document.documentElement.classList.add("dark-mode");
await i18n.changeLanguage(params.get("lang") ?? "zh");
const agent = {
  id: "header-fixture",
  name: params.has("long") ? "A very long Agent display name" : "小Rua",
  description: "",
  enabled: true,
  workspace_dir: "",
};
useAgentStore.setState({
  agents: [agent],
  selectedAgent: agent.id,
  catalogLoaded: true,
});
const noop = () => {};

export function Harness() {
  const [lastAction, setLastAction] = useState("");
  const project = params.has("project");
  const actions = [
    {
      key: "new",
      label: "New chat",
      icon: <MessageSquarePlus />,
      onClick: () => setLastAction("new"),
    },
    {
      key: "chats",
      label: "Manage chats",
      icon: <MessagesSquare />,
      onClick: () => setLastAction("chats"),
    },
    ...(project
      ? [
          {
            key: "workspace",
            label: "Workspace",
            icon: <PanelRight />,
            onClick: () => setLastAction("workspace"),
          },
        ]
      : []),
    {
      key: "tasks",
      label: "Tasks",
      icon: <ListTodo />,
      disabled: params.has("disabled"),
      onClick: () => setLastAction("tasks"),
    },
  ];
  return (
    <div className={mode === "shell" ? "desktop-mobile-shell" : ""}>
      <div className="rua-owned-chat-page">
        <ChatHeader
          actions={actions}
          moreLabel="More"
          menuItems={[
            {
              key: "existing",
              label: "Existing action",
              onClick: () => setLastAction("existing"),
            },
          ]}
        >
          {project ? (
            <ProjectAgentSelector
              agents={[agent]}
              selectedAgentId={agent.id}
              loading={false}
              switching={false}
              switchable
              confirmSelection={false}
              onSelect={async () => {}}
            />
          ) : (
            <AgentBadge />
          )}
          <ProjectSelector
            currentProject={
              project
                ? {
                    project_id: "fixture-project",
                    display_name: params.has("long")
                      ? "A very long project display name that must truncate"
                      : "项目",
                    resource_device_id: "fixture-device",
                    display_path: "",
                    lifecycle: "active",
                    created_at: "2026-01-01T00:00:00Z",
                    updated_at: "2026-01-01T00:00:00Z",
                  }
                : null
            }
            projects={[]}
            onSelect={noop}
            onSelectIndependent={noop}
            onCreate={noop}
            onOverview={noop}
          />
          <div className="rua-owned-chat-title">
            A long conversation title for layout testing
          </div>
        </ChatHeader>
        <output>{lastAction}</output>
      </div>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <MemoryRouter initialEntries={["/agents/header-fixture/chats/header-chat"]}>
    <Routes>
      <Route element={<MainLayout />}>
        <Route path="/agents/:agentId/chats/:chatId" element={<Harness />} />
      </Route>
    </Routes>
  </MemoryRouter>,
);
