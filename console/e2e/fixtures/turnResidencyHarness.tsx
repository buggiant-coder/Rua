import { useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { MemoryRouter } from "react-router-dom";
import { ModalProvider } from "../../src/ui/Modal";
import ConversationMessageList from "../../src/pages/Chat/owned/ConversationMessageList";
import { createConversationProjector } from "../../src/pages/Chat/owned/projection";
import type { ConversationViewportController } from "../../src/pages/Chat/owned/useConversationViewport";
import "../../src/styles/theme.css";
import "../../src/pages/Chat/owned/owned-conversation-surface.css";

function pair(index: number) {
  return [
    {
      id: "user-" + index,
      turn_id: "turn-" + index,
      role: "user",
      content: "Question " + index,
    },
    {
      id: "answer-" + index,
      turn_id: "turn-" + index,
      role: "assistant",
      type: "message",
      content:
        "## Answer " +
        index +
        "\n\nA stable reply with content.\n\n" +
        "A paragraph to read. ".repeat(10),
    },
  ];
}
export default function Harness() {
  const [messages, setMessages] = useState(() =>
    Array.from(
      {
        length:
          Number(new URLSearchParams(window.location.search).get("count")) ||
          180,
      },
      (_, i) => pair(i + 20),
    ).flat(),
  );
  const project = useMemo(createConversationProjector, []);
  const projection = useMemo(
    () => project("turn-fixture", { messages }),
    [project, messages],
  );
  const scrollRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<ConversationViewportController | null>(null);
  const append = () =>
    setMessages((current) => [
      ...current,
      ...pair(current.filter((item) => item.role === "user").length + 20),
    ]);
  const prepend = () => {
    viewportRef.current?.beforePrepend();
    setMessages((current) => [
      ...Array.from({ length: 20 }, (_, i) => pair(i)).flat(),
      ...current,
    ]);
  };
  const stream = () =>
    setMessages((current) =>
      current.map((item, index) =>
        index === current.length - 1
          ? { ...item, content: item.content + " Another update." }
          : item,
      ),
    );
  return (
    <main
      style={{
        height: "100dvh",
        display: "flex",
        flexDirection: "column",
        containerType: "inline-size",
      }}
    >
      <div style={{ display: "flex", gap: 8 }}>
        <button onClick={append}>Append turn</button>
        <button onClick={prepend}>Prepend page</button>
        <button onClick={stream}>Stream update</button>
        <button onClick={() => viewportRef.current?.navigate("top")}>
          First
        </button>
        <button onClick={() => viewportRef.current?.navigate("bottom")}>
          Latest
        </button>
      </div>
      <div
        ref={scrollRef}
        className="rua-owned-surface-messages"
        style={{ flex: 1, containerType: "inline-size" }}
      >
        <ConversationMessageList
          scrollRef={scrollRef}
          viewportRef={viewportRef}
          viewKey="test:turn-fixture"
          projection={projection}
        />
      </div>
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
