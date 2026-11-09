import { test, expect, goChat, submitPrompt } from "./fixtures";
import type { APIRequestContext, Page } from "@playwright/test";

type ChatSpec = {
  id: string;
  name: string;
  session_id: string;
};

async function createChat(
  request: APIRequestContext,
  base: string,
  name: string,
): Promise<ChatSpec> {
  const response = await request.post(`${base}/api/agents/default/chats`, {
    data: {
      name,
      session_id: `console:${name}`,
      user_id: `e2e-${Date.now()}`,
      channel: "console",
      meta: {},
    },
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as ChatSpec;
}

function completedChatLiveStream(answer: string): string {
  return [
    {
      object: "message",
      id: "msg-e2e",
      role: "assistant",
      status: "in_progress",
      content: [{ type: "text", text: answer }],
    },
    {
      object: "message",
      id: "msg-e2e",
      role: "assistant",
      status: "completed",
      content: [{ type: "text", text: answer }],
    },
    {
      object: "response",
      id: "resp-e2e",
      status: "completed",
      finish_reason: "stop",
    },
  ]
    .map((payload) => `data: ${JSON.stringify(payload)}\n\n`)
    .join("");
}

async function mockChatStream(
  page: Page,
  answerForCall: (callIndex: number) => string,
): Promise<{ bodies: unknown[] }> {
  let calls = 0;
  const bodies: unknown[] = [];
  await page.route(
    "**/api/agents/default/chats/*/console/submit",
    async (route) => {
      bodies.push(route.request().postDataJSON());
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
      calls += 1;
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: completedChatLiveStream(answerForCall(calls)),
      });
    },
  );
  return { bodies };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((innerResolve) => {
    resolve = innerResolve;
  });
  return { promise, resolve };
}

test.describe("chat", () => {
  test("history and search drawers mount without real LLM credentials", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const chatName = `E2E Chat History ${Date.now()}`;
    const chat = await createChat(request, base, chatName);

    try {
      await goChat(page);

      await page.getByRole("button", { name: /Chat history|聊天历史/ }).click();
      await expect(
        page.getByRole("heading", { name: /All Chats|全部聊天/ }),
      ).toBeVisible();
      await expect(page.getByText(chatName).first()).toBeVisible();

      await page.getByRole("button", { name: /Close|关闭/ }).click();
      await page.getByRole("button", { name: /Actions|操作/ }).click();
      await page.getByText(/Search Chats|搜索对话/).click();
      await expect(
        page.getByRole("heading", { name: /Search Chats|搜索对话/ }),
      ).toBeVisible();

      const search = page.getByPlaceholder(/Search conversations|搜索对话/);
      await expect(search).toBeVisible();
      await search.fill(`no-hit-${chatName}`);
      await expect(page.getByText(/No results found|未找到结果/)).toBeVisible();
    } finally {
      await request.delete(
        `${base}/api/agents/default/chats/${encodeURIComponent(chat.id)}`,
      );
    }
  });

  test("send and regenerate render mocked Rust JSON-lines responses", async ({
    page,
  }) => {
    const stream = await mockChatStream(
      page,
      (call) => `Mock assistant response ${call}`,
    );

    await goChat(page);
    await submitPrompt(page, "hello from chat e2e");

    await expect(page.getByText("hello from chat e2e")).toBeVisible();
    await expect(page.getByText("Mock assistant response 1")).toBeVisible();
    await expect.poll(() => stream.bodies.length, { timeout: 15_000 }).toBe(1);
    expect(JSON.stringify(stream.bodies[0])).toContain("hello from chat e2e");

    await page.getByTitle(/Regenerate|重新生成/).click();

    await expect(page.getByText("Mock assistant response 2")).toBeVisible();
    await expect.poll(() => stream.bodies.length, { timeout: 15_000 }).toBe(2);
  });

  test("message spacing follows visible content and branch actions live on user messages", async ({
    page,
    request,
    baseURL,
  }, testInfo) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const chat = await createChat(
      request,
      base,
      `E2E visual message layout ${Date.now()}`,
    );
    const messages = [
      {
        id: "user-visual-1",
        role: "user",
        turn_id: "turn-visual-1",
        content: "First visual prompt",
        timestamp: "2026-08-05T04:00:00Z",
      },
      {
        id: "assistant-visual-2",
        role: "assistant",
        type: "message",
        turn_id: "turn-visual-1",
        selected_variant_id: "variant-visual-2",
        content: "Regenerated visual response",
        started_at: "2026-08-05T04:00:01Z",
        finished_at: "2026-08-05T04:00:03Z",
        assistant_response_variants: [
          {
            variant_id: "variant-visual-1",
            output: [
              {
                id: "assistant-visual-1",
                role: "assistant",
                type: "message",
                content: "Original visual response",
              },
            ],
          },
          {
            variant_id: "variant-visual-2",
            output: [
              {
                id: "assistant-visual-2",
                role: "assistant",
                type: "message",
                content: "Regenerated visual response",
              },
            ],
          },
        ],
      },
      {
        id: "user-visual-2",
        role: "user",
        turn_id: "turn-visual-2",
        content: "Second visual prompt",
        timestamp: "2026-08-05T04:01:00Z",
      },
      {
        id: "assistant-visual-3",
        role: "assistant",
        type: "message",
        turn_id: "turn-visual-2",
        selected_variant_id: "variant-visual-3",
        content: "Second visual response",
        started_at: "2026-08-05T04:01:01Z",
        finished_at: "2026-08-05T04:01:02Z",
      },
    ];

    await page.route("**/api/agents/*/chats/**", async (route) => {
      const url = new URL(route.request().url());
      if (
        route.request().method() === "GET" &&
        url.pathname.endsWith(`/chats/${chat.id}`)
      ) {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            messages,
            status: "idle",
            title: chat.name,
          }),
        });
        return;
      }
      await route.continue();
    });

    try {
      await page.goto(`/agents/default/chats/${chat.id}`, {
        waitUntil: "domcontentloaded",
      });
      await expect(page.getByText("Regenerated visual response")).toBeVisible();

      const userMessages = page.locator(
        '[data-rua-chat-role="user"].rua-chat-message',
      );
      await expect(userMessages).toHaveCount(2);
      const firstUser = userMessages.nth(0);
      const secondUser = userMessages.nth(1);
      const firstAssistantText = page.getByText("Regenerated visual response");
      const firstAssistant = firstAssistantText.locator(
        "xpath=ancestor::*[@data-rua-chat-role='assistant'][1]",
      );
      const firstUserText = firstUser.getByText("First visual prompt", {
        exact: true,
      });
      const secondUserText = secondUser.getByText("Second visual prompt", {
        exact: true,
      });

      const measureGapDifference = async () => {
        const [firstUserBox, firstAssistantBox, secondUserBox] =
          await Promise.all([
            firstUserText.boundingBox(),
            firstAssistantText.boundingBox(),
            secondUserText.boundingBox(),
          ]);
        expect(firstUserBox).not.toBeNull();
        expect(firstAssistantBox).not.toBeNull();
        expect(secondUserBox).not.toBeNull();
        const userToAssistantGap =
          firstAssistantBox!.y - (firstUserBox!.y + firstUserBox!.height);
        const assistantToUserGap =
          secondUserBox!.y - (firstAssistantBox!.y + firstAssistantBox!.height);
        return Math.abs(userToAssistantGap - assistantToUserGap);
      };

      expect(await measureGapDifference()).toBeLessThanOrEqual(8);

      await firstAssistant.hover();
      const assistantTimes = firstAssistant.locator(".rua-chat-time");
      await expect(assistantTimes).toHaveCount(2);
      for (const time of await assistantTimes.all()) {
        await expect(time).toHaveCSS("opacity", "1");
      }

      for (const userMessage of [firstUser, secondUser]) {
        await userMessage.hover();
        const actions = userMessage.getByTitle(
          /More message actions|更多消息操作/,
        );
        await expect(actions).toBeVisible();
        await actions.click();
        await expect(
          page.getByRole("menuitem", { name: /Rollback|回滚/ }),
        ).toBeVisible();
        await page
          .getByRole("menuitem", { name: /Rollback|回滚/ })
          .click({ trial: true });
        await page
          .getByRole("menuitem", { name: /Branch|分支/ })
          .click({ trial: true });
        await expect(
          page.getByRole("menuitem", { name: /Branch|分支/ }),
        ).toBeVisible();
        await actions.click();
      }

      await page.screenshot({
        path: testInfo.outputPath("chat-message-spacing-and-user-actions.png"),
        fullPage: false,
      });

      await page.setViewportSize({ width: 390, height: 844 });
      await firstUser.scrollIntoViewIfNeeded();
      expect(await measureGapDifference()).toBeLessThanOrEqual(8);
      const widths = await page.evaluate(() => ({
        document: document.documentElement.scrollWidth,
        viewport: document.documentElement.clientWidth,
      }));
      expect(widths.document).toBeLessThanOrEqual(widths.viewport + 1);

      await secondUser.click();
      const mobileActions = secondUser.getByTitle(
        /More message actions|更多消息操作/,
      );
      await expect(mobileActions).toBeVisible();
      await mobileActions.click();
      await expect(
        page.getByRole("menuitem", { name: /Rollback|回滚/ }),
      ).toBeVisible();
      await page
        .getByRole("menuitem", { name: /Rollback|回滚/ })
        .click({ trial: true });
      await page
        .getByRole("menuitem", { name: /Branch|分支/ })
        .click({ trial: true });
      await expect(
        page.getByRole("menuitem", { name: /Branch|分支/ }),
      ).toBeVisible();
      await page.screenshot({
        path: testInfo.outputPath(
          "chat-message-spacing-and-user-actions-mobile.png",
        ),
        fullPage: false,
      });
      await mobileActions.click();
    } finally {
      await request.delete(
        `${base}/api/agents/default/chats/${encodeURIComponent(chat.id)}`,
      );
    }
  });

  test("stop sends the Rust console stop request while generation is pending", async ({
    page,
  }) => {
    const unblock = deferred<void>();

    await page.route(
      "**/api/agents/default/chats/*/console/submit",
      async (route) => {
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
        await unblock.promise;
        await route.fulfill({
          status: 200,
          contentType: "text/event-stream",
          body: completedChatLiveStream("response after stop"),
        });
      },
    );
    await page.route(
      /\/api\/agents\/default\/chats\/[^/]+\/console\/stop$/,
      async (route) => {
        unblock.resolve();
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ stopped: true }),
        });
      },
    );

    await goChat(page);
    await submitPrompt(page, "please stop this response");

    const stopRequest = page.waitForRequest((request) => {
      const url = request.url();
      return (
        request.method() === "POST" &&
        /\/api\/agents\/default\/chats\/[^/]+\/console\/stop$/.test(url)
      );
    });

    await page
      .locator(
        'button.rua-sender-actions-btn-loading-button, button[class*="sender-actions-btn-loading-button"]',
      )
      .click();

    await expect
      .poll(async () => (await stopRequest).url(), { timeout: 15_000 })
      .toMatch(/\/api\/agents\/default\/chats\/[^/]+\/console\/stop$/);
  });
});
