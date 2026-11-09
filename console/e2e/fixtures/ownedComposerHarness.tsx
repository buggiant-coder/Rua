import React, { useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { MemoryRouter } from "react-router-dom";
import { ModalProvider } from "../../src/ui/Modal";
import OwnedConversationSurface from "../../src/pages/Chat/owned/OwnedConversationSurface";
import "../../src/styles/theme.css";

if (new URLSearchParams(window.location.search).has("agent")) {
  await import("../../src/i18n");
}

const scenario = window as typeof window & {
  historyUpdates: number;
  switchComposerChat: (id: string) => void;
  composerReady: boolean;
};
scenario.historyUpdates = 0;

export default function Harness() {
  const params = new URLSearchParams(window.location.search);
  const [chatId, setChatId] = useState(params.get("chat") ?? "composer-chat");
  const historyChanged = useCallback(() => {
    scenario.historyUpdates += 1;
  }, []);
  useEffect(() => {
    scenario.switchComposerChat = setChatId;
    scenario.composerReady = true;
  }, []);
  return (
    <main
      style={{
        height: "100dvh",
        display: "flex",
        fontFamily: "sans-serif",
        fontSize: 15,
      }}
    >
      <OwnedConversationSurface
        key={chatId}
        address={{
          scope: "agent",
          agentId: params.get("agent") ?? "composer-agent",
          chatId,
        }}
        currentDeviceId={params.get("device") ?? undefined}
        onHistoryChange={historyChanged}
      />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <MemoryRouter>
    <ModalProvider>
      <Harness />
    </ModalProvider>
  </MemoryRouter>,
);
