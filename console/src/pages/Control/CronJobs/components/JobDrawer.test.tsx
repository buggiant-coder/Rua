import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { chatApi } from "../../../../api/modules/chat";
import { cronJobApi } from "../../../../api/modules/cronjob";
import { JobDrawer } from "./JobDrawer";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("../../../../api/modules/chat", () => ({
  chatApi: { listChats: vi.fn() },
}));
vi.mock("../../../../api/modules/cronjob", () => ({
  cronJobApi: { listDispatchTargets: vi.fn() },
}));

describe("Cron job dispatch", () => {
  it("changes the saved Chat independently of the native message destination", async () => {
    vi.mocked(cronJobApi.listDispatchTargets).mockResolvedValue({
      channels: ["dingtalk"],
      items: [
        {
          chat_id: "incoming-chat",
          chat_name: "Inbox",
          user_id: "recipient",
          channel: "dingtalk",
          session_id: "native-thread",
          conversation_id: "native-room",
        },
      ],
    });
    vi.mocked(chatApi.listChats).mockResolvedValue({
      data: [
        {
          id: "project-chat",
          name: "Photo project",
          channel: "console",
          user_id: "default",
          created_at: null,
          updated_at: null,
        },
      ],
      total: 1,
    });
    const onSubmit = vi.fn();
    render(
      <JobDrawer
        open
        saving={false}
        editingJob={{
          id: "job",
          name: "Review files",
          enabled: true,
          task_type: "agent",
          schedule: { type: "cron", cron: "0 9 * * *" },
          request: { input: "Review the new photos" },
          dispatch: {
            type: "channel",
            channel: "console",
            mode: "final",
            target: { user_id: "original", session_id: "old-thread" },
          },
        }}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );
    fireEvent.click(screen.getByText("cronJobs.groupDispatch"));
    const choose = async (field: string, option: string) => {
      fireEvent.click(screen.getByRole("combobox", { name: field }));
      fireEvent.click(await screen.findByRole("option", { name: option }));
    };
    await choose("cronJobs.continueChat", "Photo project");
    await choose(
      "cronJobs.dispatchTargetPicker",
      "Inbox · dingtalk · recipient",
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    const nativeTarget = {
      user_id: "recipient",
      session_id: "native-thread",
      conversation_id: "native-room",
    };
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(onSubmit.mock.calls[0][0].dispatch).toMatchObject({
      channel: "dingtalk",
      target: { ...nativeTarget, chat_id: "project-chat" },
    });

    await choose("cronJobs.continueChat", "cronJobs.noChat");
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(2));
    expect(onSubmit.mock.calls[1][0].dispatch).toMatchObject({
      channel: "dingtalk",
      target: { ...nativeTarget, chat_id: null },
    });
  });
});
