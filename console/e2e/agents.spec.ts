import { test, expect } from "./fixtures";
import type { APIRequestContext } from "@playwright/test";

type AgentSummary = {
  id: string;
  name: string;
  description: string;
  workspace_dir: string;
  enabled: boolean;
};

type AgentListResponse = {
  agents: AgentSummary[];
};

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

async function getAgentSummary(
  request: APIRequestContext,
  base: string,
  agentId: string,
): Promise<AgentSummary> {
  const response = await request.get(
    `${base}/api/agents/${encodeURIComponent(agentId)}`,
  );
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<AgentSummary>;
}

test.describe("agents", () => {
  test("agent registry and agents page mount on the real Rust server", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = apiBase(baseURL);
    const suffix = Date.now();
    const agentId = `e2e-agent-${suffix}`;
    const name = `E2E Agent ${suffix}`;
    const description = `QwenPaw alignment agent ${suffix}`;

    try {
      const createResponse = await request.post(`${base}/api/agents`, {
        data: {
          id: agentId,
          name,
          description,
        },
      });
      expect(createResponse.status()).toBe(201);
      await expect(await createResponse.json()).toEqual(
        expect.objectContaining({
          id: agentId,
          workspace_dir: expect.any(String),
        }),
      );

      const listResponse = await request.get(`${base}/api/agents`);
      expect(listResponse.ok()).toBeTruthy();
      const list = (await listResponse.json()) as AgentListResponse;
      expect(list.agents.some((agent) => agent.id === "default")).toBeTruthy();
      const created = list.agents.find((agent) => agent.id === agentId);
      expect(created).toEqual(
        expect.objectContaining({
          id: agentId,
          name,
          description,
          enabled: true,
          workspace_dir: expect.any(String),
        }),
      );

      const detailResponse = await request.get(`${base}/api/agents/${agentId}`);
      expect(detailResponse.ok()).toBeTruthy();
      await expect(await detailResponse.json()).toEqual(
        expect.objectContaining({
          id: agentId,
          name,
          description,
          running: expect.any(Object),
          system_prompt_files: expect.any(Array),
        }),
      );

      await page.goto("/agents", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Agents|智能体/).first()).toBeVisible();
      await expect(page.getByText(name).first()).toBeVisible();
      await expect(page.getByText(description).first()).toBeVisible();

      await page.getByText(name).first().click();
      const dialog = page.getByRole("dialog");
      await expect(dialog.getByDisplayValue(agentId)).toBeVisible();
      await expect(dialog.getByDisplayValue(name)).toBeVisible();
      await expect(dialog.getByDisplayValue(description)).toBeVisible();
      await dialog.getByRole("button", { name: /Cancel|取消/ }).click();
      await expect(dialog).toBeHidden();
    } finally {
      await request.delete(`${base}/api/agents/${encodeURIComponent(agentId)}`);
    }
  });

  test("agents page enablement action persists through the Rust agent lifecycle API", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = apiBase(baseURL);
    const suffix = Date.now();
    const agentId = `e2e-agent-toggle-${suffix}`;
    const name = `E2E Agent Toggle ${suffix}`;
    const description = `QwenPaw alignment toggle agent ${suffix}`;

    try {
      const createResponse = await request.post(`${base}/api/agents`, {
        data: {
          id: agentId,
          name,
          description,
        },
      });
      expect(createResponse.status()).toBe(201);
      await expect
        .poll(
          async () => (await getAgentSummary(request, base, agentId)).enabled,
        )
        .toBe(true);

      await page.goto("/agents", { waitUntil: "domcontentloaded" });
      const card = page
        .getByText(name)
        .first()
        .locator("xpath=ancestor::div[contains(@class, 'card')][1]");
      await expect(card).toBeVisible();

      await card.getByRole("button", { name: /Disable|禁用/ }).click();
      await page.getByRole("button", { name: /Confirm|确定/ }).click();
      await expect
        .poll(
          async () => (await getAgentSummary(request, base, agentId)).enabled,
        )
        .toBe(false);
      await expect(card.getByText(/Disabled|已禁用/)).toBeVisible();

      await card.getByRole("button", { name: /Enable|启用/ }).click();
      await page.getByRole("button", { name: /Confirm|确定/ }).click();
      await expect
        .poll(
          async () => (await getAgentSummary(request, base, agentId)).enabled,
        )
        .toBe(true);
    } finally {
      await request.delete(`${base}/api/agents/${encodeURIComponent(agentId)}`);
    }
  });
});
