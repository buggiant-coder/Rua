import { test, expect } from "./fixtures";
import type { APIRequestContext, Page } from "@playwright/test";

type ToolInfo = {
  name: string;
  description: string;
  enabled: boolean;
  async_execution: boolean;
};

async function listTools(
  request: APIRequestContext,
  baseURL?: string,
  agentId = "default",
): Promise<ToolInfo[]> {
  const base = baseURL ?? "http://127.0.0.1:8090";
  const response = await request.get(
    `${base}/api/agents/${encodeURIComponent(agentId)}/tools`,
  );
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<ToolInfo[]>;
}

async function findTool(
  request: APIRequestContext,
  baseURL: string | undefined,
  name: string,
  agentId = "default",
): Promise<ToolInfo> {
  const tools = await listTools(request, baseURL, agentId);
  const tool = tools.find((candidate) => candidate.name === name);
  expect(
    tool,
    `missing ${name} for ${agentId} in ${tools.map((t) => t.name).join(", ")}`,
  ).toBeTruthy();
  return tool as ToolInfo;
}

async function toggleTool(
  request: APIRequestContext,
  baseURL: string | undefined,
  name: string,
): Promise<ToolInfo> {
  const base = baseURL ?? "http://127.0.0.1:8090";
  const response = await request.patch(
    `${base}/api/agents/default/tools/${encodeURIComponent(name)}/toggle`,
  );
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<ToolInfo>;
}

async function restoreToolEnabledStates(
  request: APIRequestContext,
  baseURL: string | undefined,
  baseline: ToolInfo[],
): Promise<void> {
  const byName = new Map(baseline.map((tool) => [tool.name, tool]));
  const current = await listTools(request, baseURL);

  for (const tool of current) {
    const original = byName.get(tool.name);
    if (original && original.enabled !== tool.enabled) {
      await toggleTool(request, baseURL, tool.name);
    }
  }
}

async function clickHeaderToolMenuItem(
  page: Page,
  label: RegExp,
): Promise<void> {
  const headerActions = page.locator('div[class*="headerActions"]').first();
  await headerActions.locator(".rua-ui-dropdown-trigger").click();
  await page.locator(".rua-ui-dropdown-item", { hasText: label }).click();
}

async function createAgent(
  request: APIRequestContext,
  baseURL: string | undefined,
  agentId: string,
): Promise<void> {
  const base = baseURL ?? "http://127.0.0.1:8090";
  const response = await request.post(`${base}/api/agents`, {
    data: {
      id: agentId,
      name: `E2E Tools ${agentId}`,
      description: "temporary tools e2e agent",
    },
  });
  expect(response.status()).toBe(201);
}

async function deleteAgent(
  request: APIRequestContext,
  baseURL: string | undefined,
  agentId: string,
): Promise<void> {
  const base = baseURL ?? "http://127.0.0.1:8090";
  await request.delete(`${base}/api/agents/${encodeURIComponent(agentId)}`);
}

test.describe("tools", () => {
  test("tools page can search, toggle Bash, and restore state", async ({
    page,
    request,
    baseURL,
  }) => {
    const baseline = await findTool(request, baseURL, "Bash");

    await page.goto("/tools", { waitUntil: "domcontentloaded" });
    await expect(
      page.getByText(/Built-in Tools|内置工具/).first(),
    ).toBeVisible();

    await page.getByPlaceholder(/Search tools|搜索工具/).fill("Bash");
    const bashCard = page
      .locator('div[class*="toolCard"]')
      .filter({ hasText: "Bash" })
      .first();
    await expect(bashCard).toBeVisible();
    await expect(
      bashCard.getByText(/Enabled|Disabled|已启用|已禁用/),
    ).toBeVisible();

    await bashCard.locator("button").last().click();
    await expect
      .poll(async () => (await findTool(request, baseURL, "Bash")).enabled)
      .toBe(!baseline.enabled);

    await bashCard.locator("button").last().click();
    await expect
      .poll(async () => (await findTool(request, baseURL, "Bash")).enabled)
      .toBe(baseline.enabled);
  });

  test("Bash async execution control round-trips when the tool is enabled", async ({
    page,
    request,
    baseURL,
  }) => {
    const baseline = await findTool(request, baseURL, "Bash");

    await page.goto("/tools", { waitUntil: "domcontentloaded" });
    await page.getByPlaceholder(/Search tools|搜索工具/).fill("Bash");
    const bashCard = page
      .locator('div[class*="toolCard"]')
      .filter({ hasText: "Bash" })
      .first();
    await expect(bashCard).toBeVisible();

    if (!baseline.enabled) {
      await bashCard.locator("button").last().click();
      await expect
        .poll(async () => (await findTool(request, baseURL, "Bash")).enabled)
        .toBe(true);
    }

    await bashCard
      .getByRole("button", {
        name: /Async execution enabled|Async execution disabled|异步执行已启用|异步执行已禁用/,
      })
      .click();
    await expect
      .poll(
        async () => (await findTool(request, baseURL, "Bash")).async_execution,
      )
      .toBe(!baseline.async_execution);

    await bashCard
      .getByRole("button", {
        name: /Async execution enabled|Async execution disabled|异步执行已启用|异步执行已禁用/,
      })
      .click();
    await expect
      .poll(
        async () => (await findTool(request, baseURL, "Bash")).async_execution,
      )
      .toBe(baseline.async_execution);

    if (!baseline.enabled) {
      await bashCard.locator("button").last().click();
      await expect
        .poll(async () => (await findTool(request, baseURL, "Bash")).enabled)
        .toBe(false);
    }
  });

  test("global enable and disable controls persist through the agent tools API", async ({
    page,
    request,
    baseURL,
  }) => {
    const baseline = await listTools(request, baseURL);
    expect(baseline.length).toBeGreaterThan(0);

    try {
      await page.goto("/tools", { waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/Built-in Tools|内置工具/).first(),
      ).toBeVisible();

      await clickHeaderToolMenuItem(page, /Disable All|全部禁用/);
      await expect
        .poll(async () =>
          (await listTools(request, baseURL)).every((tool) => !tool.enabled),
        )
        .toBe(true);
      await expect(
        page.locator('div[class*="toolCard"] span[class*="statusText"]', {
          hasText: /Disabled|已禁用/,
        }),
      ).toHaveCount(baseline.length);

      await clickHeaderToolMenuItem(page, /Enable All|全部启用/);
      await expect
        .poll(async () =>
          (await listTools(request, baseURL)).every((tool) => tool.enabled),
        )
        .toBe(true);
      await expect(
        page.locator('div[class*="toolCard"] span[class*="statusText"]', {
          hasText: /Enabled|已启用/,
        }),
      ).toHaveCount(baseline.length);
    } finally {
      await restoreToolEnabledStates(request, baseURL, baseline);
    }
  });

  test("selected-agent tools page changes stay isolated from default agent tools", async ({
    page,
    request,
    baseURL,
  }) => {
    const agentId = `e2e-tools-${Date.now()}`;

    try {
      await createAgent(request, baseURL, agentId);
      const defaultBaseline = await findTool(request, baseURL, "Bash");
      const scopedBaseline = await findTool(request, baseURL, "Bash", agentId);

      await page.addInitScript((selectedAgent) => {
        sessionStorage.setItem(
          "rua-agent-storage",
          JSON.stringify({
            state: { selectedAgent },
            version: 0,
          }),
        );
      }, agentId);

      await page.goto("/tools", { waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/Built-in Tools|内置工具/).first(),
      ).toBeVisible();

      await page.getByPlaceholder(/Search tools|搜索工具/).fill("Bash");
      const bashCard = page
        .locator('div[class*="toolCard"]')
        .filter({ hasText: "Bash" })
        .first();
      await expect(bashCard).toBeVisible();

      await bashCard.locator("button").last().click();
      await expect
        .poll(
          async () =>
            (await findTool(request, baseURL, "Bash", agentId)).enabled,
        )
        .toBe(!scopedBaseline.enabled);

      await expect
        .poll(async () => (await findTool(request, baseURL, "Bash")).enabled)
        .toBe(defaultBaseline.enabled);
    } finally {
      await deleteAgent(request, baseURL, agentId);
    }
  });
});
