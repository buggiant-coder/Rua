import { type APIRequestContext } from "@playwright/test";
import { test, expect } from "./fixtures";

const AGENT_ID = "default";

function mcpClient(name: string, description: string) {
  return {
    name,
    description,
    enabled: false,
    transport: "stdio",
    command: "/bin/echo",
    args: ["mcp"],
    env: {},
    cwd: "",
  };
}

async function createAgentMcp(
  request: APIRequestContext,
  base: string,
  key: string,
  name: string,
  description: string,
) {
  const response = await request.post(`${base}/api/agents/${AGENT_ID}/mcp`, {
    data: {
      client_key: key,
      client: mcpClient(name, description),
    },
  });
  expect(response.status()).toBe(201);
}

async function deleteAgentMcp(
  request: APIRequestContext,
  base: string,
  key: string,
) {
  await request.delete(`${base}/api/agents/${AGENT_ID}/mcp/${key}`);
}

async function getAgentMcp(
  request: APIRequestContext,
  base: string,
  key: string,
): Promise<{ key: string; enabled: boolean }> {
  const response = await request.get(
    `${base}/api/agents/${AGENT_ID}/mcp/${key}`,
  );
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<{ key: string; enabled: boolean }>;
}

async function createSharedMcp(
  request: APIRequestContext,
  base: string,
  key: string,
  name: string,
  description: string,
) {
  const response = await request.post(`${base}/api/shared-mcp`, {
    data: {
      client_key: key,
      client: mcpClient(name, description),
    },
  });
  expect(response.ok()).toBeTruthy();
}

async function deleteSharedMcp(
  request: APIRequestContext,
  base: string,
  key: string,
) {
  await request.delete(`${base}/api/shared-mcp/${key}`);
}

async function getSharedMcp(
  request: APIRequestContext,
  base: string,
  key: string,
): Promise<{ key: string; description: string; enabled: boolean }> {
  const response = await request.get(`${base}/api/shared-mcp/${key}`);
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<{
    key: string;
    description: string;
    enabled: boolean;
  }>;
}

test.describe("mcp", () => {
  test("agent MCP page renders seeded disabled clients", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const key = `e2e_mcp_${Date.now()}`;
    const name = "E2E MCP Client";
    const description = "E2E disabled MCP client";

    await createAgentMcp(request, base, key, name, description);
    try {
      const list = await request.get(`${base}/api/agents/${AGENT_ID}/mcp`);
      expect(list.ok()).toBeTruthy();
      const clients = (await list.json()) as Array<{ key: string }>;
      expect(clients.some((client) => client.key === key)).toBeTruthy();

      await page.goto("/mcp", { waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/MCP Clients|MCP 客户端/).first(),
      ).toBeVisible();

      const search = page.getByPlaceholder(/Search MCP|搜索 MCP/).first();
      await expect(search).toBeVisible();
      await search.fill(key);

      await expect(page.getByText(name).first()).toBeVisible();
      await expect(page.getByText(description).first()).toBeVisible();
      const card = page
        .locator('div[class*="mcpCard"]')
        .filter({ hasText: name })
        .first();
      const enabledToggle = card.locator("input.rua-toggle-input").first();
      await expect(enabledToggle).not.toBeChecked();

      await enabledToggle.click();
      await expect
        .poll(async () => (await getAgentMcp(request, base, key)).enabled)
        .toBe(true);

      await enabledToggle.click();
      await expect
        .poll(async () => (await getAgentMcp(request, base, key)).enabled)
        .toBe(false);

      await page
        .getByRole("button", { name: /Tools|工具列表/ })
        .first()
        .click();
      await expect(
        page
          .getByText(
            /Enable this MCP client to view its tools|启用此 MCP 客户端后即可查看工具列表/,
          )
          .first(),
      ).toBeVisible();
    } finally {
      await deleteAgentMcp(request, base, key);
    }
  });

  test("shared MCP page renders seeded local shared clients", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const key = `e2e_shared_mcp_${Date.now()}`;
    const name = "E2E Shared MCP";
    const description = "E2E shared MCP client";

    await createSharedMcp(request, base, key, name, description);
    try {
      const list = await request.get(`${base}/api/shared-mcp`);
      expect(list.ok()).toBeTruthy();
      const clients = (await list.json()) as Array<{ key: string }>;
      expect(clients.some((client) => client.key === key)).toBeTruthy();

      await page.goto("/shared-mcp", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Shared MCP|共享 MCP/).first()).toBeVisible();

      const search = page.getByPlaceholder(/Search shared MCP|搜索共享 MCP/);
      await expect(search).toBeVisible();
      await search.fill(key);

      await expect(page.getByText(name).first()).toBeVisible();
      await expect(page.getByText(description).first()).toBeVisible();

      const updatedDescription = "E2E shared MCP client updated from page";
      const card = page
        .locator('div[class*="mcpCard"]')
        .filter({ hasText: name })
        .first();
      await card.getByRole("button", { name: /Edit|编辑/ }).click();
      const editor = page.locator("textarea").first();
      await expect(editor).toBeVisible();
      await editor.fill(
        JSON.stringify(
          {
            key,
            ...mcpClient(name, updatedDescription),
            enabled: true,
          },
          null,
          2,
        ),
      );
      await page.getByRole("button", { name: /Save|保存/ }).click();
      await expect
        .poll(async () => {
          const client = await getSharedMcp(request, base, key);
          return {
            description: client.description,
            enabled: client.enabled,
          };
        })
        .toEqual({ description: updatedDescription, enabled: true });
    } finally {
      await deleteSharedMcp(request, base, key);
    }
  });
});
