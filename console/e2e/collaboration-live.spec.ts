import { expect, test } from "@playwright/test";

const chats: string[] = JSON.parse(
  process.env.RUA_COLLABORATION_CHATS ?? "[]",
);
const turns = JSON.parse(process.env.RUA_COLLABORATION_TURNS ?? "[]");
const sources: string[] = JSON.parse(
  process.env.RUA_BACKGROUND_SOURCES ?? "[]",
);

test("registered tool Task is visible through the actual scoped HTTP API", async ({
  request,
}) => {
  const task = JSON.parse(process.env.RUA_LIVE_TOOL_TASK!);
  const prefix = `/api/agents/default/chats/${task.chat_id}/tasks`;
  const listed = await request.get(prefix);
  expect(listed.ok()).toBeTruthy();
  expect(
    (await listed.json()).some(
      (item: { task_id: string }) => item.task_id === task.task_id,
    ),
  ).toBeTruthy();
  const detail = await request.get(`${prefix}/${task.task_id}`);
  expect(detail.ok()).toBeTruthy();
  expect((await detail.json()).state).toBe("failed");
  const wrongOwner = await request.get(
    `/api/agents/default/chats/other-chat/tasks/${task.task_id}`,
  );
  expect(wrongOwner.ok()).toBeFalsy();
});

for (const [index, width] of [390, 1440].entries()) {
  test(`idle source shows durable background waiting at ${width}px`, async ({
    page,
  }) => {
    await page.addInitScript(() => localStorage.setItem("language", "en"));
    await page.setViewportSize({ width, height: 844 });
    await page.goto(
      `/e2e/fixtures/owned-composer.html?agent=default&chat=${encodeURIComponent(
        sources[index],
      )}`,
    );
    const waiting = page.locator('[data-rua-background-work="waiting"]');
    await expect(waiting).toBeVisible();
    expect((await waiting.boundingBox())!.height).toBeLessThanOrEqual(36);
    await expect(waiting.getByRole("list")).toHaveCount(0);
    await expect(page.getByLabel("Stop generating")).toHaveCount(0);
    await page.reload();
    await expect(waiting).toBeVisible();
    const stopped = page.waitForResponse((response) =>
      response.url().endsWith(`/chats/${sources[index]}/console/stop`),
    );
    await page
      .getByRole("button", { name: "Stop automatic continuation" })
      .click();
    const response = await stopped;
    expect(response.ok()).toBeTruthy();
    expect(response.request().postDataJSON()).not.toHaveProperty("turn_id");
    await expect(page.getByText(/Auto-resume stopped/)).toBeVisible();
  });
  test(`actual collaboration service renders and stops at ${width}px`, async ({
    page,
  }, testInfo) => {
    expect(chats).toHaveLength(2);
    await page.addInitScript(() => localStorage.setItem("language", "en"));
    const chat = chats[index];
    await page.setViewportSize({ width, height: 844 });
    await page.goto(
      `/e2e/fixtures/owned-composer.html?agent=default&chat=${encodeURIComponent(
        chat,
      )}`,
    );
    const relation = page.locator(".rua-owned-collaboration-relationship");
    await expect(relation).toBeVisible();
    await expect(relation).not.toContainText("chat.collaborationExecutorAgent");
    expect((await relation.boundingBox())!.height).toBeLessThan(100);
    expect(
      (await page.locator(".rua-owned-surface-messages").boundingBox())!
        .height,
    ).toBeGreaterThan(300);
    const input = page.locator(".rua-owned-composer textarea");
    await input.fill("Direct participation remains available");
    await expect(input).toHaveValue("Direct participation remains available");
    await input.fill("");
    const stopped = page.waitForResponse((response) =>
      response.url().endsWith(`/chats/${chat}/console/stop`),
    );
    await page.getByLabel("Stop generating").click();
    const response = await stopped;
    expect(response.request().postDataJSON()).toEqual(turns[index]);
    expect(response.ok()).toBeTruthy();
    expect((await response.json()).status).toBe("stop_requested");
    await expect(page.getByLabel("Stop generating")).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath(`live-${width}.png`) });
  });
}
