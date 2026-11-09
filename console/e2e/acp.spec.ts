import { test, expect } from "./fixtures";
import type { APIRequestContext } from "@playwright/test";

async function getRunner(
  request: APIRequestContext,
  base: string,
  key: string,
): Promise<{ key: string; enabled: boolean }> {
  const response = await request.get(
    `${base}/api/acp/${encodeURIComponent(key)}`,
  );
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<{ key: string; enabled: boolean }>;
}

test.describe("acp", () => {
  test("ACP page renders builtin and seeded custom runners", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const key = `e2e_acp_runner_${Date.now()}`;
    const command = "echo";

    const create = await request.post(`${base}/api/acp`, {
      data: {
        key,
        command,
        args: ["hello-acp"],
        env: { RUA_E2E: "1" },
        enabled: false,
        trusted: true,
        tool_parse_mode: "call_detail",
        stdio_buffer_limit_bytes: 1048576,
      },
    });
    expect(create.status()).toBe(201);

    try {
      const list = await request.get(`${base}/api/acp`);
      expect(list.ok()).toBeTruthy();
      const runners = (await list.json()) as Array<{
        key: string;
        is_builtin: boolean;
      }>;
      expect(runners.some((runner) => runner.key === key)).toBeTruthy();
      expect(
        runners.some(
          (runner) => runner.key === "opencode" && runner.is_builtin,
        ),
      ).toBeTruthy();

      await page.goto("/acp", { waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/ACP Agents|ACP 智能体/).first(),
      ).toBeVisible();

      const search = page
        .getByPlaceholder(/Search ACP agents|搜索 ACP 智能体/)
        .first();
      await expect(search).toBeVisible();
      await search.fill(key);

      await expect(page.getByText(key).first()).toBeVisible();
      await expect(page.getByText(command).first()).toBeVisible();
      await expect(page.getByText("hello-acp").first()).toBeVisible();

      await search.fill("");
      await page
        .getByRole("button", { name: /Built-in|内置/ })
        .first()
        .click();
      await expect(page.getByText("opencode").first()).toBeVisible();

      await page
        .getByRole("button", { name: /Custom|自定义/ })
        .first()
        .click();
      await search.fill(key);
      await expect(page.getByText(key).first()).toBeVisible();
    } finally {
      await request.delete(`${base}/api/acp/${encodeURIComponent(key)}`);
    }
  });

  test("ACP runner enablement toggles through the page and Rust API", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const key = `e2e_acp_toggle_${Date.now()}`;

    const create = await request.post(`${base}/api/acp`, {
      data: {
        key,
        command: "echo",
        args: ["hello-acp-toggle"],
        env: {},
        enabled: true,
        trusted: true,
        tool_parse_mode: "call_detail",
        stdio_buffer_limit_bytes: 1048576,
      },
    });
    expect(create.status()).toBe(201);

    try {
      await page.goto("/acp", { waitUntil: "domcontentloaded" });
      const search = page
        .getByPlaceholder(/Search ACP agents|搜索 ACP 智能体/)
        .first();
      await expect(search).toBeVisible();
      await search.fill(key);

      const card = page
        .locator('div[class*="runnerCard"]')
        .filter({ hasText: key })
        .first();
      const enabledToggle = card.locator("input.rua-toggle-input").first();
      await expect(enabledToggle).toBeChecked();

      await enabledToggle.click();
      await expect
        .poll(async () => (await getRunner(request, base, key)).enabled)
        .toBe(false);

      await enabledToggle.click();
      await expect
        .poll(async () => (await getRunner(request, base, key)).enabled)
        .toBe(true);
    } finally {
      await request.delete(`${base}/api/acp/${encodeURIComponent(key)}`);
    }
  });
});
