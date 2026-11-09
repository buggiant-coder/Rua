import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";
import type { APIRequestContext, Page } from "@playwright/test";
import { test, expect } from "./fixtures";

type ChatSpec = {
  id: string;
  session_id: string;
};

type ChatSubmitBody = {
  session_id: string;
  input: Array<{ content: Array<{ text: string }> }>;
};

type SessionState = {
  agent?: {
    memory?: {
      content?: unknown[];
      turns?: Array<{ variants?: unknown[] }>;
      runtime_notes?: Array<{ summary?: string }>;
    };
  };
};

type CapturedFetchCall = { url: string; body: unknown };

declare global {
  interface Window {
    __ruaChatFetchCalls?: CapturedFetchCall[];
    __ruaCloseChatStream?: () => void;
    __ruaPushDelayedChatFrame?: (payload: unknown) => void;
    __ruaCloseDelayedChatStream?: () => void;
  }
}

const AGENT_ID = "default";

function workspaceDir(): string {
  if (process.env.RUA_WORKSPACE_DIR) return process.env.RUA_WORKSPACE_DIR;
  if (process.env.RUA_ROOT) {
    return path.join(process.env.RUA_ROOT, "workspaces", "default");
  }
  if (process.env.RUA_E2E_DIR) {
    return path.join(
      process.env.RUA_E2E_DIR,
      "working",
      "workspaces",
      "default",
    );
  }
  throw new Error(
    "chat-runtime-controls e2e needs RUA_E2E_DIR, RUA_ROOT, or RUA_WORKSPACE_DIR",
  );
}

function sessionPath(sessionId: string): string {
  return path.join(workspaceDir(), "sessions", `${AGENT_ID}_${sessionId}.json`);
}

function atomicWriteJson(filePath: string, value: unknown): void {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  const tmp = `${filePath}.${process.pid}.${Date.now()}.tmp`;
  const payload = `${JSON.stringify(value, null, 2)}\n`;
  const fd = fs.openSync(tmp, "w");
  try {
    fs.writeFileSync(fd, payload, "utf8");
    fs.fsyncSync(fd);
  } finally {
    fs.closeSync(fd);
  }
  fs.renameSync(tmp, filePath);
  const dirFd = fs.openSync(path.dirname(filePath), "r");
  try {
    fs.fsyncSync(dirFd);
  } finally {
    fs.closeSync(dirFd);
  }
}

function textMessage(
  id: string,
  role: "user" | "assistant",
  text: string,
  timestamp: string,
) {
  return {
    id,
    role,
    content: [{ type: "text", text }],
    name: role === "user" ? "user" : AGENT_ID,
    timestamp,
    device_id: "",
    metadata: {},
  };
}

function sessionStateWithVariants() {
  const branchTime = "2026-06-18T00:00:00.000Z";
  const laterTime = "2026-06-18T00:01:00.000Z";
  const user = textMessage(
    "user-1",
    "user",
    "Pick one generated answer",
    branchTime,
  );
  const first = textMessage(
    "assistant-1",
    "assistant",
    "First generated answer",
    branchTime,
  );
  const second = textMessage(
    "assistant-1",
    "assistant",
    "Second generated answer",
    branchTime,
  );
  const laterUser = textMessage(
    "user-2",
    "user",
    "Follow up that must be removed",
    laterTime,
  );
  const laterAssistant = textMessage(
    "assistant-2",
    "assistant",
    "Later assistant that must be removed",
    laterTime,
  );

  return {
    agent: {
      name: AGENT_ID,
      _sys_prompt: "Seeded e2e prompt snapshot",
      _active_mode_names: ["e2e-mode"],
      _active_prompt_files: [
        {
          id: "prompt-1",
          name: "E2E prompt",
          enabled: true,
          content_hash: "sha256:e2e",
        },
      ],
      _pending_action: null,
      toolkit: { active_groups: null },
      memory: {
        _compressed_summary: "",
        content: [
          [user, []],
          [first, []],
          [laterUser, []],
          [laterAssistant, []],
        ],
        turns: [
          {
            turn_id: "turn-1",
            user_message_id: "user-1",
            assistant_message_id: "assistant-1",
            selected_variant_id: "variant-1",
            variants: [
              {
                variant_id: "variant-1",
                message: first,
                created_at: branchTime,
              },
              {
                variant_id: "variant-2",
                message: second,
                created_at: branchTime,
              },
            ],
            committed: false,
          },
        ],
        runtime_notes: [],
      },
    },
    metadata: {},
  };
}

function sessionStateFromMessages(
  content: Array<[ReturnType<typeof textMessage>, unknown[]]>,
  turns: unknown[] = [],
  runtimeNotes: unknown[] = [],
) {
  return {
    agent: {
      name: AGENT_ID,
      _sys_prompt: "Seeded e2e prompt snapshot",
      _active_mode_names: ["e2e-mode"],
      _active_prompt_files: [],
      _pending_action: null,
      toolkit: { active_groups: null },
      memory: {
        _compressed_summary: "",
        content,
        turns,
        runtime_notes: runtimeNotes,
      },
    },
    metadata: {},
  };
}

async function createChat(
  request: APIRequestContext,
  baseURL: string | undefined,
): Promise<ChatSpec> {
  const base = baseURL ?? "http://127.0.0.1:8090";
  const response = await request.post(`${base}/api/agents/${AGENT_ID}/chats`, {
    data: {
      name: `Runtime controls ${randomUUID()}`,
      user_id: "default",
      channel: "console",
      meta: {},
    },
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as ChatSpec;
}

function readSession(sessionId: string): SessionState {
  return JSON.parse(
    fs.readFileSync(sessionPath(sessionId), "utf8"),
  ) as SessionState;
}

async function seedVariantSession(chat: ChatSpec): Promise<void> {
  atomicWriteJson(sessionPath(chat.session_id), sessionStateWithVariants());
}

async function submitOwnedPrompt(page: Page, text: string): Promise<void> {
  const input = page.locator(".rua-owned-composer textarea").first();
  await expect(input).toBeVisible();
  await input.fill(text);
  await input.press("Enter");
}

async function mockConsoleChatStream(
  page: Page,
  chat: ChatSpec,
  answer: string,
  persist: () => void,
): Promise<{ body: unknown | null }> {
  const captured: { body: unknown | null } = { body: null };
  await page.route(
    "**/api/agents/default/chats/*/console/submit",
    async (route) => {
      captured.body = route.request().postDataJSON();
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ accepted: true, mode: "local" }),
      });
    },
  );
  await page.route(
    "**/api/agents/default/chats/*/console/live",
    async (route) => {
      persist();
      const output = [
        {
          id: "assistant-stream",
          role: "assistant",
          type: "message",
          content: [{ type: "text", text: answer }],
        },
      ];
      await route.fulfill({
        status: 200,
        headers: { "Content-Type": "text/event-stream" },
        body:
          `data: ${JSON.stringify({
            object: "response",
            status: "in_progress",
            output,
          })}\n\n` +
          `data: ${JSON.stringify({
            object: "response",
            status: "completed",
            output,
          })}\n\n`,
      });
    },
  );
  void chat;
  return captured;
}

async function installBrowserStreamingFetchMock(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const originalFetch = window.fetch.bind(window);
    const encoder = new TextEncoder();
    const calls: Array<{ url: string; body: unknown }> = [];
    let streamController: ReadableStreamDefaultController<Uint8Array> | null =
      null;

    window.__ruaChatFetchCalls = calls;
    window.__ruaCloseChatStream = () => {
      streamController?.close();
      streamController = null;
    };

    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url =
        typeof input === "string"
          ? input
          : input instanceof URL
          ? input.toString()
          : input.url;
      const bodyText = typeof init?.body === "string" ? init.body : "";
      const body = bodyText ? JSON.parse(bodyText) : null;

      if (
        url.includes("/api/agents/default/chats/") &&
        url.includes("/console/stop")
      ) {
        calls.push({ url, body });
        streamController?.close();
        streamController = null;
        return new Response(null, { status: 204 });
      }

      if (
        url.includes("/api/agents/default/chats/") &&
        url.includes("/console/submit")
      ) {
        calls.push({ url, body });
        return new Response(JSON.stringify({ accepted: true, mode: "local" }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }

      if (
        url.includes("/api/agents/default/chats/") &&
        url.includes("/console/live")
      ) {
        calls.push({ url, body });
        const stream = new ReadableStream<Uint8Array>({
          start(controller) {
            streamController = controller;
            controller.enqueue(
              encoder.encode(
                `data: ${JSON.stringify({
                  object: "response",
                  status: "in_progress",
                  output: [
                    {
                      id: "tool-call-e2e",
                      role: "assistant",
                      type: "plugin_call",
                      content: [{ type: "tool", data: { name: "Bash" } }],
                    },
                  ],
                })}\n\n`,
              ),
            );
          },
        });
        return new Response(stream, {
          status: 200,
          headers: { "Content-Type": "text/event-stream" },
        });
      }

      return originalFetch(input, init);
    };
  });
}

async function installDelayedChatStreamMock(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const originalFetch = window.fetch.bind(window);
    const encoder = new TextEncoder();
    let streamController: ReadableStreamDefaultController<Uint8Array> | null =
      null;

    window.__ruaPushDelayedChatFrame = (payload: unknown) => {
      streamController?.enqueue(
        encoder.encode(`data: ${JSON.stringify(payload)}\n\n`),
      );
    };
    window.__ruaCloseDelayedChatStream = () => {
      streamController?.close();
      streamController = null;
    };

    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url =
        typeof input === "string"
          ? input
          : input instanceof URL
          ? input.toString()
          : input.url;

      if (
        url.includes("/api/agents/default/chats/") &&
        url.includes("/console/submit")
      ) {
        return new Response(JSON.stringify({ accepted: true, mode: "local" }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }

      if (
        url.includes("/api/agents/default/chats/") &&
        url.includes("/console/live")
      ) {
        const stream = new ReadableStream<Uint8Array>({
          start(controller) {
            streamController = controller;
            init?.signal?.addEventListener("abort", () => {
              try {
                controller.close();
              } catch {}
              streamController = null;
            });
          },
        });
        return new Response(stream, {
          status: 200,
          headers: { "Content-Type": "text/event-stream" },
        });
      }

      return originalFetch(input, init);
    };
  });
}

test.describe("chat runtime controls", () => {
  test("submit renders completion without refresh and persists to session file", async ({
    page,
    request,
    baseURL,
  }) => {
    const chat = await createChat(request, baseURL);
    const now = "2026-06-18T00:02:00.000Z";
    const submittedText = "E2E submit without refresh";
    const answerText = "E2E streamed answer persisted";
    const captured = await mockConsoleChatStream(page, chat, answerText, () => {
      const user = textMessage("user-submit-1", "user", submittedText, now);
      const assistant = textMessage(
        "assistant-submit-1",
        "assistant",
        answerText,
        now,
      );
      atomicWriteJson(
        sessionPath(chat.session_id),
        sessionStateFromMessages([
          [user, []],
          [assistant, []],
        ]),
      );
    });

    await page.goto(`/agents/default/chats/${chat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await submitOwnedPrompt(page, submittedText);

    await expect(page.getByText(answerText)).toBeVisible();
    await expect(page.getByText("Generating")).toHaveCount(0);
    const submitBody = captured.body as ChatSubmitBody;
    expect(submitBody.session_id).toBe(chat.id);
    expect(submitBody.input[0].content[0].text).toBe(submittedText);

    await page.reload({ waitUntil: "domcontentloaded" });
    await expect(page.getByText(answerText)).toBeVisible();
    expect(readSession(chat.session_id).agent.memory.content).toHaveLength(2);
  });

  test("regenerate becomes an in-place assistant variant without refresh", async ({
    page,
    request,
    baseURL,
  }) => {
    const chat = await createChat(request, baseURL);
    const now = "2026-06-18T00:03:00.000Z";
    const user = textMessage("user-regen-1", "user", "Regenerate me", now);
    const first = textMessage(
      "assistant-regen-1",
      "assistant",
      "First answer",
      now,
    );
    atomicWriteJson(
      sessionPath(chat.session_id),
      sessionStateFromMessages(
        [
          [user, []],
          [first, []],
        ],
        [
          {
            turn_id: "turn-regen-1",
            user_message_id: "user-regen-1",
            assistant_message_id: "assistant-regen-1",
            selected_variant_id: "variant-regen-1",
            variants: [
              {
                variant_id: "variant-regen-1",
                message: first,
                created_at: now,
              },
            ],
            committed: false,
          },
        ],
      ),
    );

    const captured = await mockConsoleChatStream(
      page,
      chat,
      "Second answer",
      () => {
        const second = textMessage(
          "assistant-regen-1",
          "assistant",
          "Second answer",
          now,
        );
        atomicWriteJson(
          sessionPath(chat.session_id),
          sessionStateFromMessages(
            [
              [user, []],
              [second, []],
            ],
            [
              {
                turn_id: "turn-regen-1",
                user_message_id: "user-regen-1",
                assistant_message_id: "assistant-regen-1",
                selected_variant_id: "variant-regen-2",
                variants: [
                  {
                    variant_id: "variant-regen-1",
                    message: first,
                    created_at: now,
                  },
                  {
                    variant_id: "variant-regen-2",
                    message: second,
                    created_at: now,
                  },
                ],
                committed: false,
              },
            ],
          ),
        );
      },
    );

    await page.goto(`/agents/default/chats/${chat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await expect(page.getByText("First answer")).toBeVisible();
    await page.getByLabel("Regenerate").click();

    await expect(page.getByText("Second answer")).toBeVisible();
    await expect(page.getByText("2/2")).toBeVisible();
    await expect(page.getByText("Regenerate me")).toHaveCount(1);
    await expect(page.getByText("Generating")).toHaveCount(0);
    const submitBody = captured.body as ChatSubmitBody;
    expect(submitBody.session_id).toBe(chat.id);
    expect(submitBody.input[0].content[0].text).toBe("Regenerate me");
    expect(
      readSession(chat.session_id).agent.memory.turns[0].variants,
    ).toHaveLength(2);
  });

  test("response variants page in place and selected variant is persisted", async ({
    page,
    request,
    baseURL,
  }) => {
    const chat = await createChat(request, baseURL);
    await seedVariantSession(chat);

    await page.goto(`/agents/default/chats/${chat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await expect(page.getByText("First generated answer")).toBeVisible();
    await expect(page.getByText("1/2")).toBeVisible();

    await page.getByLabel("Next variant").click();
    await expect(page.getByText("Second generated answer")).toBeVisible();
    await expect(page.getByText("2/2")).toBeVisible();

    await expect
      .poll(
        () =>
          readSession(chat.session_id).agent.memory.turns[0]
            .selected_variant_id,
      )
      .toBe("variant-2");
    expect(
      readSession(chat.session_id).agent.memory.content[1][0].content[0].text,
    ).toBe("Second generated answer");
  });

  test("rollback commits the selected variant and removes later conversation state", async ({
    page,
    request,
    baseURL,
  }) => {
    const chat = await createChat(request, baseURL);
    await seedVariantSession(chat);

    await page.goto(`/agents/default/chats/${chat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await page.getByLabel("Next variant").click();
    await expect(page.getByText("Second generated answer")).toBeVisible();

    await page.getByLabel("More response actions").click();
    await page.getByRole("menuitem", { name: /Rollback/ }).click();

    await expect(page.getByText("Second generated answer")).toBeVisible();
    await expect(
      page.getByText("Later assistant that must be removed"),
    ).toHaveCount(0);

    await expect
      .poll(() => readSession(chat.session_id).agent.memory.content.length)
      .toBe(2);
    const saved = readSession(chat.session_id);
    expect(saved.agent.memory.content[1][0].content[0].text).toBe(
      "Second generated answer",
    );
    expect(saved.agent.memory.turns[0].committed).toBe(true);
    expect(saved.agent.memory.turns[0].variants).toHaveLength(1);
  });

  test("branch creates a new chat with selected history and leaves source chat intact", async ({
    page,
    request,
    baseURL,
  }) => {
    const chat = await createChat(request, baseURL);
    await seedVariantSession(chat);

    await page.goto(`/agents/default/chats/${chat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await page.getByLabel("Next variant").click();
    await expect(page.getByText("Second generated answer")).toBeVisible();

    await page.getByLabel("More response actions").click();
    await page.getByRole("menuitem", { name: /Branch/ }).click();

    await page.waitForURL(
      (url) =>
        /\/chat\/[^/]+/.test(url.pathname) && !url.pathname.endsWith(chat.id),
    );
    const branchId = page.url().match(/\/chat\/([^/?#]+)/)?.[1];
    expect(branchId).toBeTruthy();
    await expect(page.getByText("Second generated answer")).toBeVisible();
    await expect(
      page.getByText("Later assistant that must be removed"),
    ).toHaveCount(0);

    const base = baseURL ?? "http://127.0.0.1:8090";
    const branchResponse = await request.get(
      `${base}/api/agents/${AGENT_ID}/chats/${branchId}`,
    );
    expect(branchResponse.ok()).toBeTruthy();
    const branchHistory = await branchResponse.json();
    expect(branchHistory.messages).toHaveLength(2);
    const branchSession = readSession(branchId!);
    expect(branchSession.agent.memory.content).toHaveLength(2);
    expect(branchSession.agent.memory.content[1][0].content[0].text).toBe(
      "Second generated answer",
    );

    const sourceSession = readSession(chat.session_id);
    expect(sourceSession.agent.memory.content).toHaveLength(4);
  });

  test("stop ends an in-flight stream through the active chat id", async ({
    page,
    request,
    baseURL,
  }) => {
    const chat = await createChat(request, baseURL);
    await installBrowserStreamingFetchMock(page);

    await page.goto(`/agents/default/chats/${chat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await submitOwnedPrompt(page, "Stop this slow turn");

    await page.getByLabel("Stop generating").click();

    await expect
      .poll(() =>
        page.evaluate(() => {
          const activeChatId = location.pathname.split("/").pop() || "";
          return (window.__ruaChatFetchCalls || []).some((call) => {
            const parsed = new URL(String(call.url), location.origin);
            return (
              parsed.pathname ===
              `/api/agents/default/chats/${activeChatId}/console/stop`
            );
          });
        }),
      )
      .toBe(true);
    await expect(page.getByLabel("Send message")).toBeVisible();
  });

  test("deleting the active chat from the owned drawer navigates to a fresh chat", async ({
    page,
    request,
    baseURL,
  }) => {
    const chat = await createChat(request, baseURL);
    atomicWriteJson(
      sessionPath(chat.session_id),
      sessionStateFromMessages([
        [
          textMessage(
            "user-delete-1",
            "user",
            "Chat that will be deleted",
            "2026-06-18T00:04:00.000Z",
          ),
          [],
        ],
      ]),
    );

    page.on("dialog", (dialog) => dialog.accept());

    await page.goto(`/agents/default/chats/${chat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await expect(page.getByText("Chat that will be deleted")).toBeVisible();
    await page.getByRole("button", { name: "Chats", exact: true }).click();
    await expect(
      page.locator(`[data-rua-chat-session-id="${chat.id}"]`),
    ).toBeVisible();

    await page
      .locator(`[data-rua-chat-session-id="${chat.id}"]`)
      .getByLabel("Delete chat")
      .click();

    await page.waitForURL(
      (url) =>
        /\/chat\/[^/]+/.test(url.pathname) && !url.pathname.endsWith(chat.id),
    );
    await expect(page.getByText("Start a conversation")).toBeVisible();

    const base = baseURL ?? "http://127.0.0.1:8090";
    const listResponse = await request.get(
      `${base}/api/agents/${AGENT_ID}/chats`,
    );
    expect(listResponse.ok()).toBeTruthy();
    const chats = await listResponse.json();
    expect(chats.some((item: ChatSpec) => item.id === chat.id)).toBe(false);
  });

  test("late stream frames from a previous route do not render in the active chat", async ({
    page,
    request,
    baseURL,
  }) => {
    const firstChat = await createChat(request, baseURL);
    const secondChat = await createChat(request, baseURL);
    await installDelayedChatStreamMock(page);

    await page.goto(`/agents/default/chats/${firstChat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await submitOwnedPrompt(page, "Start a delayed stream");
    await expect(page.getByText("Start a delayed stream")).toBeVisible();

    await page.goto(`/agents/default/chats/${secondChat.id}`, {
      waitUntil: "domcontentloaded",
    });
    await expect(page.getByText("Start a conversation")).toBeVisible();

    await page.evaluate(
      () =>
        window.__ruaPushDelayedChatFrame?.({
          object: "response",
          status: "completed",
          output: [
            {
              id: "assistant-stale",
              role: "assistant",
              type: "message",
              content: [{ type: "text", text: "stale delayed answer" }],
            },
          ],
        }),
    );

    await expect(page.getByText("stale delayed answer")).toHaveCount(0);
    await expect(page.getByText("Start a conversation")).toBeVisible();
  });

  test("completed background task runtime notes render from persisted session state", async ({
    page,
    request,
    baseURL,
  }) => {
    const chat = await createChat(request, baseURL);
    atomicWriteJson(
      sessionPath(chat.session_id),
      sessionStateFromMessages(
        [
          [
            textMessage(
              "user-runtime-note-1",
              "user",
              "Run background task",
              "2026-06-18T00:05:00.000Z",
            ),
            [],
          ],
        ],
        [],
        [
          {
            runtime_note_id: "runtime-note-e2e",
            kind: "background_tool_task_terminal",
            task_id: "task-runtime-note-e2e",
            tool_name: "Bash",
            terminal_status: "completed",
            summary: "rua-background-e2e completed",
            result_ref: {
              ref_type: "background_task",
              task_id: "task-runtime-note-e2e",
              tool_call_id: "tool-call-runtime-note-e2e",
            },
          },
        ],
      ),
    );

    await page.goto(`/agents/default/chats/${chat.id}`, {
      waitUntil: "domcontentloaded",
    });

    await expect(page.getByText("Run background task")).toBeVisible();
    await expect(page.getByText("Bash")).toBeVisible();
    await expect(page.getByText("completed", { exact: true })).toBeVisible();
    await expect(
      page.getByText("rua-background-e2e completed"),
    ).toBeVisible();
  });
});
