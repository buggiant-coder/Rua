import { expect, test, type Page } from "@playwright/test";
import type { TaskRecord } from "../src/api/types/task";

const project = {
  project_id: "project-rua",
  display_name: "rua-rust",
  resource_device_id: "device-mac",
  display_path: "/Users/example/MyProjects/rua-rust",
  lifecycle: "active",
  created_at: "2026-08-01T00:00:00Z",
  updated_at: "2026-08-01T00:00:00Z",
};

const secondProject = {
  ...project,
  project_id: "project-codex",
  display_name: "codex",
  display_path: "/Users/example/MyProjects/codex",
};

const LONG_PROJECT_NAME =
  "rua-rust multi-device Project with a deliberately long name";
const LONG_RESOURCE_DEVICE_ID =
  "device-mac-resource-with-a-deliberately-long-identity";
const LONG_PROJECT_PATH =
  "/Users/example/MyProjects/rua-rust-with-a-deliberately-long-worktree-and-feature-branch-name";
const LONG_AGENT_NAME =
  "Default Repository Implementation Agent With A Long Name";
const LONG_MODEL_NAME = "Qwen Plus Project Reasoning Model With A Long Name";
const LONG_CHAT_NAME =
  "Implement concurrent Project Chats with a deliberately long title";

const runningMessages = [
  {
    id: "message-user-running",
    role: "user",
    content: "Inspect the Project runtime and keep the implementation scoped.",
  },
  {
    id: "message-reasoning-running",
    role: "assistant",
    type: "reasoning",
    content: [
      {
        type: "text",
        text: "I will inspect the relevant Project files first.",
      },
    ],
  },
  {
    id: "message-tool-call-running",
    role: "assistant",
    type: "plugin_call",
    content: [
      {
        type: "data",
        data: {
          call_id: "call-running",
          name: "bash",
          arguments: { command: "git status --short" },
        },
      },
    ],
  },
  {
    id: "message-tool-output-running",
    role: "system",
    type: "plugin_call_output",
    content: [
      {
        type: "data",
        data: {
          call_id: "call-running",
          name: "bash",
          output: "Working tree inspected.",
        },
      },
    ],
  },
  {
    id: "message-assistant-final",
    role: "assistant",
    type: "message",
    content: "The Project runtime is isolated and ready for the next change.",
  },
];

const archivedMessages = [
  {
    id: "message-user-archived",
    role: "user",
    content: "Keep this completed Project decision in stable history.",
  },
  {
    id: "message-assistant-archived",
    role: "assistant",
    type: "message",
    content: "The archived Project history remains readable.",
  },
];

function projectTaskRecord(chatId: string): TaskRecord {
  return {
    task_id: "project-task-running",
    spec: {
      owner: {
        chat_address: {
          scope: { kind: "project", project_id: project.project_id },
          chat_id: chatId,
        },
      },
      instruction: "Continue project verification",
      runner: {
        type: "tool",
        tool_name: "Fixture_ProjectTask",
        arguments: {},
      },
      context_refs: [],
      depends_on: [],
      origin: {},
    },
    state: "running",
    attempt_no: 1,
    input_capability: "steering_message",
    watch_active: false,
    created_at: "2026-08-01T04:00:00Z",
    updated_at: "2026-08-01T04:00:01Z",
    started_at: "2026-08-01T04:00:01Z",
    finished_at: null,
    result: null,
    attempts: [
      {
        attempt_no: 1,
        state: "running",
        device: {
          device_id: project.resource_device_id,
          device_name: "Project workstation",
          host_profile: "desktop",
          authority_epoch: 1,
        },
        authorization_agent_id: "default",
        created_at: "2026-08-01T04:00:00Z",
        updated_at: "2026-08-01T04:00:01Z",
        started_at: "2026-08-01T04:00:01Z",
        result: null,
      },
    ],
    waiting_on: [],
    blocked_by: [],
  };
}

type ProjectChatMockOptions = {
  lifecycle?: "active" | "archived";
  longLabels?: boolean;
};

async function mockProjectChatApis(
  page: Page,
  options: ProjectChatMockOptions = {},
) {
  const projectFixture = {
    ...project,
    lifecycle: options.lifecycle ?? project.lifecycle,
    ...(options.longLabels
      ? {
          display_name: LONG_PROJECT_NAME,
          resource_device_id: LONG_RESOURCE_DEVICE_ID,
          display_path: LONG_PROJECT_PATH,
        }
      : {}),
  };
  const defaultAgentName = options.longLabels
    ? LONG_AGENT_NAME
    : "Default Agent";
  const modelName = options.longLabels ? LONG_MODEL_NAME : "Qwen Plus";
  const chats = [
    {
      id: "chat-pinned",
      user_id: "project-rua",
      channel: "console",
      name: "Pinned project notes",
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T01:00:00Z",
      status: "idle",
      pinned: true,
      pin_order: 1,
      model: {
        provider_id: "dashscope",
        model: "qwen-plus",
        thinking_strength: "auto",
      },
    },
    {
      id: "chat-running",
      user_id: "project-rua",
      channel: "console",
      name: options.longLabels ? LONG_CHAT_NAME : "Implement concurrent chats",
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T03:00:00Z",
      status: "running",
      pinned: false,
      pin_order: 0,
      model: {
        provider_id: "dashscope",
        model: "qwen-plus",
        thinking_strength: "auto",
      },
    },
    {
      id: "chat-review",
      user_id: "project-rua",
      channel: "console",
      name: "Review drawer",
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T02:00:00Z",
      status: "idle",
      pinned: false,
      pin_order: 0,
      model: {
        provider_id: "dashscope",
        model: "qwen-plus",
        thinking_strength: "auto",
      },
    },
  ];
  const managementRequests: Array<{
    method: string;
    chatId: string;
    force?: string | null;
    body?: unknown;
  }> = [];
  const apiRequests: Array<{ method: string; path: string; body?: unknown }> =
    [];
  let acpActive = false;
  let selectedAgentId = "default";
  let selectionRevision = 1;
  const projectTasks = new Map<string, TaskRecord>();

  await page.addInitScript(() => {
    sessionStorage.setItem(
      "rua_project_lazy_chat_project-rua:00000000-0000-4000-8000-000000000405",
      "1",
    );
    sessionStorage.setItem(
      "rua_project_chat_agent_project-rua:00000000-0000-4000-8000-000000000405",
      "default",
    );
  });
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const pathname = url.pathname;
    if (!pathname.startsWith("/api/")) {
      return route.continue();
    }
    const path = pathname.replace(/^\/api/, "");
    const rawBody = route.request().postData();
    let requestBody: unknown;
    if (rawBody) {
      try {
        requestBody = JSON.parse(rawBody);
      } catch {
        requestBody = rawBody;
      }
    }
    apiRequests.push({
      method: route.request().method(),
      path,
      ...(requestBody === undefined ? {} : { body: requestBody }),
    });
    const json = (
      body: unknown,
      responseStatus = 200,
      headers: Record<string, string> = {},
    ) =>
      route.fulfill({
        status: responseStatus,
        headers: {
          "Content-Type": "application/json",
          ...headers,
        },
        body: JSON.stringify(body),
      });
    if (path === "/auth/status") {
      return json({ enabled: false, has_users: false });
    }
    if (path === "/sync/status") {
      return json({ enabled: false, paired: false, peer_count: 0 });
    }
    if (path === "/sync/devices/status") {
      return json({ revocation_state: "active", is_revoked: false });
    }
    if (path === "/sync/devices") {
      return json({
        devices: [
          {
            device_id: "controller",
            public_key: "controller-key",
            friendly_name: "Phone",
            label: "Android",
            added_at_ms: 1,
            last_seen_ms: Date.now(),
            is_self: true,
            connectivity_state: "connected",
          },
        ],
      });
    }
    if (path === "/agents") {
      return json({
        agents: [
          {
            id: "default",
            name: defaultAgentName,
            description: "General project work",
            workspace_dir: "/agents/default",
            enabled: true,
          },
          {
            id: "code",
            name: "Code Agent",
            description: "Repository implementation",
            workspace_dir: "/agents/code",
            enabled: true,
          },
          {
            id: "disabled",
            name: "Disabled Agent",
            description: "Unavailable",
            workspace_dir: "/agents/disabled",
            enabled: false,
          },
        ],
      });
    }
    if (path === "/models") {
      return json([
        {
          id: "dashscope",
          name: "DashScope",
          api_key: "sk-test",
          require_api_key: true,
          is_custom: false,
          base_url: "",
          models: [
            { id: "qwen-plus", name: modelName },
            { id: "qwen-max", name: "Qwen Max" },
          ],
          extra_models: [],
        },
      ]);
    }
    if (path === "/models/templates") {
      return json([]);
    }
    if (path === "/models/active") {
      return json({
        default_llm: { provider_id: "dashscope", model: "qwen-plus" },
      });
    }
    if (path === "/projects") {
      return json({ projects: [projectFixture, secondProject] });
    }
    if (path === "/projects/project-rua") {
      return json(projectFixture);
    }
    if (path === "/projects/project-codex") {
      return json(secondProject);
    }
    if (
      path === "/projects/project-rua/chats" &&
      route.request().method() === "GET"
    ) {
      const search = (url.searchParams.get("search") || "").toLowerCase();
      const offset = Number(url.searchParams.get("offset") || 0);
      const limit = Number(url.searchParams.get("limit") || 20);
      const filtered = orderedChats(chats).filter((chat) =>
        chat.name.toLowerCase().includes(search),
      );
      return json(filtered.slice(offset, offset + limit), 200, {
        "X-Total-Count": String(filtered.length),
      });
    }
    const controlMatch = path.match(
      /^\/projects\/project-rua\/chats\/([^/]+)\/(.+)$/,
    );
    if (controlMatch) {
      const chatId = decodeURIComponent(controlMatch[1]);
      const child = controlMatch[2];
      const chat = chats.find((candidate) => candidate.id === chatId);
      if (!chat) return json({ detail: "Project Chat not found" }, 404);
      const task = projectTasks.get(chatId) ?? projectTaskRecord(chatId);
      projectTasks.set(chatId, task);
      const taskPath = `tasks/${task.task_id}`;

      if (child === "tasks" && route.request().method() === "GET") {
        return json([task]);
      }
      if (child === "tasks/activity" && route.request().method() === "GET") {
        return route.fulfill({
          status: 200,
          headers: { "Content-Type": "text/event-stream" },
          body: `event: snapshot\ndata: ${JSON.stringify({
            type: "snapshot",
            tasks: [task],
          })}\n\n`,
        });
      }
      if (child === taskPath && route.request().method() === "GET") {
        return json(task);
      }
      if (
        child === `${taskPath}/watch` &&
        route.request().method() === "POST"
      ) {
        task.watch_active = true;
        return json({ task_id: task.task_id, active: true });
      }
      if (
        child === `${taskPath}/unwatch` &&
        route.request().method() === "POST"
      ) {
        task.watch_active = false;
        return json({ task_id: task.task_id, active: false });
      }
      if (
        child === `${taskPath}/input` &&
        route.request().method() === "POST"
      ) {
        return json({ accepted: true });
      }
      if (
        child === `${taskPath}/cancel` &&
        route.request().method() === "POST"
      ) {
        task.state = "cancelled";
        task.input_capability = "none";
        task.finished_at = "2026-08-01T04:01:00Z";
        task.attempts[task.attempts.length - 1].state = "cancelled";
        task.attempts[task.attempts.length - 1].finished_at = task.finished_at;
        return json(task);
      }
      if (
        child === `${taskPath}/retry` &&
        route.request().method() === "POST"
      ) {
        task.state = "accepted";
        task.attempt_no += 1;
        task.finished_at = null;
        task.result = null;
        task.attempts.push({
          attempt_no: task.attempt_no,
          state: "accepted",
          device: {
            device_id: project.resource_device_id,
            device_name: "Project workstation",
            host_profile: "desktop",
            authority_epoch: 1,
          },
          authorization_agent_id: selectedAgentId,
          created_at: "2026-08-01T04:02:00Z",
          updated_at: "2026-08-01T04:02:00Z",
          result: null,
        });
        return json(task);
      }

      if (child === "options" && route.request().method() === "GET") {
        return json(runningConfig());
      }
      if (child === "options" && route.request().method() === "PUT") {
        return json(requestBody);
      }
      if (child === "resource-policy") {
        return json({
          policy: {
            profile: "resource-write",
            source: "project_default",
            cwd: projectFixture.display_path,
            writable_roots: [projectFixture.display_path],
            dedicated_operations_root_confined: true,
            tool_guard_enabled: true,
            approval_mode: "ask_first",
          },
          pending_approval_count: 0,
          chat_rule_count: 1,
          agent_rule_count: 2,
        });
      }
      if (child === "console/stop" && route.request().method() === "POST") {
        chat.status = "idle";
        return json({ stopped: true });
      }
      if (child === "approvals/snapshot") {
        return json({ pending: [], resolved: [] });
      }
      if (child === "acp/permissions") {
        return json({ pending: [] });
      }
      if (child === "acp/runners") {
        return json([
          {
            key: "codex",
            enabled: true,
            command: "codex",
            args: [],
            env: {},
            trusted: true,
            tool_parse_mode: "call_detail",
            stdio_buffer_limit_bytes: 1048576,
            is_builtin: true,
          },
        ]);
      }
      if (child === "acp/sessions") {
        return json([]);
      }
      if (child === "acp/enter" && route.request().method() === "POST") {
        acpActive = true;
        return json({
          ok: true,
          message: "Entered ACP",
          history_preview: null,
          status: {
            active: true,
            runner: "codex",
            session_id: "session-codex",
            cwd: projectFixture.display_path,
            chat_id: chatId,
            workspace_dir: projectFixture.display_path,
            target_device_id: projectFixture.resource_device_id,
          },
        });
      }
      if (child === "acp/exit" && route.request().method() === "POST") {
        acpActive = false;
        return json({ ok: true, message: "Exited ACP" });
      }
      if (child === "acp/status") {
        return json({
          active: acpActive,
          runner: acpActive ? "codex" : null,
          session_id: acpActive ? "session-codex" : null,
          cwd: acpActive ? projectFixture.display_path : null,
          chat_id: chatId,
          workspace_dir: projectFixture.display_path,
          target_device_id: projectFixture.resource_device_id,
        });
      }
      if (child === "agent-selection" && route.request().method() === "PATCH") {
        const body = requestBody as {
          expected_agent_id?: string;
          agent_id?: string;
        };
        if (
          body.expected_agent_id !== selectedAgentId ||
          typeof body.agent_id !== "string"
        ) {
          return json({ detail: "Project Chat Agent changed" }, 409);
        }
        selectedAgentId = body.agent_id;
        selectionRevision += 1;
        return json({
          selection: {
            agent_id: selectedAgentId,
            selected_at: "2026-08-01T04:00:00Z",
            revision: selectionRevision,
          },
          selected_agent_available: true,
          switchable: true,
          blockers: [],
        });
      }
    }
    const chatMatch = path.match(/^\/projects\/project-rua\/chats\/([^/]+)$/);
    if (chatMatch) {
      const chatId = decodeURIComponent(chatMatch[1]);
      const chat = chats.find((candidate) => candidate.id === chatId);
      if (!chat) return json({ detail: "Project Chat not found" }, 404);

      if (route.request().method() === "GET") {
        return json({
          messages:
            projectFixture.lifecycle === "archived"
              ? archivedMessages
              : chat.status === "running"
              ? runningMessages
              : [],
          title: chat.name,
          status: chat.status,
          model: chat.model,
          control: {
            revision: 1,
            placement: {
              authority_device_id: projectFixture.resource_device_id,
              authority_epoch: "epoch-project-rua",
              switchable: false,
            },
            active_turn:
              chat.status === "running"
                ? {
                    turn_id: "turn-running",
                    phase: "streaming",
                    authority_device_id: projectFixture.resource_device_id,
                    authority_epoch: "epoch-project-rua",
                    driver: "rua",
                    started_at: "2026-08-01T03:00:00Z",
                    queued_count: 0,
                  }
                : null,
            queue: { paused: false, entries: [] },
            acp: { active: false },
            allowed_controls: {
              stop: chat.status === "running",
              queue: true,
              interject: true,
              edit_queued_input: true,
              delete_queued_input: true,
              switch_authority: false,
              enter_acp: true,
              exit_acp: false,
            },
            project_chat_agent_state: {
              selection: {
                agent_id: selectedAgentId,
                selected_at: "2026-08-01T00:00:00Z",
                revision: selectionRevision,
              },
              selected_agent_available: true,
              switchable: chat.status !== "running",
              blockers: chat.status === "running" ? ["active_turn"] : [],
            },
          },
        });
      }
      if (route.request().method() === "PATCH") {
        const body = route.request().postDataJSON() as Partial<
          (typeof chats)[number]
        >;
        managementRequests.push({
          method: "PATCH",
          chatId,
          body,
        });
        Object.assign(chat, body);
        return json(chat);
      }
      if (route.request().method() === "DELETE") {
        const force = url.searchParams.get("force");
        managementRequests.push({ method: "DELETE", chatId, force });
        if (chat.status === "running" && force !== "true") {
          return json({ detail: "Project Chat is busy" }, 409);
        }
        const index = chats.findIndex((candidate) => candidate.id === chatId);
        chats.splice(index, 1);
        return json({
          deleted: true,
          chat_id: chatId,
          was_busy: chat.status === "running",
        });
      }
    }
    return json({});
  });

  return { chats, managementRequests, apiRequests };
}

function orderedChats<
  T extends { pinned: boolean; pin_order: number; updated_at: string },
>(chats: T[]): T[] {
  return [...chats].sort((left, right) => {
    if (left.pinned !== right.pinned) return left.pinned ? -1 : 1;
    if (left.pinned && left.pin_order !== right.pin_order) {
      return left.pin_order - right.pin_order;
    }
    return Date.parse(right.updated_at) - Date.parse(left.updated_at);
  });
}

function runningConfig() {
  return {
    max_iters: 100,
    auto_continue_on_text_only: false,
    llm_retry_enabled: true,
    llm_max_retries: 3,
    llm_backoff_base: 1,
    llm_backoff_cap: 30,
    llm_max_concurrent: 5,
    llm_max_qpm: 0,
    llm_rate_limit_pause: 30,
    llm_rate_limit_jitter: 0,
    llm_acquire_timeout: 120,
    max_input_length: 131072,
    history_max_length: 10000,
    context_compact: {
      token_count_model: "default",
      token_count_use_mirror: false,
      token_count_estimate_divisor: 4,
      context_compact_enabled: true,
      blocking_compact_ratio: 0.95,
      history_replacement_enabled: false,
      early_background_compaction_enabled: false,
      early_compaction_target_ratio: 0.5,
      context_reserve_ratio: 0.1,
      compact_with_thinking_block: true,
      compact_enhanced_prompt: true,
      thinking_turns_to_keep: 1,
      min_keep_messages: 5,
    },
    tool_result_compact: {
      enabled: true,
      projection_truncation_enabled: false,
      recent_n: 2,
      old_max_bytes: 4000,
      execution_output_max_tokens: 10000,
      retention_days: 5,
    },
    memory_summary: {
      memory_summary_enabled: true,
      dream_cron: "0 23 * * *",
      search_mode: "precise",
    },
    embedding_config: {
      backend: "",
      api_key: "",
      base_url: "",
      model_name: "",
      dimensions: 1024,
      enable_cache: true,
      use_dimensions: true,
      max_cache_size: 1000,
      max_input_length: 8192,
      max_batch_size: 16,
    },
    memory_manager_backend: "remelight",
  };
}

async function openProjectChat(
  page: Page,
  mobile: boolean,
  chatId = "00000000-0000-4000-8000-000000000405",
  options: ProjectChatMockOptions = {},
) {
  await mockProjectChatApis(page, options);
  await page.setViewportSize(
    mobile ? { width: 390, height: 844 } : { width: 1440, height: 900 },
  );
  const suffix = mobile ? "?_mobile=1" : "";
  await page.goto(`/projects/project-rua/chats/${chatId}${suffix}`, {
    waitUntil: "networkidle",
  });
  const projectName = options.longLabels ? LONG_PROJECT_NAME : "rua-rust";
  const agentName = options.longLabels ? LONG_AGENT_NAME : "Default Agent";
  await expect(page.getByText(projectName, { exact: true })).toBeVisible();
  if (mobile) {
    await expect(page.locator(`[title="${agentName}"]`)).toBeVisible();
  } else {
    await expect(page.getByText(agentName, { exact: true })).toBeVisible();
  }
  const input = page.locator("textarea").last();
  await expect(input).toBeEditable();
  const widths = await page.evaluate(() => ({
    document: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
  }));
  expect(widths.document).toBeLessThanOrEqual(widths.viewport + 1);
  return input;
}

async function expectNoHorizontalScroll(page: Page) {
  const widths = await page.evaluate(() => ({
    document: document.documentElement.scrollWidth,
    viewport: document.documentElement.clientWidth,
    body: document.body.scrollWidth,
  }));
  expect(widths.document).toBeLessThanOrEqual(widths.viewport + 1);
  expect(widths.body).toBeLessThanOrEqual(widths.viewport + 1);
}

test("Project Lazy Chat fits desktop and keeps production composer", async ({
  page,
}, testInfo) => {
  const input = await openProjectChat(page, false);
  await input.fill("Inspect the repository");
  await expect(
    page.getByRole("button", { name: /Send message|发送消息/ }),
  ).toBeEnabled();
  await page.screenshot({
    path: testInfo.outputPath("project-chat-desktop.png"),
    fullPage: false,
  });
});

test("Project running Chat keeps the Codex conversation layout and blocks Agent switching", async ({
  page,
}, testInfo) => {
  const composer = await openProjectChat(page, false, "chat-running");
  const userMessage = page.getByText(
    "Inspect the Project runtime and keep the implementation scoped.",
  );
  const assistantMessage = page.getByText(
    "The Project runtime is isolated and ready for the next change.",
  );
  await expect(userMessage).toBeVisible();
  await page
    .getByRole("button", {
      name: /1 个思考, 1 个工具调用|1 thought, 1 tool call/i,
    })
    .click();
  await expect(page.getByText("Thinking", { exact: true })).toBeVisible();
  await expect(page.getByText("bash", { exact: true })).toBeVisible();
  await expect(assistantMessage).toBeVisible();

  const agentControl = page.getByRole("button", {
    name: "Project Chat Agent",
  });
  await expect(agentControl).toHaveAttribute(
    "data-switch-blocked-reason",
    "Finish or stop the active Turn before switching Agent",
  );
  await agentControl.click();
  await expect(page.getByRole("button", { name: /Code Agent/ })).toBeDisabled();
  await page.waitForTimeout(220);

  const [assistantBox, composerBox] = await Promise.all([
    assistantMessage.boundingBox(),
    composer.boundingBox(),
  ]);
  expect(assistantBox).not.toBeNull();
  expect(composerBox).not.toBeNull();
  expect(assistantBox!.y + assistantBox!.height).toBeLessThanOrEqual(
    composerBox!.y + 1,
  );
  await expectNoHorizontalScroll(page);
  await page.screenshot({
    path: testInfo.outputPath("project-running-switch-blocked-desktop.png"),
    fullPage: false,
  });
});

test("Project running Chat keeps messages and Composer stable on mobile", async ({
  page,
}, testInfo) => {
  const composer = await openProjectChat(page, true, "chat-running");
  const assistantMessage = page.getByText(
    "The Project runtime is isolated and ready for the next change.",
  );
  await expect(assistantMessage).toBeVisible();
  const [assistantBox, composerBox] = await Promise.all([
    assistantMessage.boundingBox(),
    composer.boundingBox(),
  ]);
  expect(assistantBox).not.toBeNull();
  expect(composerBox).not.toBeNull();
  expect(assistantBox!.y + assistantBox!.height).toBeLessThanOrEqual(
    composerBox!.y + 1,
  );
  await expectNoHorizontalScroll(page);
  await page.screenshot({
    path: testInfo.outputPath("project-running-mobile.png"),
    fullPage: false,
  });
});

test("Archived Project keeps history visible and exposes lifecycle actions", async ({
  page,
}, testInfo) => {
  const composer = await openProjectChat(page, false, "chat-pinned", {
    lifecycle: "archived",
  });
  await expect(page.getByText("Archived", { exact: true })).toBeVisible();
  await expect(
    page.getByText("The archived Project history remains readable."),
  ).toBeVisible();
  await expect(composer).toBeEditable();
  await expect(
    page.getByRole("button", { name: /Send message|发送消息/ }),
  ).toBeDisabled();
  await page.getByRole("button", { name: "More Project actions" }).click();
  await expect(page.getByRole("menuitem", { name: "Reopen" })).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Permanently Delete" }),
  ).toBeVisible();
  await page.waitForTimeout(220);
  await expectNoHorizontalScroll(page);
  await page.screenshot({
    path: testInfo.outputPath("project-archived-desktop.png"),
    fullPage: false,
  });
});

test("Long Project labels remain contained in the production workspace", async ({
  page,
}, testInfo) => {
  await openProjectChat(page, false, "chat-running", {
    longLabels: true,
  });
  await page.getByRole("button", { name: "Workspace" }).click();
  const drawer = page.getByRole("complementary", { name: "Workspace" });
  await expect(drawer).toBeVisible();
  await expect(
    drawer.getByText(LONG_PROJECT_NAME, { exact: true }),
  ).toBeVisible();
  await expect(
    drawer.getByText(LONG_RESOURCE_DEVICE_ID.slice(0, 12)),
  ).toBeVisible();
  await expect(drawer.getByText(LONG_CHAT_NAME)).toBeVisible();
  await expect(
    drawer.getByText(LONG_PROJECT_PATH, { exact: true }),
  ).toHaveCount(2);
  await expect(page.getByText(LONG_AGENT_NAME, { exact: true })).toBeVisible();
  await expect(page.getByText(LONG_MODEL_NAME, { exact: true })).toBeVisible();
  await page.waitForTimeout(220);
  const drawerBox = await drawer.boundingBox();
  expect(drawerBox).not.toBeNull();
  expect(drawerBox!.x + drawerBox!.width).toBeLessThanOrEqual(1441);
  await expectNoHorizontalScroll(page);
  await page.screenshot({
    path: testInfo.outputPath("project-long-labels-workspace-desktop.png"),
    fullPage: false,
  });
});

test("Project Agent selector opens from the controller-local replica", async ({
  page,
}, testInfo) => {
  let switchRequests = 0;
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.endsWith("/agent-selection")) {
      switchRequests += 1;
    }
  });
  await openProjectChat(page, false);
  await page
    .getByRole("button", { name: /Project Chat Agent|项目对话智能体/i })
    .click();
  await expect(
    page.getByText("Project Chat Agent", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Code Agent", { exact: true })).toBeVisible();
  await expect(page.getByText("Disabled Agent", { exact: true })).toBeVisible();
  expect(switchRequests).toBe(0);
  await page.waitForTimeout(220);
  await page.screenshot({
    path: testInfo.outputPath("project-agent-selector-desktop.png"),
    fullPage: false,
  });
});

test("Project Task controls follow the current Agent after a switch", async ({
  page,
}) => {
  const state = await mockProjectChatApis(page);
  await page.goto("/projects/project-rua/chats/chat-review", {
    waitUntil: "domcontentloaded",
  });

  const agentControl = page.getByRole("button", {
    name: /Project Chat Agent|项目对话智能体/i,
  });
  await agentControl.click();
  await page.getByRole("button", { name: /Code Agent/ }).click();
  const switchRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/agent-selection") &&
      request.method() === "PATCH",
  );
  const confirmation = page.getByRole("dialog");
  await confirmation
    .getByRole("button", { name: /Switch Agent|切换智能体/i })
    .click();
  await switchRequest;
  await expect(confirmation).not.toBeVisible();
  await expect(agentControl).toContainText("Code Agent");

  await page.getByRole("button", { name: /Task activity|任务活动/i }).click();
  const drawer = page.getByRole("dialog");
  await expect(drawer).toContainText("Continue project verification");

  const watchRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/project-task-running/watch") &&
      request.method() === "POST",
  );
  await drawer.getByRole("button", { name: /Watch|监控/i }).click();
  await watchRequest;
  await expect(
    drawer.getByRole("button", { name: /Unwatch|取消监控/i }),
  ).toBeVisible();

  const unwatchRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/project-task-running/unwatch") &&
      request.method() === "POST",
  );
  await drawer.getByRole("button", { name: /Unwatch|取消监控/i }).click();
  await unwatchRequest;

  const inputRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/project-task-running/input") &&
      request.method() === "POST",
  );
  await drawer.locator("textarea").fill("Re-check the current branch");
  await drawer.getByRole("button", { name: /Send|发送/i }).click();
  const acceptedInput = await inputRequest;
  expect(acceptedInput.postDataJSON()).toEqual({
    submission_id: expect.any(String),
    input: {
      type: "text",
      text: "Re-check the current branch",
    },
  });

  const cancelRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/project-task-running/cancel") &&
      request.method() === "POST",
  );
  await drawer.getByRole("button", { name: /Cancel|取消/ }).click();
  await cancelRequest;
  await expect(
    drawer.getByRole("button", { name: /Retry|重试/i }),
  ).toBeVisible();

  const retryRequest = page.waitForRequest(
    (request) =>
      request.url().endsWith("/project-task-running/retry") &&
      request.method() === "POST",
  );
  await drawer.getByRole("button", { name: /Retry|重试/i }).click();
  const retry = await retryRequest;
  expect(retry.postDataJSON()).toEqual({ authorization_agent_id: "code" });
  expect(retry.url()).toContain(
    "/api/projects/project-rua/chats/chat-review/tasks/project-task-running/retry",
  );
  expect(
    state.apiRequests.some(
      (request) =>
        request.path ===
          "/projects/project-rua/chats/chat-review/tasks/project-task-running/retry" &&
        request.method === "POST",
    ),
  ).toBe(true);
  await expect(
    drawer.locator('[data-task-state="accepted"]').first(),
  ).toBeVisible();
});

test("Project selector changes the shell and Chat in one navigation", async ({
  page,
}, testInfo) => {
  await openProjectChat(page, false);
  await page.getByRole("button", { name: "Select Project" }).click();
  await expect(page.getByRole("button", { name: /codex/ })).toBeVisible();
  await page.waitForTimeout(220);
  await page.screenshot({
    path: testInfo.outputPath("project-selector-desktop.png"),
    fullPage: false,
  });

  await page.getByRole("button", { name: /codex/ }).click();
  await expect(page).toHaveURL(/\/projects\/project-codex\/chats\/[^/?]+$/);
  await expect(page.getByText("codex", { exact: true })).toBeVisible();
  await expect(page.getByText("rua-rust", { exact: true })).toHaveCount(0);
});

test("Project drawers are right-side, exclusive, and keep the composer mounted", async ({
  page,
}, testInfo) => {
  await openProjectChat(page, false, "chat-running");
  const composer = page.locator("textarea").last();

  await page.getByRole("button", { name: "Project Chats" }).click();
  const chatsDrawer = page.getByRole("complementary", {
    name: "Project Chats",
  });
  await expect(chatsDrawer).toBeVisible();
  await expect(
    chatsDrawer.getByText("Implement concurrent chats"),
  ).toBeVisible();
  await expect(chatsDrawer.getByText("running")).toBeVisible();
  await expect(chatsDrawer.getByText(/Archive/i)).toHaveCount(0);
  await page.waitForTimeout(220);
  const [composerBox, chatsBox] = await Promise.all([
    composer.boundingBox(),
    chatsDrawer.boundingBox(),
  ]);
  expect(composerBox).not.toBeNull();
  expect(chatsBox).not.toBeNull();
  expect(composerBox!.x + composerBox!.width).toBeLessThanOrEqual(
    chatsBox!.x + 1,
  );
  await page.screenshot({
    path: testInfo.outputPath("project-chats-drawer-desktop.png"),
    fullPage: false,
  });

  await page.getByRole("button", { name: "Workspace" }).click();
  await expect(chatsDrawer).toHaveCount(0);
  await expect(
    page.getByRole("complementary", { name: "Workspace" }),
  ).toBeVisible();
  await expect(page.getByText("Resource write")).toBeVisible();
  await expect(page.getByText("0 pending · 1 Chat · 2 Agent")).toBeVisible();
  await expect(page.getByText("Root confined")).toBeVisible();
  await expect(composer).toBeEditable();
  await page.waitForTimeout(220);
  await page.screenshot({
    path: testInfo.outputPath("project-workspace-drawer-desktop.png"),
    fullPage: false,
  });
});

test("Project Lazy Chat keeps mobile draft editable while offline", async ({
  page,
}, testInfo) => {
  const input = await openProjectChat(page, true);
  await expect(
    page.getByTitle("resource-device · Not connected"),
  ).toBeVisible();
  await input.fill("Keep this phone draft");
  await expect(input).toHaveValue("Keep this phone draft");
  await expect(
    page.getByRole("button", { name: /Send message|发送消息/ }),
  ).toBeEnabled();
  await page.screenshot({
    path: testInfo.outputPath("project-chat-mobile-offline.png"),
    fullPage: false,
  });
});

test("Project Chats drawer uses the full mobile workspace", async ({
  page,
}, testInfo) => {
  await openProjectChat(page, true);
  await page.getByRole("button", { name: "Project Chats" }).click();
  const drawer = page.getByRole("complementary", { name: "Project Chats" });
  await expect(drawer).toBeVisible();
  await expect(drawer.getByText("Pinned project notes")).toBeVisible();
  await expect(
    drawer.getByRole("button", { name: "Rename Pinned project notes" }),
  ).toBeVisible();
  await page.waitForTimeout(220);
  const box = await drawer.boundingBox();
  expect(box).not.toBeNull();
  expect(Math.round(box!.width)).toBe(390);
  await page.screenshot({
    path: testInfo.outputPath("project-chats-drawer-mobile.png"),
    fullPage: false,
  });
});

test("Project Chats drawer opens an existing Chat on mobile", async ({
  page,
}) => {
  await page.addInitScript(() => localStorage.setItem("language", "en"));
  await openProjectChat(page, true);
  const historyLength = await page.evaluate(() => window.history.length);
  await page.getByRole("button", { name: "Project Chats" }).click();

  await page
    .locator("button.rua-owned-chat-list-item-main")
    .filter({ hasText: "Implement concurrent chats" })
    .click();

  await expect(page).toHaveURL(
    /\/projects\/project-rua\/chats\/chat-running$/,
  );
  expect(await page.evaluate(() => window.history.length)).toBe(
    historyLength + 1,
  );
  await expect(
    page.getByText(
      "Inspect the Project runtime and keep the implementation scoped.",
    ),
  ).toBeVisible();
  await expect(page.locator(".rua-owned-session-drawer")).toHaveCount(0);

  await page.getByRole("button", { name: "Project Chats" }).click();
  await page
    .locator("button.rua-owned-chat-list-item-main")
    .filter({ hasText: "Implement concurrent chats" })
    .click();

  await expect(page).toHaveURL(
    /\/projects\/project-rua\/chats\/chat-running$/,
  );
  await expect(page.locator(".rua-owned-session-drawer")).toHaveCount(0);
});

test("Project Chat drawer manages rows and busy deletion without stopping siblings", async ({
  page,
}) => {
  const state = await mockProjectChatApis(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  const lazyChatId = "00000000-0000-4000-8000-000000000405";
  await page.goto(`/projects/project-rua/chats/${lazyChatId}`, {
    waitUntil: "networkidle",
  });

  await page.getByRole("button", { name: "Project Chats" }).click();
  const drawer = page.getByRole("complementary", { name: "Project Chats" });
  await expect(drawer.getByText("Review drawer")).toBeVisible();

  await drawer
    .getByRole("textbox", { name: "Search Project Chats" })
    .fill("review");
  await expect(drawer.getByText("Review drawer")).toBeVisible();
  await expect(drawer.getByText("Implement concurrent chats")).toHaveCount(0);
  await drawer.getByRole("textbox", { name: "Search Project Chats" }).fill("");
  await expect(drawer.getByText("Implement concurrent chats")).toBeVisible();

  await drawer.getByRole("button", { name: "Rename Review drawer" }).click();
  await page
    .getByRole("textbox", { name: "Project Chat name" })
    .fill("Renamed review");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(drawer.getByText("Renamed review")).toBeVisible();

  await drawer.getByRole("button", { name: "Delete Renamed review" }).click();
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(drawer.getByText("Renamed review")).toHaveCount(0);
  await expect(page).toHaveURL(
    new RegExp(`/projects/project-rua/chats/${lazyChatId}$`),
  );

  await drawer.getByText("Implement concurrent chats").click();
  await expect(page).toHaveURL(
    /\/projects\/project-rua\/chats\/chat-running$/,
  );
  await page.getByRole("button", { name: "Project Chats" }).click();
  const runningDrawer = page.getByRole("complementary", {
    name: "Project Chats",
  });
  await runningDrawer
    .getByRole("button", { name: "Delete Implement concurrent chats" })
    .click();
  await expect(page.getByText(/hard-stop only this Chat/i)).toBeVisible();
  await page.getByRole("button", { name: "Stop and Delete" }).click();

  await expect(page).toHaveURL(
    /\/projects\/project-rua\/chats\/chat-pinned$/,
  );
  expect(state.managementRequests).toEqual(
    expect.arrayContaining([
      expect.objectContaining({
        method: "PATCH",
        chatId: "chat-review",
        body: { name: "Renamed review" },
      }),
      {
        method: "DELETE",
        chatId: "chat-review",
        force: "false",
      },
      {
        method: "DELETE",
        chatId: "chat-running",
        force: "true",
      },
    ]),
  );
  expect(
    state.apiRequests.some((request) => request.path.includes("/stop")),
  ).toBe(false);
  expect(state.chats.map((chat) => chat.id)).toEqual(["chat-pinned"]);
});

test("Project Model, Reasoning, Options, and ACP use canonical controls without authority switching", async ({
  page,
}, testInfo) => {
  const state = await mockProjectChatApis(page);
  await page.addInitScript(() => {
    localStorage.setItem("rua-theme", "dark");
    localStorage.setItem("rua-skin", "pill");
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/projects/project-rua/chats/chat-running", {
    waitUntil: "domcontentloaded",
  });

  await page.getByText("Qwen Plus", { exact: true }).click();
  await expect(
    page.getByText(/Model and thinking|模型与思考/, { exact: true }),
  ).toBeVisible();
  const overlap = await page.evaluate(() => {
    const panel = document
      .querySelector("[data-rua-model-thinking-panel]")
      ?.getBoundingClientRect();
    const composer = document
      .querySelector(".rua-owned-composer")
      ?.getBoundingClientRect();
    return panel && composer
      ? {
          panelLeft: panel.left,
          panelRight: panel.right,
          panelTop: panel.top,
          panelBottom: panel.bottom,
          composerLeft: composer.left,
          composerRight: composer.right,
          composerTop: composer.top,
          composerBottom: composer.bottom,
        }
      : null;
  });
  expect(overlap).not.toBeNull();
  expect(overlap!.panelLeft).toBeLessThanOrEqual(overlap!.composerLeft);
  expect(overlap!.panelRight).toBeGreaterThanOrEqual(overlap!.composerRight);
  expect(overlap!.panelTop).toBeLessThanOrEqual(overlap!.composerTop);
  expect(overlap!.panelBottom).toBeGreaterThanOrEqual(
    overlap!.composerBottom - 1,
  );
  const overlayStyles = await page.evaluate(() => {
    const panel = document.querySelector<HTMLElement>(
      "[data-rua-model-thinking-panel] > div",
    );
    const composer = document.querySelector<HTMLElement>(
      ".rua-owned-composer",
    );
    if (!panel || !composer) return null;
    const panelStyle = getComputedStyle(panel);
    const composerStyle = getComputedStyle(composer);
    return {
      composerBoxShadow: composerStyle.boxShadow,
      composerBorderColor: composerStyle.borderColor,
      composerRadius: composerStyle.borderRadius,
      panelRadius: panelStyle.borderRadius,
    };
  });
  expect(overlayStyles).not.toBeNull();
  expect(overlayStyles!.composerBoxShadow).toBe("none");
  expect(overlayStyles!.composerBorderColor).toBe("rgba(0, 0, 0, 0)");
  expect(overlayStyles!.panelRadius).toBe(overlayStyles!.composerRadius);
  await expectNoHorizontalScroll(page);
  await page.screenshot({
    path: testInfo.outputPath("project-model-thinking-panel.png"),
    fullPage: false,
  });
  await page.getByText("DashScope", { exact: true }).click();
  await page.getByText("Qwen Max", { exact: true }).click();
  await expect
    .poll(() =>
      state.apiRequests.some(
        (request) =>
          request.method === "PATCH" &&
          request.path === "/projects/project-rua/chats/chat-running" &&
          JSON.stringify(request.body).includes('"model":"qwen-max"'),
      ),
    )
    .toBe(true);

  await page.getByRole("button", { name: /High|高/, exact: true }).click();
  await expect
    .poll(() =>
      state.apiRequests.some(
        (request) =>
          request.method === "PATCH" &&
          request.path === "/projects/project-rua/chats/chat-running" &&
          JSON.stringify(request.body).includes('"thinking_strength":"high"'),
      ),
    )
    .toBe(true);

  await page
    .getByRole("button", { name: /More Project actions|更多项目操作/ })
    .click();
  await expect(
    page.getByRole("menuitem", {
      name: /Execution device|Choose authority device|执行设备|选择.*设备/,
    }),
  ).toHaveCount(0);
  await page
    .getByRole("menuitem", {
      name: /Conversation options|对话选项/,
    })
    .click();
  await expect
    .poll(() =>
      state.apiRequests.some(
        (request) =>
          request.method === "GET" &&
          request.path === "/projects/project-rua/chats/chat-running/options",
      ),
    )
    .toBe(true);
  await page.getByRole("button", { name: /Save|保存/, exact: true }).click();
  await expect
    .poll(() =>
      state.apiRequests.some(
        (request) =>
          request.method === "PUT" &&
          request.path === "/projects/project-rua/chats/chat-running/options",
      ),
    )
    .toBe(true);

  await page
    .getByRole("button", { name: /More Project actions|更多项目操作/ })
    .click();
  await page.getByRole("menuitem", { name: /Enter ACP|进入 ACP/ }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: /Enter new session|New|新建/, exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText("codex");
  const enterRequest = state.apiRequests.find(
    (request) =>
      request.method === "POST" &&
      request.path === "/projects/project-rua/chats/chat-running/acp/enter",
  );
  expect(enterRequest).toBeDefined();
  expect(JSON.stringify(enterRequest?.body)).not.toContain("target_device_id");

  await page.getByRole("button", { name: /Exit ACP|退出 ACP/ }).click();
  await expect
    .poll(() =>
      state.apiRequests.some(
        (request) =>
          request.method === "POST" &&
          request.path ===
            "/projects/project-rua/chats/chat-running/acp/exit",
      ),
    )
    .toBe(true);
});
