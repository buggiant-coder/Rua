import { test, expect } from "./fixtures";

type CronJob = {
  id: string;
  name: string;
  enabled?: boolean;
  text?: string;
};

function cronPayload(name: string, text: string) {
  return {
    name,
    enabled: true,
    schedule: {
      type: "cron",
      cron: "0 9 * * *",
      timezone: "UTC",
    },
    task_type: "text",
    text,
    dispatch: {
      type: "channel",
      channel: "console",
      target: {
        user_id: "e2e-cron-user",
        session_id: "console:e2e-cron-session",
      },
      mode: "stream",
    },
    runtime: {
      max_concurrency: 1,
      timeout_seconds: 120,
      misfire_grace_seconds: 60,
    },
  };
}

test.describe("cron jobs", () => {
  test("cron API and page empty state are wired", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const response = await request.get(`${base}/api/agents/default/cron/jobs`);
    expect(response.ok()).toBeTruthy();

    const jobs = (await response.json()) as CronJob[];
    expect(Array.isArray(jobs)).toBeTruthy();

    await page.goto("/cron-jobs", { waitUntil: "domcontentloaded" });
    await expect(page.getByText(/Cron Jobs|定时任务/).first()).toBeVisible();

    const headerButtons = page
      .locator(".rua-btn")
      .filter({ has: page.locator("svg") });
    await expect(headerButtons.first()).toBeVisible();
    await expect(headerButtons.nth(1)).toBeVisible();
    await headerButtons.first().click();

    await expect(
      page.getByText(/No cron jobs yet|暂无数据|定时任务/).first(),
    ).toBeVisible();
  });

  test("seeded job can be searched, toggled, run, and deleted", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const name = `e2e_cron_lifecycle_${Date.now()}`;
    const text = "E2E cron lifecycle text";
    const create = await request.post(`${base}/api/agents/default/cron/jobs`, {
      data: cronPayload(name, text),
    });
    expect(create.status()).toBe(201);
    const job = (await create.json()) as CronJob;

    let deleted = false;
    try {
      await page.route(
        new RegExp(`/api/agents/default/cron/jobs/${job.id}/run$`),
        async (route) => {
          await route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({ success: true, message: "Job completed" }),
          });
        },
      );

      await page.goto("/cron-jobs", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Cron Jobs|定时任务/).first()).toBeVisible();

      const search = page.getByPlaceholder(/Search jobs|搜索任务|搜索/).first();
      await expect(search).toBeVisible();
      await search.fill(name);

      const row = page
        .locator('div[class*="listItem"]')
        .filter({ hasText: name })
        .first();
      await expect(row).toBeVisible();
      await expect(row.getByText(text)).toBeVisible();

      await row.locator("label.rua-toggle").first().click();
      await expect
        .poll(async () => {
          const response = await request.get(
            `${base}/api/agents/default/cron/jobs/${job.id}`,
          );
          if (!response.ok()) return null;
          return ((await response.json()) as CronJob).enabled;
        })
        .toBe(false);

      const runRequest = page.waitForRequest(
        (req) =>
          req.url().endsWith(`/api/agents/default/cron/jobs/${job.id}/run`) &&
          req.method() === "POST",
      );
      await row.locator("button.rua-btn").nth(0).click();
      await expect(
        page.getByText(/Execute Task Now|立即执行任务/).first(),
      ).toBeVisible();
      await page
        .getByRole("button", { name: /Execute Now|立即执行/ })
        .last()
        .click();
      await runRequest;

      await row.locator("button.rua-btn").nth(2).click();
      await expect(
        page
          .getByText(/Confirm Delete|Delete Cron Job|确认删除|删除定时任务/)
          .first(),
      ).toBeVisible();
      await page
        .getByRole("button", { name: /Delete|删除/ })
        .last()
        .click();
      await expect(row).not.toBeVisible();
      await expect
        .poll(async () => {
          const response = await request.get(
            `${base}/api/agents/default/cron/jobs`,
          );
          if (!response.ok()) return true;
          const jobs = (await response.json()) as CronJob[];
          return jobs.some((candidate) => candidate.id === job.id);
        })
        .toBe(false);
      deleted = true;
    } finally {
      if (!deleted) {
        await request.delete(`${base}/api/agents/default/cron/jobs/${job.id}`);
      }
    }
  });

  test("job drawer can create and edit a text cron job with backend readback", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const name = `e2e_cron_drawer_${Date.now()}`;
    const editedName = `${name}_edited`;
    const text = "E2E cron drawer text";
    const editedText = "E2E cron drawer edited text";
    let jobId: string | null = null;

    try {
      await page.goto("/cron-jobs", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Cron Jobs|定时任务/).first()).toBeVisible();

      const headerButtons = page
        .locator(".rua-btn")
        .filter({ has: page.locator("svg") });
      await headerButtons.nth(1).click();
      await expect(
        page.getByRole("dialog").getByText(/Create Job|创建任务/),
      ).toBeVisible();

      await page.getByPlaceholder(/Daily Morning Report|每日早报/).fill(name);
      await page.getByPlaceholder(/simple text tasks|简单消息任务/).fill(text);

      const createResponse = page.waitForResponse(
        (response) =>
          response.url().endsWith("/api/agents/default/cron/jobs") &&
          response.request().method() === "POST",
      );
      await page
        .getByRole("button", { name: /Save|保存/ })
        .last()
        .click();
      const created = (await (await createResponse).json()) as CronJob;
      jobId = created.id;
      const createdId = created.id;

      await expect
        .poll(async () => {
          const response = await request.get(
            `${base}/api/agents/default/cron/jobs/${createdId}`,
          );
          if (!response.ok()) return null;
          return ((await response.json()) as CronJob).text;
        })
        .toBe(text);
      await expect(page.getByText(name).first()).toBeVisible();

      const row = page
        .locator('div[class*="listItem"]')
        .filter({ hasText: name })
        .first();
      await row.locator("button.rua-btn").nth(1).click();
      await expect(
        page.getByRole("dialog").getByText(/Edit Job|编辑任务/),
      ).toBeVisible();
      await page
        .getByPlaceholder(/Daily Morning Report|每日早报/)
        .fill(editedName);
      await page
        .getByPlaceholder(/simple text tasks|简单消息任务/)
        .fill(editedText);

      const updateResponse = page.waitForResponse(
        (response) =>
          response
            .url()
            .endsWith(`/api/agents/default/cron/jobs/${createdId}`) &&
          response.request().method() === "PUT",
      );
      await page
        .getByRole("button", { name: /Save|保存/ })
        .last()
        .click();
      await expect(page.getByText(/Reschedule|重新调度/).first()).toBeVisible();
      await page
        .getByRole("button", { name: /Save|保存/ })
        .last()
        .click();
      expect((await updateResponse).ok()).toBeTruthy();

      await expect
        .poll(async () => {
          const response = await request.get(
            `${base}/api/agents/default/cron/jobs/${createdId}`,
          );
          if (!response.ok()) return null;
          const updated = (await response.json()) as CronJob;
          return `${updated.name}:${updated.text}`;
        })
        .toBe(`${editedName}:${editedText}`);
      await expect(page.getByText(editedName).first()).toBeVisible();
    } finally {
      if (jobId) {
        await request.delete(`${base}/api/agents/default/cron/jobs/${jobId}`);
      }
    }
  });
});
