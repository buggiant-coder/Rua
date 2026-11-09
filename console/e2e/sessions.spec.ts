import { test, expect } from "./fixtures";
import type { APIRequestContext } from "@playwright/test";

type ChatSpec = {
  id: string;
  name: string;
  session_id: string;
  user_id: string;
  channel: string;
  created_at: string;
  updated_at: string;
  status?: string;
};

async function createSession(
  request: APIRequestContext,
  base: string,
  input: { name: string; userId: string; channel: string },
): Promise<ChatSpec> {
  const response = await request.post(`${base}/api/agents/default/chats`, {
    data: {
      name: input.name,
      session_id: `console:${input.userId}`,
      user_id: input.userId,
      channel: input.channel,
      meta: { e2e: "sessions" },
    },
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as ChatSpec;
}

async function deleteSession(
  request: APIRequestContext,
  base: string,
  id: string,
): Promise<void> {
  await request.delete(
    `${base}/api/agents/default/chats/${encodeURIComponent(id)}`,
  );
}

test.describe("sessions", () => {
  test("chat-backed session API and page filters match the Rust console model", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const suffix = Date.now();
    const primaryName = `E2E Sessions Alpha ${suffix}`;
    const secondaryName = `E2E Sessions Beta ${suffix}`;
    const userId = `sessions-user-${suffix}`;
    const channel = `sessions-channel-${suffix}`;
    const created: ChatSpec[] = [];

    try {
      created.push(
        await createSession(request, base, {
          name: primaryName,
          userId,
          channel,
        }),
      );
      created.push(
        await createSession(request, base, {
          name: secondaryName,
          userId,
          channel,
        }),
      );

      const listResponse = await request.get(
        `${base}/api/agents/default/chats?user_id=${encodeURIComponent(
          userId,
        )}&channel=${encodeURIComponent(channel)}&limit=20`,
      );
      expect(listResponse.ok()).toBeTruthy();
      expect(Number(listResponse.headers()["x-total-count"] ?? "0")).toBe(2);

      const sessions = (await listResponse.json()) as ChatSpec[];
      expect(sessions.map((item) => item.name).sort()).toEqual([
        primaryName,
        secondaryName,
      ]);
      for (const session of sessions) {
        expect(session).toEqual(
          expect.objectContaining({
            id: expect.any(String),
            session_id: expect.any(String),
            user_id: userId,
            channel,
            created_at: expect.any(String),
            updated_at: expect.any(String),
          }),
        );
      }

      await page.goto("/sessions", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Sessions|会话/).first()).toBeVisible();
      await expect(page.getByText(primaryName).first()).toBeVisible();
      await expect(page.getByText(secondaryName).first()).toBeVisible();

      const search = page.getByPlaceholder(/Search title|搜索标题/);
      await search.fill(primaryName);
      await expect(page.getByText(primaryName).first()).toBeVisible();
      await expect(page.getByText(secondaryName).first()).toBeHidden();
    } finally {
      await Promise.all(
        created.map((session) => deleteSession(request, base, session.id)),
      );
    }
  });

  test("session action row opens the Rust edit drawer without relying on AntD tables", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const suffix = Date.now();
    const name = `E2E Sessions Editable ${suffix}`;
    const userId = `sessions-edit-user-${suffix}`;
    const channel = `sessions-edit-channel-${suffix}`;
    const created: ChatSpec[] = [];

    try {
      const session = await createSession(request, base, {
        name,
        userId,
        channel,
      });
      created.push(session);

      await page.goto("/sessions", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(name).first()).toBeVisible();

      const row = page
        .getByText(name)
        .first()
        .locator("xpath=ancestor::div[contains(@class, 'listItem')][1]");
      const actions = row.locator(
        "div[class*='listItemActions'] button.rua-btn",
      );
      await expect(actions).toHaveCount(2);
      await expect(actions.nth(1)).toBeEnabled();

      await actions.first().click();
      const drawer = page.getByRole("dialog");
      await expect(drawer.getByText(/Edit session|编辑会话/)).toBeVisible();
      await expect(
        drawer.getByPlaceholder(/Session name|会话名称/),
      ).toHaveValue(name);
      await expect(drawer.getByDisplayValue(session.id)).toBeVisible();
      await expect(drawer.getByDisplayValue(userId)).toBeVisible();
      await expect(drawer.getByDisplayValue(channel)).toBeVisible();

      await drawer.getByRole("button", { name: /Cancel|取消/ }).click();
      await expect(drawer).toBeHidden();
    } finally {
      await Promise.all(
        created.map((session) => deleteSession(request, base, session.id)),
      );
    }
  });

  test("batch selection mode exposes selectable Rust chat-backed sessions without deleting them", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const suffix = Date.now();
    const name = `E2E Sessions Batch ${suffix}`;
    const userId = `sessions-batch-user-${suffix}`;
    const channel = `sessions-batch-channel-${suffix}`;
    const created: ChatSpec[] = [];

    try {
      const session = await createSession(request, base, {
        name,
        userId,
        channel,
      });
      created.push(session);

      await page.goto("/sessions", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(name).first()).toBeVisible();

      await page.locator(".rua-ui-dropdown-trigger").first().click();
      await page
        .getByRole("menuitem", { name: /Batch delete|批量删除/ })
        .click();

      const row = page
        .getByText(name)
        .first()
        .locator("xpath=ancestor::div[contains(@class, 'listItem')][1]");
      const checkbox = row.locator("input.rua-check-input").first();
      await expect(checkbox).toBeVisible();
      await expect(checkbox).not.toBeChecked();

      await checkbox.check();
      await expect(checkbox).toBeChecked();

      await page
        .locator("button.rua-btn")
        .filter({ has: page.locator("svg") })
        .first()
        .click();
      await expect(checkbox).toBeHidden();

      const readback = await request.get(
        `${base}/api/agents/default/chats/${encodeURIComponent(session.id)}`,
      );
      expect(readback.ok()).toBeTruthy();
      const persisted = (await readback.json()) as ChatSpec;
      expect(persisted.name).toBe(name);
    } finally {
      await Promise.all(
        created.map((session) => deleteSession(request, base, session.id)),
      );
    }
  });
});
