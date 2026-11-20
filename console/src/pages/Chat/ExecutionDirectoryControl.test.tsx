import { render, screen } from "@testing-library/react";
import { it, expect, vi } from "vitest";
import type { ChatHistory } from "@/api/types";
import ExecutionDirectoryControl from "./ExecutionDirectoryControl";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const position = { device_id: "local", path: "/old", managed: false };
function show(history: Partial<ChatHistory>) {
  render(
    <ExecutionDirectoryControl
      chatId="chat"
      devices={[]}
      draft={false}
      history={
        {
          messages: [],
          meta: { execution_directory: position },
          ...history,
        } as ChatHistory
      }
      position={position}
      onSaved={vi.fn()}
      onDraftChange={vi.fn()}
    />,
  );
  return screen.getByRole("button", { name: "executionLocation.title" });
}
it.each([
  [{ active_turn: { turn_id: "running" } }, "activeTurn"],
  [{ queue: { entries: [{ input_id: "pending" }] } }, "queuedInput"],
  [{ chat_agent_state: { blockers: ["unresolved_approval"] } }, "approval"],
  [{ chat_agent_state: { blockers: ["archived_project"] } }, "archived"],
])(
  "disables a saved directory change for the reported admission blocker",
  (control, reason) => {
    const button = show({ control: control as ChatHistory["control"] });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute(
      "title",
      "executionLocation.blockers." + reason,
    );
  },
);
it("keeps the directory editable with only background work", () => {
  expect(
    show({
      control: {
        queue: { entries: [] },
        background_work: { items: [{ id: "running" }] },
      } as ChatHistory["control"],
    }),
  ).toBeEnabled();
});
