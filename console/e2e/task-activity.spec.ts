import { expect, test, type Page } from "@playwright/test";
import type { TaskRecord, TaskState } from "../src/api/types/task";

const AGENT_ID = "default";

function taskRecord(
  taskId: string,
  state: TaskState,
  chatId: string,
): TaskRecord {
  const terminal = state === "interrupted";
  const result = terminal
    ? {
        status: "interrupted",
        output:
          "Outcome\nInterrupted\n\nKey Results\nBounded remote result remains readable.\n\nVerification\nStopped\n\nRemaining Work\nRetry",
        full_result_ref: {
          uri: `task://${taskId}/attempts/2/raw-output.json`,
          ref_type: "task_raw_output",
          source_device_id: "remote-task-device",
        },
        artifacts: [
          {
            id: `${taskId}:artifact`,
            name: "handoff.md",
            media_type: "text/markdown",
            resource: {
              uri: `task://${taskId}/attempts/2/handoff/handoff.md`,
              ref_type: "task_handoff",
              source_device_id: "remote-task-device",
            },
            metadata: {},
          },
        ],
        device: {
          device_id: "remote-task-device",
          device_name: "Remote workstation",
          host_profile: "desktop",
          authority_epoch: 4,
        },
        error: { message: "authority transferred", retryable: false },
      }
    : null;
  return {
    task_id: taskId,
    spec: {
      owner: {
        chat_address: {
          scope: { kind: "agent", agent_id: AGENT_ID },
          chat_id: chatId,
        },
      },
      instruction:
        state === "running"
          ? "Run long verification"
          : "Inspect remote release artifacts",
      runner:
        state === "running"
          ? { type: "tool", tool_name: "Fixture_Long", arguments: {} }
          : { type: "sub_agent" },
      context_refs: [],
      depends_on: [],
      origin: {},
    },
    state,
    attempt_no: terminal ? 2 : 1,
    input_capability: state === "running" ? "stdin" : "none",
    watch_active: false,
    created_at: "2026-07-27T00:00:00Z",
    updated_at: "2026-07-27T00:01:00Z",
    started_at: "2026-07-27T00:00:01Z",
    finished_at: terminal ? "2026-07-27T00:01:00Z" : null,
    result,
    attempts: terminal
      ? [
          {
            attempt_no: 1,
            state: "failed",
            device: {
              device_id: "remote-task-device",
              authority_epoch: 3,
            },
            created_at: "2026-07-26T23:00:00Z",
            updated_at: "2026-07-26T23:01:00Z",
            finished_at: "2026-07-26T23:01:00Z",
            result: {
              status: "failed",
              output: "Outcome\nFirst Attempt failed.",
              artifacts: [],
              device: {
                device_id: "remote-task-device",
                authority_epoch: 3,
              },
              error: { message: "first failure", retryable: false },
            },
          },
          {
            attempt_no: 2,
            state,
            device: result!.device,
            created_at: "2026-07-27T00:00:00Z",
            updated_at: "2026-07-27T00:01:00Z",
            finished_at: "2026-07-27T00:01:00Z",
            result,
          },
        ]
      : [
          {
            attempt_no: 1,
            state,
            device: {
              device_id: "local-task-device",
              authority_epoch: 4,
            },
            created_at: "2026-07-27T00:00:00Z",
            updated_at: "2026-07-27T00:01:00Z",
            started_at: "2026-07-27T00:00:01Z",
            result: null,
          },
        ],
    waiting_on: [],
    blocked_by: [],
  };
}

async function installTaskRoutes(page: Page, chatId: string) {
  const running = taskRecord("task-running", "running", chatId);
  const interrupted = taskRecord("task-interrupted", "interrupted", chatId);
  const tasks = [running, interrupted];

  await page.route(
    `**/api/agents/${AGENT_ID}/chats/${chatId}/tasks?*`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(tasks),
      });
    },
  );
  await page.route(
    `**/api/agents/${AGENT_ID}/chats/${chatId}/tasks/*`,
    async (route) => {
      const task = route.request().url().includes("task-running")
        ? running
        : interrupted;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(task),
      });
    },
  );
  await page.route(
    `**/api/agents/${AGENT_ID}/chats/${chatId}/tasks/task-running/watch`,
    async (route) => {
      running.watch_active = true;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ task_id: running.task_id, active: true }),
      });
    },
  );
  await page.route(
    `**/api/agents/${AGENT_ID}/chats/${chatId}/tasks/task-running/input`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ accepted: true }),
      });
    },
  );
  await page.route(
    `**/api/agents/${AGENT_ID}/chats/${chatId}/tasks/task-running/cancel`,
    async (route) => {
      running.state = "interrupted";
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(running),
      });
    },
  );
  await page.route(
    `**/api/agents/${AGENT_ID}/chats/${chatId}/tasks/task-interrupted/retry`,
    async (route) => {
      interrupted.state = "accepted";
      interrupted.attempt_no = 3;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(interrupted),
      });
    },
  );
  await page.route(
    `**/api/agents/${AGENT_ID}/chats/${chatId}/tasks/activity`,
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: `event: snapshot\ndata: ${JSON.stringify({
          type: "snapshot",
          tasks,
        })}\n\n`,
      });
    },
  );
  return { running, interrupted };
}

async function createChat(
  page: Page,
  baseURL: string | undefined,
): Promise<string> {
  const response = await page.request.post(
    `${baseURL ?? "http://127.0.0.1:8090"}/api/agents/${AGENT_ID}/chats`,
    {
      data: {
        name: "Task activity E2E",
        user_id: "default",
        channel: "console",
        meta: {},
      },
    },
  );
  expect(response.ok()).toBeTruthy();
  return String((await response.json()).id);
}

test("Task Activity reconstructs state and exposes eligible controls", async ({
  page,
  baseURL,
}) => {
  const chatId = await createChat(page, baseURL);
  await installTaskRoutes(page, chatId);
  await page.goto(`/agents/default/chats/${chatId}`);

  const taskActivityButton = page.getByRole("button", {
    name: /Task activity|任务活动/i,
  });
  await expect(taskActivityButton).toBeVisible();
  await page.getByRole("button", { name: /More|更多/i }).click();
  await expect(
    page.getByRole("menuitem", { name: /Task activity|任务活动/i }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  await taskActivityButton.click();
  const drawer = page.getByRole("dialog");
  await expect(drawer).toContainText("Run long verification");
  await expect(drawer).toContainText("Inspect remote release artifacts");

  await drawer.getByRole("button", { name: /Run long verification/ }).click();
  const watchRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/task-running/watch") &&
      request.method() === "POST",
  );
  await drawer.getByRole("button", { name: /Watch|监控/i }).click();
  await watchRequest;
  const input = drawer.getByRole("textbox");
  await input.fill("continue");
  await drawer
    .getByRole("button", { name: /Append newline|追加换行/i })
    .click();
  const inputRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/task-running/input") &&
      request.method() === "POST",
  );
  await drawer.getByRole("button", { name: /Send|发送/i }).click();
  await inputRequest;
  const cancelRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/task-running/cancel") &&
      request.method() === "POST",
  );
  await drawer.getByRole("button", { name: /Cancel|取消/ }).click();
  await cancelRequest;

  await drawer
    .getByRole("button", { name: /Inspect remote release artifacts/ })
    .click();
  await expect(drawer.getByRole("tab", { name: /#1/ })).toBeVisible();
  await expect(drawer.getByRole("tab", { name: /#2/ })).toBeVisible();
  await drawer.getByRole("tab", { name: /#1/ }).click();
  await expect(drawer).toContainText("First Attempt failed");
  await drawer.getByRole("tab", { name: /#2/ }).click();
  await expect(drawer).toContainText("Bounded remote result remains readable");
  await expect(drawer).toContainText(
    /Source device unavailable|来源设备不可用/i,
  );

  const retryRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/task-interrupted/retry") &&
      request.method() === "POST",
  );
  await drawer.getByRole("button", { name: /Retry|重试/ }).click();
  await retryRequest;
  await expect(page.locator("[data-rua-task-card]")).toHaveCount(0);
});

test("Task Activity remains usable in the mobile full-width drawer", async ({
  page,
  baseURL,
}) => {
  const chatId = await createChat(page, baseURL);
  await installTaskRoutes(page, chatId);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`/agents/default/chats/${chatId}`);
  await page.evaluate(() =>
    document.documentElement.classList.add("mobile-mode"),
  );

  const taskActivityButton = page.getByRole("button", {
    name: /Task activity|任务活动/i,
  });
  await expect(taskActivityButton).toBeVisible();
  await page.getByRole("button", { name: /More|更多/i }).click();
  await expect(
    page.getByRole("menuitem", { name: /Task activity|任务活动/i }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  await taskActivityButton.click();
  const drawer = page.getByRole("dialog");
  await expect(drawer).toBeVisible();
  const box = await drawer.boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.width).toBeLessThanOrEqual(390);
  await expect(
    drawer.getByRole("button", { name: /Run long verification/ }),
  ).toBeVisible();
  await drawer
    .getByRole("button", { name: /Inspect remote release artifacts/ })
    .click();
  await expect(drawer).toContainText("Bounded remote result remains readable");
});
