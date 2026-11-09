import { test, expect } from "./fixtures";
import type { APIRequestContext } from "@playwright/test";

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

type FileGuardConfig = {
  enabled: boolean;
  paths: string[];
  builtin_paths?: string[];
  disabled_patterns?: string[];
};

async function getFileGuard(
  request: APIRequestContext,
  base: string,
): Promise<FileGuardConfig> {
  const response = await request.get(`${base}/api/config/security/file-guard`);
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<FileGuardConfig>;
}

async function putFileGuard(
  request: APIRequestContext,
  base: string,
  config: FileGuardConfig,
) {
  const response = await request.put(`${base}/api/config/security/file-guard`, {
    data: config,
  });
  expect(response.ok()).toBeTruthy();
}

test.describe("security", () => {
  test("security APIs and owned settings tabs mount on the real Rust server", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = apiBase(baseURL);
    for (const path of [
      "/api/agents/default/security/tool-guard",
      "/api/agents/default/security/tool-guard/builtin-rules",
      "/api/config/security/file-guard",
      "/api/config/security/skill-scanner",
    ]) {
      const response = await request.get(`${base}${path}`);
      expect(response.ok()).toBeTruthy();
    }

    await page.goto("/security", { waitUntil: "domcontentloaded" });
    await expect(page.getByText(/Security|安全/).first()).toBeVisible();
    await expect(
      page.getByRole("tab", { name: /Tool Guard|工具/ }),
    ).toBeVisible();
    await expect(
      page.getByText(/Execution Approval Mode|执行审批模式/),
    ).toBeVisible();
    await expect(page.getByText(/Guarded Tools|受保护工具/)).toBeVisible();
    await expect(page.getByText(/Denied Tools|拒绝工具/)).toBeVisible();
    await expect(page.getByText(/Detection Rules|检测规则/)).toBeVisible();

    await page.getByRole("tab", { name: /File Guard|文件/ }).click();
    await expect(
      page.getByText(/Enable File Guard|启用文件守卫/),
    ).toBeVisible();
    const pathInput = page.getByPlaceholder(
      /file or directory path|文件或目录路径/,
    );
    await expect(pathInput).toBeVisible();
    await pathInput.fill("~/.ssh/");
    await expect(pathInput).toHaveValue("~/.ssh/");
    await pathInput.fill("");

    await page.getByRole("tab", { name: /Skill Scanner|技能/ }).click();
    await expect(page.getByText(/Scanner Mode|扫描模式/)).toBeVisible();
    await expect(page.getByText(/Scan Alerts|扫描告警/)).toBeVisible();
    await expect(page.getByText(/Whitelist|白名单/)).toBeVisible();
  });

  test("file guard custom paths added from the page persist through the Rust security API", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = apiBase(baseURL);
    const original = await getFileGuard(request, base);
    const customPath = `/tmp/rua-e2e-file-guard-${Date.now()}/`;

    try {
      await page.goto("/security", { waitUntil: "domcontentloaded" });
      await page.getByRole("tab", { name: /File Guard|文件/ }).click();

      const enabledToggle = page.locator("input.rua-toggle-input").first();
      if (!(await enabledToggle.isChecked())) {
        await enabledToggle.click();
        await expect
          .poll(async () => (await getFileGuard(request, base)).enabled)
          .toBe(true);
      }

      const pathInput = page.getByPlaceholder(
        /file or directory path|文件或目录路径/,
      );
      await expect(pathInput).toBeVisible();
      await pathInput.fill(customPath);
      await page.getByRole("button", { name: /Add|添加/ }).click();

      await expect
        .poll(async () => (await getFileGuard(request, base)).paths)
        .toContain(customPath);
      await expect(page.getByText(customPath)).toBeVisible();
    } finally {
      await putFileGuard(request, base, original);
    }
  });
});
