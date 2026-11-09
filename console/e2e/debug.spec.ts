import { test, expect } from "./fixtures";

type BackendLogsResponse = {
  path: string;
  exists: boolean;
  lines: number;
  updated_at: number | null;
  size: number;
  content: string;
};

test.describe("debug", () => {
  test("backend log page mounts and can refresh logs", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const apiResponse = await request.get(
      `${base}/api/console/debug/backend-logs?lines=20`,
    );
    expect(apiResponse.ok()).toBeTruthy();

    const body = (await apiResponse.json()) as BackendLogsResponse;
    expect(body.path).toContain("logs/rua.log");
    expect(body.lines).toBe(20);
    expect(typeof body.exists).toBe("boolean");
    expect(typeof body.content).toBe("string");

    await page.goto("/debug", { waitUntil: "domcontentloaded" });
    await expect(page.getByText(/Backend logs|后端日志/).first()).toBeVisible();
    await expect(page.getByText(/Newest first|最新在前/)).toBeVisible();
    await expect(page.getByText(/Auto refresh|自动刷新/)).toBeVisible();

    const refreshButton = page
      .getByRole("button", { name: /Refresh|刷新/ })
      .first();
    await expect(refreshButton).toBeVisible();
    await refreshButton.click();

    await expect(
      page.getByPlaceholder(/Search backend logs|搜索后端日志/),
    ).toBeVisible();
    await page
      .getByPlaceholder(/Search backend logs|搜索后端日志/)
      .fill("rua");

    await expect(
      page
        .getByText(
          /Backend log output will appear here|Backend log file was not found yet|后端日志会显示在这里|暂未找到后端日志文件/,
        )
        .or(page.locator("mark").first())
        .first(),
    ).toBeVisible();
  });

  test("backend log controls toggle and filter without leaving the page", async ({
    page,
  }) => {
    await page.goto("/debug", { waitUntil: "domcontentloaded" });
    await expect(page.getByText(/Backend logs|后端日志/).first()).toBeVisible();

    await expect(page.getByRole("button", { name: /Copy|复制/ })).toBeVisible();

    const toggles = page.locator("input.rua-toggle-input");
    await expect(toggles).toHaveCount(2);

    const newestFirst = toggles.nth(0);
    const autoRefresh = toggles.nth(1);
    const newestBefore = await newestFirst.isChecked();
    const autoBefore = await autoRefresh.isChecked();

    await newestFirst.click();
    await expect(newestFirst).toBeChecked({ checked: !newestBefore });
    await newestFirst.click();
    await expect(newestFirst).toBeChecked({ checked: newestBefore });

    await autoRefresh.click();
    await expect(autoRefresh).toBeChecked({ checked: !autoBefore });
    await autoRefresh.click();
    await expect(autoRefresh).toBeChecked({ checked: autoBefore });

    await page.locator(".rua-ui-select-trigger").first().click();
    const dropdown = page.locator(".rua-ui-select-dropdown").first();
    await expect(dropdown).toBeVisible();
    for (const level of ["All", "ERROR", "WARNING", "INFO", "DEBUG"]) {
      await expect(dropdown.getByText(level, { exact: true })).toBeVisible();
    }
    await dropdown.getByText("ERROR", { exact: true }).click();
    await expect(page.locator(".rua-ui-select-value")).toContainText("ERROR");

    await page.locator(".rua-ui-select-trigger").first().click();
    await page
      .locator(".rua-ui-select-dropdown")
      .first()
      .getByText("All", { exact: true })
      .click();
    await expect(page.locator(".rua-ui-select-value")).toContainText("All");
  });

  test("backend log level, search highlight, and sort use rendered log content", async ({
    page,
  }) => {
    await page.route(
      "**/api/console/debug/backend-logs?lines=200",
      async (route) => {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            path: "/tmp/rua.log",
            exists: true,
            lines: 200,
            updated_at: 1_718_678_400,
            size: 128,
            content: [
              "2026-06-18T00:00:01Z INFO alpha-first",
              "2026-06-18T00:00:02Z ERROR beta-target",
            ].join("\n"),
          }),
        });
      },
    );

    await page.goto("/debug", { waitUntil: "domcontentloaded" });
    const alpha = page.getByText(/INFO alpha-first/).first();
    const beta = page.getByText(/ERROR beta-target/).first();
    await expect(alpha).toBeVisible();
    await expect(beta).toBeVisible();

    const alphaBoxNewest = await alpha.boundingBox();
    const betaBoxNewest = await beta.boundingBox();
    expect(alphaBoxNewest).not.toBeNull();
    expect(betaBoxNewest).not.toBeNull();
    expect(betaBoxNewest!.y).toBeLessThan(alphaBoxNewest!.y);

    await page.locator("input.rua-toggle-input").first().click();
    const alphaBoxOldest = await alpha.boundingBox();
    const betaBoxOldest = await beta.boundingBox();
    expect(alphaBoxOldest).not.toBeNull();
    expect(betaBoxOldest).not.toBeNull();
    expect(alphaBoxOldest!.y).toBeLessThan(betaBoxOldest!.y);

    await page.locator(".rua-ui-select-trigger").first().click();
    await page
      .locator(".rua-ui-select-dropdown")
      .first()
      .getByText("ERROR", { exact: true })
      .click();
    await expect(beta).toBeVisible();
    await expect(alpha).toBeHidden();

    await page
      .getByPlaceholder(/Search backend logs|搜索后端日志/)
      .fill("target");
    await expect(page.locator("mark").getByText("target")).toBeVisible();
    await expect(page.getByText(/ERROR beta-target/).first()).toBeVisible();
  });

  test("backend log copy and auto-refresh use the rendered log content", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      Object.defineProperty(window, "isSecureContext", {
        value: true,
        configurable: true,
      });
      Object.defineProperty(navigator, "clipboard", {
        value: {
          writeText: async (text: string) => {
            (
              window as unknown as { __ruaCopiedText?: string }
            ).__ruaCopiedText = text;
          },
        },
        configurable: true,
      });
    });

    let calls = 0;
    await page.route(
      "**/api/console/debug/backend-logs?lines=200",
      async (route) => {
        calls += 1;
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            path: "/tmp/rua.log",
            exists: true,
            lines: 200,
            updated_at: 1_718_678_400 + calls,
            size: 64,
            content: `2026-06-18T00:00:0${calls}Z INFO debug-auto-refresh-${calls}`,
          }),
        });
      },
    );

    await page.goto("/debug", { waitUntil: "domcontentloaded" });
    await expect(page.getByText("debug-auto-refresh-1")).toBeVisible();

    await page.getByRole("button", { name: /Copy|复制/ }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as unknown as { __ruaCopiedText?: string })
              .__ruaCopiedText,
        ),
      )
      .toContain("debug-auto-refresh-1");

    await expect
      .poll(() => calls, { timeout: 5_000 })
      .toBeGreaterThanOrEqual(2);
    await expect(page.getByText(/debug-auto-refresh-[2-9]/)).toBeVisible();
  });
});
