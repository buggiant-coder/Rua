import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router-dom";
import { ToastProvider } from "@/ui";
import i18n from "@/i18n";
import type { TaskRecord, TaskState } from "@/api/types/task";
import TaskActivityDrawer from "./TaskActivityDrawer";
import type {
  ChatSpec,
  ConversationBackgroundWorkView,
} from "@/api/types/chat";
import type { ConversationApiTarget } from "@/api/modules/conversation";

const collaborationApiMock = vi.hoisted(() => ({
  agents: vi.fn(),
  chats: vi.fn(),
  projectChats: vi.fn(),
}));
vi.mock("@/api/modules/agents", () => ({
  agentsApi: { listAgents: collaborationApiMock.agents },
}));
vi.mock("@/api/modules/chat", () => ({
  chatApi: { listChats: collaborationApiMock.chats },
}));
vi.mock("@/api/modules/projectChats", () => ({
  projectChatsApi: { listChats: collaborationApiMock.projectChats },
}));

const taskApiMock = vi.hoisted(() => ({
  listTasks: vi.fn(),
  getTask: vi.fn(),
  cancelTask: vi.fn(),
  retryTask: vi.fn(),
  watchTask: vi.fn(),
  unwatchTask: vi.fn(),
  sendInput: vi.fn(),
  getFullResult: vi.fn(),
  activityPath: vi.fn(
    (target: { agentId: string }, chatId: string) =>
      `/agents/${target.agentId}/chats/${chatId}/tasks/activity`,
  ),
}));

const conversationApiMock = vi.hoisted(() => ({
  getAcpPermissionSnapshot: vi.fn(),
  decideAcpPermission: vi.fn(),
}));

vi.mock("@/api/modules/task", () => ({
  taskApi: taskApiMock,
}));

vi.mock("@/api/modules/conversation", async () => {
  const actual = await vi.importActual<
    typeof import("@/api/modules/conversation")
  >("@/api/modules/conversation");
  return {
    ...actual,
    conversationApi: conversationApiMock,
  };
});

vi.mock("@/api/config", () => ({
  getApiUrl: (path: string) => `/api${path}`,
}));

vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer test" }),
}));

function taskFixture(
  state: TaskState,
  overrides: Partial<TaskRecord> = {},
): TaskRecord {
  return {
    task_id: "task-11111111-2222",
    spec: {
      owner: {
        chat_address: {
          scope: { kind: "agent", agent_id: "agent-a" },
          chat_id: "chat-a",
        },
      },
      instruction: "Inspect the release candidate",
      runner: {
        type: "tool",
        tool_name: "Fixture_Inspect",
        arguments: {},
      },
      context_refs: [],
      depends_on: [],
      origin: {},
    },
    state,
    attempt_no: 2,
    input_capability: "none",
    watch_active: false,
    created_at: "2026-07-27T01:00:00Z",
    updated_at: "2026-07-27T01:03:00Z",
    started_at: "2026-07-27T01:00:05Z",
    finished_at: state === "running" ? null : "2026-07-27T01:03:00Z",
    result:
      state === "running"
        ? null
        : {
            status: state,
            output:
              "Outcome\nCompleted\n\nKey Results\nBounded result remains readable.",
            full_result_ref: {
              uri: "task://task-11111111-2222/attempts/2/raw-output.json",
              ref_type: "task_raw_output",
              source_device_id: "device-a",
            },
            artifacts: [
              {
                id: "artifact-1",
                name: "handoff.md",
                media_type: "text/markdown",
                resource: {
                  uri: "task://task-11111111-2222/attempts/2/handoff/handoff.md",
                  ref_type: "task_handoff",
                  source_device_id: "device-a",
                },
              },
            ],
            device: {
              device_id: "device-a",
              device_name: "Desktop A",
              host_profile: "desktop",
              authority_epoch: "0000000000000007000000000000000000000000",
            },
          },
    attempts: [
      {
        attempt_no: 1,
        state: "failed",
        device: {
          device_id: "device-a",
          device_name: "Desktop A",
          authority_epoch: "0000000000000006000000000000000000000000",
        },
        created_at: "2026-07-27T00:00:00Z",
        updated_at: "2026-07-27T00:01:00Z",
        finished_at: "2026-07-27T00:01:00Z",
        result: {
          status: "failed",
          output: "Outcome\nFailed first Attempt",
          artifacts: [],
          device: {
            device_id: "device-a",
            device_name: "Desktop A",
            authority_epoch: "0000000000000006000000000000000000000000",
          },
          error: {
            kind: "runner",
            message: "first failure",
            retryable: false,
          },
        },
      },
      {
        attempt_no: 2,
        state,
        device: {
          device_id: "device-a",
          device_name: "Desktop A",
          authority_epoch: "0000000000000007000000000000000000000000",
        },
        created_at: "2026-07-27T01:00:00Z",
        updated_at: "2026-07-27T01:03:00Z",
        started_at: "2026-07-27T01:00:05Z",
        finished_at: state === "running" ? null : "2026-07-27T01:03:00Z",
        result:
          state === "running"
            ? null
            : {
                status: state,
                output:
                  "Outcome\nCompleted\n\nKey Results\nBounded result remains readable.",
                full_result_ref: {
                  uri: "task://task-11111111-2222/attempts/2/raw-output.json",
                  ref_type: "task_raw_output",
                  source_device_id: "device-a",
                },
                artifacts: [
                  {
                    id: "artifact-1",
                    name: "handoff.md",
                    media_type: "text/markdown",
                    resource: {
                      uri: "task://task-11111111-2222/attempts/2/handoff/handoff.md",
                      ref_type: "task_handoff",
                      source_device_id: "device-a",
                    },
                  },
                ],
                device: {
                  device_id: "device-a",
                  device_name: "Desktop A",
                  host_profile: "desktop",
                  authority_epoch: "0000000000000007000000000000000000000000",
                },
              },
      },
    ],
    waiting_on: [],
    blocked_by: [],
    ...overrides,
  };
}

function activityResponse(tasks: TaskRecord[]) {
  return new Response(
    `event: snapshot\ndata: ${JSON.stringify({
      type: "snapshot",
      tasks,
    })}\n\n`,
    {
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
    },
  );
}

function renderDrawer(
  tasks: TaskRecord[],
  currentDeviceId: string | null = "device-a",
  options: {
    backgroundWork?: ConversationBackgroundWorkView;
    target?: ConversationApiTarget;
  } = {},
) {
  taskApiMock.listTasks.mockResolvedValue(tasks);
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(activityResponse(tasks)));
  return render(
    <MemoryRouter>
      <I18nextProvider i18n={i18n}>
        <ToastProvider>
          <TaskActivityDrawer
            open
            onClose={vi.fn()}
            agentId="agent-a"
            chatId="chat-a"
            currentDeviceId={currentDeviceId}
            {...options}
          />
        </ToastProvider>
      </I18nextProvider>
    </MemoryRouter>,
  );
}

describe("TaskActivityDrawer", () => {
  const child = (
    id: string,
    status: "idle" | "running",
    source: Record<string, unknown> = {
      scope: { kind: "agent", agent_id: "agent-a" },
      chat_id: "chat-a",
    },
  ): ChatSpec => ({
    id,
    name: `Collaboration ${id}`,
    session_id: id,
    user_id: "user",
    channel: "console",
    created_at: null,
    updated_at: null,
    status,
    collaboration: {
      source,
      executor: { kind: "agent" },
      executor_agent_id: "agent-b",
      configuration: {
        desired_source_epoch: "1",
        applied_source_epoch: "1",
        status: "ready",
      },
    },
  });

  it("shows cross-agent collaborations above completed tasks and links to their own Chat", async () => {
    collaborationApiMock.agents.mockResolvedValue({
      agents: [{ id: "agent-a" }, { id: "agent-b" }],
    });
    collaborationApiMock.chats.mockImplementation(async (_params, agentId) => ({
      data:
        agentId === "agent-b"
          ? [
              child("child-running", "running"),
              child("child-ended", "idle"),
              child("unrelated", "running", {
                scope: { kind: "agent", agent_id: "other" },
                chat_id: "chat-a",
              }),
            ]
          : [],
      total: agentId === "agent-b" ? 3 : 0,
    }));
    renderDrawer([taskFixture("completed")]);
    const active = await screen.findByRole("region", { name: "In progress" });
    await waitFor(() =>
      expect(
        within(active).getByText("Collaboration child-running"),
      ).toBeInTheDocument(),
    );
    expect(within(active).getByRole("heading")).toHaveTextContent(
      "In progress1",
    );
    expect(
      within(screen.getByRole("region", { name: "Ended" })).getByText(
        "Collaboration child-ended",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("Collaboration unrelated"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Open collaboration chat" }),
    ).toHaveAttribute("href", "/agents/agent-b/chats/child-running");
    expect(
      screen.queryByRole("button", { name: "Cancel task" }),
    ).not.toBeInTheDocument();
    expect(collaborationApiMock.chats).toHaveBeenCalledWith(
      expect.objectContaining({
        source_chat_id: "chat-a",
        view: "collaboration",
      }),
      "agent-b",
    );
  });

  it("keeps idle collaboration ended regardless of delivery facts and uses project navigation", async () => {
    const address = {
      scope: { kind: "project" as const, project_id: "project-a" },
      chat_id: "child-project",
    };
    const source = { scope: address.scope, chat_id: "chat-a" };
    collaborationApiMock.projectChats.mockResolvedValue({
      data: [child("child-project", "idle", source)],
      total: 1,
    });
    renderDrawer([], "device-a", {
      target: { scope: "project", projectId: "project-a" },
      backgroundWork: {
        suppressed: false,
        items: [
          {
            kind: "collaboration",
            id: "request-1",
            status: "result_ready",
            chat: address,
          },
          {
            kind: "collaboration",
            id: "request-2",
            status: "delivery_failed",
            chat: address,
          },
        ],
      },
    });
    const ended = await screen.findByRole("region", { name: "Ended" });
    expect(within(ended).getAllByRole("button")).toHaveLength(1);
    expect(
      screen.getByRole("link", { name: "Open collaboration chat" }),
    ).toHaveAttribute("href", "/projects/project-a/chats/child-project");
    expect(screen.queryByText("request-1")).toBeNull();
    expect(screen.queryByText("request-2")).toBeNull();
    await waitFor(() =>
      expect(collaborationApiMock.projectChats).toHaveBeenCalledWith(
        "project-a",
        expect.objectContaining({ source_chat_id: "chat-a" }),
      ),
    );
    expect(collaborationApiMock.agents).not.toHaveBeenCalled();
  });

  it("still shows pending collaboration when the catalog fails and reports the error", async () => {
    collaborationApiMock.agents.mockRejectedValue(
      new Error("catalog unavailable"),
    );
    renderDrawer([taskFixture("completed")], "device-a", {
      backgroundWork: {
        suppressed: false,
        items: [
          {
            kind: "collaboration",
            id: "request-live",
            status: "running",
            chat: {
              scope: { kind: "agent", agent_id: "agent-b" },
              chat_id: "child-live",
            },
          },
        ],
      },
    });
    expect(
      await screen.findByRole("link", { name: "Open collaboration chat" }),
    ).toHaveAttribute("href", "/agents/agent-b/chats/child-live");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "catalog unavailable",
    );
  });
  it("groups all unfinished tasks above terminal tasks regardless of recency", async () => {
    const activeStates: TaskState[] = [
      "accepted",
      "waiting",
      "blocked",
      "running",
      "waiting_approval",
    ];
    const endedStates: TaskState[] = [
      "completed",
      "failed",
      "cancelled",
      "interrupted",
    ];
    const tasks = [...endedStates, ...activeStates].map((state, index) => {
      const task = taskFixture(state, {
        task_id: `task-${state}`,
        updated_at: `2026-07-27T01:00:${String(59 - index).padStart(2, "0")}Z`,
      });
      task.spec.instruction = `Task ${state}`;
      return task;
    });
    renderDrawer(tasks);
    const active = await screen.findByRole("region", { name: "In progress" });
    const ended = screen.getByRole("region", { name: "Ended" });
    expect(
      active.compareDocumentPosition(ended) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(within(active).getByRole("heading")).toHaveTextContent(
      "In progress5",
    );
    expect(within(ended).getByRole("heading")).toHaveTextContent("Ended4");
    expect(
      within(active)
        .getAllByRole("button")
        .map(
          (row) => row.querySelector(".rua-task-list-row-title")?.textContent,
        ),
    ).toEqual(activeStates.map((state) => `Task ${state}`));
    expect(
      within(ended)
        .getAllByRole("button")
        .map(
          (row) => row.querySelector(".rua-task-list-row-title")?.textContent,
        ),
    ).toEqual(endedStates.map((state) => `Task ${state}`));
  });

  it("moves a finished task to the ended group while keeping it selected", async () => {
    const task = taskFixture("running");
    renderDrawer([task]);
    const active = await screen.findByRole("region", { name: "In progress" });
    expect(within(active).getAllByRole("button")).toHaveLength(1);
    expect(screen.getByText("No ended tasks")).toBeInTheDocument();
    taskApiMock.listTasks.mockResolvedValue([{ ...task, state: "failed" }]);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() =>
      expect(within(active).queryByRole("button")).not.toBeInTheDocument(),
    );
    const ended = screen.getByRole("region", { name: "Ended" });
    expect(within(ended).getAllByRole("button")).toHaveLength(1);
    expect(within(ended).getByRole("button")).toHaveAttribute(
      "aria-current",
      "true",
    );
    expect(screen.getByText("No tasks in progress")).toBeInTheDocument();
  });

  it.each(["running", "completed"] as const)(
    "shows the exact command and cwd for a %s tool task without executing it",
    async (state) => {
      const command =
        "printf '%s\\n' '<script>not markup</script>'\nprintf '完成'";
      const task = taskFixture(state);
      task.spec.runner = {
        type: "tool",
        tool_name: "bash",
        arguments: {
          command,
          cwd: "/workspace/project with spaces",
          yield_seconds: 10,
        },
      };
      renderDrawer([task]);

      const dialog = await screen.findByRole("dialog");
      const execution = await within(dialog).findByRole("region", {
        name: "Execution details",
      });
      expect(within(execution).getByLabelText("Command").textContent).toBe(
        command,
      );
      expect(
        within(execution).getByText("/workspace/project with spaces"),
      ).toBeInTheDocument();
      expect(execution.querySelector("script")).toBeNull();
      fireEvent.click(
        within(execution).getByRole("button", { name: "Copy command" }),
      );
      await waitFor(() =>
        expect(navigator.clipboard.writeText).toHaveBeenCalledWith(command),
      );
      const parameters = within(execution).getByLabelText("Tool arguments");
      expect(JSON.parse(parameters.textContent!)).toEqual(
        task.spec.runner.arguments,
      );
      expect(
        dialog.querySelector(".rua-task-list-command")?.textContent,
      ).toBe(command);
      expect(taskApiMock.sendInput).not.toHaveBeenCalled();
      expect(taskApiMock.retryTask).not.toHaveBeenCalled();
    },
  );

  it("shows and copies structured arguments for non-shell tools", async () => {
    const task = taskFixture("running");
    task.spec.runner = {
      type: "tool",
      tool_name: "web_fetch",
      arguments: {
        url: "https://example.com",
        options: { enabled: true },
        limit: 0,
      },
    };
    renderDrawer([task]);
    const execution = await screen.findByRole("region", {
      name: "Execution details",
    });
    expect(
      JSON.parse(
        within(execution).getByLabelText("Tool arguments").textContent!,
      ),
    ).toEqual(task.spec.runner.arguments);
    expect(
      within(execution).queryByLabelText("Command"),
    ).not.toBeInTheDocument();
    fireEvent.click(
      within(execution).getByRole("button", { name: "Copy arguments" }),
    );
    await waitFor(() =>
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
        JSON.stringify(task.spec.runner.arguments, null, 2),
      ),
    );
  });

  it("renders an empty argument object explicitly", async () => {
    renderDrawer([taskFixture("running")]);
    const execution = await screen.findByRole("region", {
      name: "Execution details",
    });
    expect(within(execution).getByLabelText("Tool arguments").textContent).toBe(
      "{}",
    );
  });

  beforeEach(async () => {
    vi.clearAllMocks();
    collaborationApiMock.agents.mockResolvedValue({
      agents: [{ id: "agent-a" }],
    });
    collaborationApiMock.chats.mockResolvedValue({ data: [], total: 0 });
    collaborationApiMock.projectChats.mockResolvedValue({ data: [], total: 0 });
    conversationApiMock.getAcpPermissionSnapshot.mockResolvedValue({
      pending: [],
    });
    conversationApiMock.decideAcpPermission.mockResolvedValue({});
    await i18n.changeLanguage("en");
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  it("shows Task identity separately from preserved Attempts and dependencies", async () => {
    const upstream = taskFixture("completed", {
      task_id: "upstream-task",
      spec: {
        ...taskFixture("completed").spec,
        instruction: "Prepare dependency",
      },
      attempt_no: 1,
      attempts: [taskFixture("completed").attempts[1]],
    });
    const task = taskFixture("blocked", {
      blocked_by: ["upstream-task"],
    });
    renderDrawer([task, upstream]);

    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getAllByText("Inspect the release candidate"),
    ).toHaveLength(2);
    expect(within(dialog).getByRole("tab", { name: /#1/ })).toBeVisible();
    expect(within(dialog).getByRole("tab", { name: /#2/ })).toBeVisible();
    expect(within(dialog).getByText("Blocked by")).toBeVisible();

    const firstAttempt = within(dialog).getByRole("tab", { name: /#1/ });
    fireEvent.click(firstAttempt);
    await waitFor(() =>
      expect(firstAttempt).toHaveAttribute("aria-selected", "true"),
    );
    expect(dialog.querySelector(".rua-task-result-output")).toHaveTextContent(
      "Failed first Attempt",
    );
  });

  it("offers only eligible controls and refreshes from the control response", async () => {
    const running = taskFixture("running");
    const cancelled = taskFixture("cancelled");
    taskApiMock.cancelTask.mockResolvedValue(cancelled);
    renderDrawer([running]);

    const dialog = await screen.findByRole("dialog");
    const cancel = within(dialog).getAllByRole("button", {
      name: /Cancel task|取消任务/,
    })[0];
    expect(
      within(dialog).queryByRole("button", { name: /Retry|重试/ }),
    ).toBeNull();

    fireEvent.click(cancel);
    expect(taskApiMock.cancelTask).not.toHaveBeenCalled();
    const confirmCancel = within(dialog).getAllByRole("button", {
      name: /Confirm cancellation|确认取消/,
    })[0];
    expect(
      within(dialog).getByRole("button", { name: /Keep task|保留任务/ }),
    ).toBeVisible();

    fireEvent.click(confirmCancel);
    await waitFor(() =>
      expect(taskApiMock.cancelTask).toHaveBeenCalledWith(
        { scope: "agent", agentId: "agent-a" },
        "chat-a",
        running.task_id,
      ),
    );
    expect(
      await within(dialog).findByRole("button", { name: /Retry|重试/ }),
    ).toBeVisible();
    expect(
      within(dialog).queryByRole("button", {
        name: /Cancel task|取消任务|Confirm cancellation|确认取消/,
      }),
    ).toBeNull();
  });

  it("renders capability-aware Watch and steering controls", async () => {
    const running = taskFixture("running", {
      input_capability: "steering_message",
      watch_active: false,
    });
    taskApiMock.watchTask.mockResolvedValue({ active: true });
    taskApiMock.sendInput.mockResolvedValue({ accepted: true });
    taskApiMock.getTask.mockResolvedValue({
      ...running,
      watch_active: true,
    });
    renderDrawer([running]);

    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Watch" }));
    await waitFor(() =>
      expect(taskApiMock.watchTask).toHaveBeenCalledWith(
        { scope: "agent", agentId: "agent-a" },
        "chat-a",
        running.task_id,
      ),
    );
    const input = within(dialog).getByRole("textbox");
    fireEvent.change(input, { target: { value: "change direction" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(taskApiMock.sendInput).toHaveBeenCalledWith(
        { scope: "agent", agentId: "agent-a" },
        "chat-a",
        running.task_id,
        { type: "text", text: "change direction" },
      ),
    );
    expect(
      within(dialog).queryByRole("button", { name: "Close stdin" }),
    ).toBeNull();
  });

  it("shows explicit newline and EOF controls only for stdin", async () => {
    const running = taskFixture("running", {
      input_capability: "stdin",
    });
    taskApiMock.sendInput.mockResolvedValue({ accepted: true });
    taskApiMock.getTask.mockResolvedValue(running);
    renderDrawer([running]);

    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByRole("textbox");
    fireEvent.change(input, { target: { value: "yes" } });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Append newline" }),
    );
    expect(input).toHaveValue("yes\n");
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Close stdin" }),
    );
    await waitFor(() =>
      expect(taskApiMock.sendInput).toHaveBeenCalledWith(
        { scope: "agent", agentId: "agent-a" },
        "chat-a",
        running.task_id,
        { type: "close_stdin" },
      ),
    );
  });

  it("keeps bounded output visible when Device-local details are unavailable", async () => {
    renderDrawer([taskFixture("completed")], "device-b");

    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText(/Bounded result remains readable/),
    ).toBeVisible();
    expect(within(dialog).getByText(/Source device unavailable/)).toBeVisible();
    expect(
      within(dialog).getByText(/Detailed output is stored on Device device-a/),
    ).toBeVisible();
    expect(
      within(dialog).queryByRole("button", { name: /Load full result/ }),
    ).toBeNull();
  });

  it("treats Device-scoped resources as unavailable when local Device identity is unknown", async () => {
    renderDrawer([taskFixture("completed")], null);

    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText(/Bounded result remains readable/),
    ).toBeVisible();
    expect(
      within(dialog).getByText(/Detailed output is stored on Device device-a/),
    ).toBeVisible();
    expect(
      within(dialog).queryByRole("button", { name: /Load full result/ }),
    ).toBeNull();
  });
});
