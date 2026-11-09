import { test, expect } from "./fixtures";
import type { APIRequestContext, Page } from "@playwright/test";

type EnvVar = {
  key: string;
  value: string;
  masked?: boolean | null;
};

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

async function replaceEnvs(
  request: APIRequestContext,
  baseURL: string | undefined,
  envs: Record<string, string>,
): Promise<EnvVar[]> {
  const response = await request.put(`${apiBase(baseURL)}/api/envs`, {
    data: envs,
  });
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<EnvVar[]>;
}

async function listEnvs(
  request: APIRequestContext,
  baseURL?: string,
): Promise<EnvVar[]> {
  const response = await request.get(`${apiBase(baseURL)}/api/envs`);
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<EnvVar[]>;
}

async function envValue(
  request: APIRequestContext,
  baseURL: string | undefined,
  key: string,
): Promise<string | undefined> {
  return (await listEnvs(request, baseURL)).find((env) => env.key === key)
    ?.value;
}

function envsToRecord(envs: EnvVar[]): Record<string, string> {
  return Object.fromEntries(envs.map((env) => [env.key, env.value]));
}

async function openCreateDrawer(page: Page): Promise<void> {
  await page.getByRole("button", { name: /Create Variable|新建变量/ }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
}

async function fillDrawer(
  page: Page,
  key: string,
  value: string,
): Promise<void> {
  await page.getByPlaceholder("MY_VAR").fill(key);
  await page.getByRole("textbox").last().fill(value);
}

async function saveDrawer(page: Page): Promise<void> {
  await page.getByRole("button", { name: /^Save$|^保存$/ }).click();
  await expect(page.getByRole("dialog")).toBeHidden();
}

test.describe("environments", () => {
  test("environment variables page creates, edits, and deletes through the real API", async ({
    page,
    request,
    baseURL,
  }) => {
    const key = `E2E_ENV_${Date.now()}`;
    const originalEnvs = await listEnvs(request, baseURL);

    try {
      await replaceEnvs(request, baseURL, {});

      await page.goto("/environments", { waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/Environment Variables|环境变量/).first(),
      ).toBeVisible();
      await expect(
        page.getByText(/No environment variables configured yet|暂无环境变量/),
      ).toBeVisible();

      await openCreateDrawer(page);
      await fillDrawer(page, key, "initial-value");
      await saveDrawer(page);

      await expect
        .poll(() => envValue(request, baseURL, key))
        .toBe("initial-value");
      const row = page.locator(".rua-ui-table-row", { hasText: key }).first();
      await expect(row).toBeVisible();
      await expect(row).toContainText("initial-value");

      await row.getByRole("button", { name: /Edit|编辑/ }).click();
      await expect(page.getByRole("dialog")).toBeVisible();
      await page.getByRole("textbox").last().fill("updated-value");
      await saveDrawer(page);
      await expect
        .poll(() => envValue(request, baseURL, key))
        .toBe("updated-value");
      await expect(row).toContainText("updated-value");

      await row.getByRole("button", { name: /Delete|删除/ }).click();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: /^Delete$|^删除$/ })
        .click();
      await expect.poll(() => envValue(request, baseURL, key)).toBeUndefined();
      await expect(page.getByText(key)).toHaveCount(0);
    } finally {
      await replaceEnvs(request, baseURL, envsToRecord(originalEnvs));
    }
  });
});
