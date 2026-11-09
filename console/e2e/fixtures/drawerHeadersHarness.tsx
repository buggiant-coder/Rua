import React from "react";
import { createRoot } from "react-dom/client";
import { MemoryRouter } from "react-router-dom";
import { PageHeader } from "../../src/components/PageHeader";
import { FileEditor } from "../../src/pages/Agent/Workspace/components/FileEditor";
import { Drawer, Btn } from "../../src/ui";
import workspaceStyles from "../../src/pages/Agent/Workspace/index.module.less";
import libraryStyles from "../../src/pages/Shared/PromptLibrary/index.module.less";
import navigationStyles from "../../src/layouts/NavigationPanel.module.less";
import "../../src/styles/theme.css";
import "../../src/styles/layout.css";
import "../../src/pages/Chat/owned-chat-page.css";

const params = new URLSearchParams(location.search);
const mode = params.get("mode");
const kind = params.get("kind");
const noop = () => {};
if (mode === "mobile") document.documentElement.classList.add("mobile-mode");
if (params.has("dark")) document.documentElement.classList.add("dark-mode");

export default function Harness() {
  const editor = kind === "editor" || kind === "library";
  return (
    <div className={mode === "desktop" ? "desktop-mobile-shell" : undefined}>
      {params.has("chat") ? (
        <header className="rua-owned-chat-header" data-testid="page-header">
          <div className="rua-owned-chat-header-main">Chat</div>
          <div className="rua-owned-chat-header-actions">
            <Btn>Action</Btn>
          </div>
        </header>
      ) : (
        <PageHeader current="Settings" className="reference-header" />
      )}
      <Drawer
        open
        onClose={noop}
        placement={params.get("placement") === "left" ? "left" : "right"}
        width={360}
        title={
          editor || kind === "navigation"
            ? undefined
            : "任务活动 — A long drawer title ".repeat(5)
        }
        className={
          editor
            ? kind === "library"
              ? libraryStyles.mobileEditorDrawer
              : workspaceStyles.mobileEditorDrawer
            : kind === "navigation"
            ? "rua-nav-drawer"
            : undefined
        }
      >
        {editor ? (
          <FileEditor
            selectedFile={{
              filename: "long-file-name-".repeat(8) + ".md",
              path: "/workspace/" + "long-path/".repeat(8),
              size: 0,
              updated_at: 0,
            }}
            fileContent=""
            loading={false}
            hasChanges
            onContentChange={noop}
            onSave={noop}
            onReset={noop}
            onDelete={noop}
            onSyncToLibrary={noop}
            onCloseDrawer={noop}
          />
        ) : kind === "navigation" ? (
          <div
            className={navigationStyles.header}
            data-testid="navigation-header"
          >
            Rua
          </div>
        ) : (
          <div style={{ height: 2000 }}>Scrollable body</div>
        )}
      </Drawer>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <MemoryRouter>
    <Harness />
  </MemoryRouter>,
);
