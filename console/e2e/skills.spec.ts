import { type APIRequestContext } from "@playwright/test";
import { test, expect } from "./fixtures";

const AGENT_ID = "default";

const skillContent = (name: string, description: string) =>
  `---\nname: ${name}\ndescription: ${description}\n---\n\n# ${name}\n`;

async function createWorkspaceSkill(
  request: APIRequestContext,
  base: string,
  name: string,
  description: string,
) {
  const response = await request.post(`${base}/api/agents/${AGENT_ID}/skills`, {
    data: {
      name,
      description,
      content: skillContent(name, description),
      enable: true,
    },
  });
  expect(response.status()).toBe(201);
}

async function deleteWorkspaceSkill(
  request: APIRequestContext,
  base: string,
  name: string,
) {
  await request.delete(`${base}/api/agents/${AGENT_ID}/skills/${name}`);
}

async function getWorkspaceSkill(
  request: APIRequestContext,
  base: string,
  name: string,
): Promise<{ name: string; enabled: boolean }> {
  const response = await request.get(`${base}/api/agents/${AGENT_ID}/skills`);
  expect(response.ok()).toBeTruthy();
  const skills = (await response.json()) as Array<{
    name: string;
    enabled: boolean;
  }>;
  const skill = skills.find((item) => item.name === name);
  expect(skill, `expected seeded skill ${name} in workspace list`).toBeTruthy();
  return skill!;
}

async function createPoolSkill(
  request: APIRequestContext,
  base: string,
  name: string,
  description: string,
) {
  const response = await request.post(`${base}/api/skills/pool/create`, {
    data: {
      name,
      description,
      content: skillContent(name, description),
    },
  });
  expect(response.status()).toBe(201);
}

async function deletePoolSkill(
  request: APIRequestContext,
  base: string,
  name: string,
) {
  await request.delete(`${base}/api/skills/pool/${name}`);
}

test.describe("skills", () => {
  test("workspace skills page renders seeded local skills", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const name = `e2e_workspace_skill_${Date.now()}`;
    const description = "E2E workspace skill description";

    await createWorkspaceSkill(request, base, name, description);
    try {
      const list = await request.get(`${base}/api/agents/${AGENT_ID}/skills`);
      expect(list.ok()).toBeTruthy();
      const skills = (await list.json()) as Array<{ name: string }>;
      expect(skills.some((skill) => skill.name === name)).toBeTruthy();

      await page.goto("/skills", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Skills|技能/).first()).toBeVisible();

      const search = page.getByPlaceholder(/Search skills|搜索技能/).first();
      await expect(search).toBeVisible();
      await search.fill(name);

      await expect(page.getByText(name).first()).toBeVisible();
      await expect(page.getByText(description).first()).toBeVisible();

      await page.getByText(name).first().click();
      await expect(page.getByText(/View Skill|查看技能/).first()).toBeVisible();
      await expect(page.getByText(name).first()).toBeVisible();
      await page.getByRole("button", { name: /Cancel|取消/ }).click();
    } finally {
      await deleteWorkspaceSkill(request, base, name);
    }
  });

  test("workspace skill enablement changes persist through the Rust agent API", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const name = `e2e_workspace_skill_toggle_${Date.now()}`;
    const description = "E2E workspace skill toggle description";

    await createWorkspaceSkill(request, base, name, description);
    try {
      await expect
        .poll(
          async () => (await getWorkspaceSkill(request, base, name)).enabled,
        )
        .toBe(true);

      await page.goto("/skills", { waitUntil: "domcontentloaded" });
      const search = page.getByPlaceholder(/Search skills|搜索技能/).first();
      await expect(search).toBeVisible();
      await search.fill(name);

      const card = page
        .locator('div[class*="skillCard"]')
        .filter({ hasText: name })
        .first();
      await expect(card).toBeVisible();
      await expect(card.getByText(description)).toBeVisible();

      await card.getByRole("button", { name: /Disable|禁用/ }).click();
      await expect
        .poll(
          async () => (await getWorkspaceSkill(request, base, name)).enabled,
        )
        .toBe(false);

      await card.getByRole("button", { name: /Enable|启用/ }).click();
      await expect
        .poll(
          async () => (await getWorkspaceSkill(request, base, name)).enabled,
        )
        .toBe(true);
    } finally {
      await deleteWorkspaceSkill(request, base, name);
    }
  });

  test("skill pool page renders seeded local pool skills", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const name = `e2e_pool_skill_${Date.now()}`;
    const description = "E2E pool skill description";

    await createPoolSkill(request, base, name, description);
    try {
      const list = await request.get(`${base}/api/skills/pool`);
      expect(list.ok()).toBeTruthy();
      const skills = (await list.json()) as Array<{ name: string }>;
      expect(skills.some((skill) => skill.name === name)).toBeTruthy();

      await page.goto("/skill-pool", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Skill Pool|技能池/).first()).toBeVisible();

      const search = page.getByPlaceholder(/Search skills|搜索技能/).first();
      await expect(search).toBeVisible();
      await search.fill(name);

      await expect(page.getByText(name).first()).toBeVisible();
      await expect(page.getByText(description).first()).toBeVisible();

      await page.getByText(name).first().click();
      await expect(page.getByText(/View Skill|查看技能/).first()).toBeVisible();
      await expect(page.getByText(name).first()).toBeVisible();
      await page.getByRole("button", { name: /Cancel|取消/ }).click();
    } finally {
      await deletePoolSkill(request, base, name);
    }
  });
});
