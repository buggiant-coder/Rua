import { test, expect } from "./fixtures";
import type { APIRequestContext, Page } from "@playwright/test";

type ChannelConfig = Record<
  string,
  {
    enabled?: boolean;
    bot_prefix?: string;
    isBuiltin?: boolean;
  }
>;

type SingleChannelConfig = {
  enabled?: boolean;
  bot_prefix?: string;
  [key: string]: unknown;
};

async function getConsoleConfig(
  request: APIRequestContext,
  base: string,
): Promise<SingleChannelConfig> {
  const response = await request.get(`${base}/api/config/channels/console`);
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as SingleChannelConfig;
}

async function putConsoleConfig(
  request: APIRequestContext,
  base: string,
  config: SingleChannelConfig,
) {
  const response = await request.put(`${base}/api/config/channels/console`, {
    data: { ...config, enabled: true },
  });
  expect(response.ok()).toBeTruthy();
}

async function openConsoleDrawer(page: Page) {
  await page.getByPlaceholder(/Search channels|搜索频道/).fill("console");
  const card = page
    .locator('div[class*="channelCard"]')
    .filter({ hasText: /Console|控制台/ })
    .first();
  await expect(card).toBeVisible();
  await card.click();
  const drawer = page.getByRole("dialog").filter({
    hasText: /Console Settings|控制台 设置/,
  });
  await expect(drawer).toBeVisible();
  return drawer;
}

test.describe("channels", () => {
  test("API exposes Rust canonical channel ids", async ({
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";

    const typesResponse = await request.get(
      `${base}/api/config/channels/types`,
    );
    expect(typesResponse.ok()).toBeTruthy();
    const types = (await typesResponse.json()) as string[];

    expect(types).toContain("console");
    expect(types).toContain("dingtalk");
    expect(types).toContain("wechat");
    expect(types).toContain("yuanbao");
    expect(types).not.toContain("weixin");

    const configResponse = await request.get(`${base}/api/config/channels`);
    expect(configResponse.ok()).toBeTruthy();
    const channels = (await configResponse.json()) as ChannelConfig;

    expect(channels.console?.enabled).toBe(true);
    expect(channels.console?.isBuiltin).toBe(true);
    expect(channels.dingtalk?.isBuiltin).toBe(true);
  });

  test("page supports channel search and local settings drawer", async ({
    page,
  }) => {
    await page.goto("/channels", { waitUntil: "domcontentloaded" });

    await expect(page.getByText(/Channels|频道/).first()).toBeVisible();

    const search = page.getByPlaceholder(/Search channels|搜索频道/);
    await expect(search).toBeVisible();
    await expect(page.getByText(/Console|控制台/).first()).toBeVisible();
    await expect(page.getByText(/DingTalk|钉钉/).first()).toBeVisible();

    await search.fill("ding");
    await expect(page.getByText(/DingTalk|钉钉/).first()).toBeVisible();
    await expect(page.getByText(/Console|控制台/).first()).not.toBeVisible();

    await search.fill("wechat");
    await expect(page.getByText(/WeChat|微信/).first()).toBeVisible();

    await search.fill("weixin");
    await expect(page.getByText(/WeChat|微信/).first()).not.toBeVisible();
    await expect(
      page.getByText(/No matching channels|暂无数据/).first(),
    ).toBeVisible();

    await search.fill("console");
    await page
      .getByText(/Console|控制台/)
      .first()
      .click();

    await expect(
      page.getByText(/Console Settings|控制台 设置/).first(),
    ).toBeVisible();
    await expect(page.getByText(/Enabled|启用/).first()).toBeVisible();
    await expect(page.getByText(/Bot Prefix|机器人前缀/).first()).toBeVisible();

    await page.getByRole("button", { name: /Cancel|取消/ }).click();
    await expect(
      page.getByText(/Console Settings|控制台 设置/).first(),
    ).not.toBeVisible();
  });

  test("Console channel bot prefix save persists and cancel preserves state", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const original = await getConsoleConfig(request, base);
    const savedPrefix = `e2e-console-${Date.now()}`;
    const cancelledPrefix = `${savedPrefix}-cancelled`;

    try {
      await page.goto("/channels", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Channels|频道/).first()).toBeVisible();

      let drawer = await openConsoleDrawer(page);
      const prefixInput = drawer.getByPlaceholder("@bot").first();
      await prefixInput.fill(savedPrefix);
      await drawer.getByRole("button", { name: /Save|保存/ }).click();

      await expect
        .poll(async () => (await getConsoleConfig(request, base)).bot_prefix)
        .toBe(savedPrefix);

      drawer = await openConsoleDrawer(page);
      const reopenedInput = drawer.getByPlaceholder("@bot").first();
      await expect(reopenedInput).toHaveValue(savedPrefix);
      await reopenedInput.fill(cancelledPrefix);
      await drawer.getByRole("button", { name: /Cancel|取消/ }).click();

      await expect
        .poll(async () => (await getConsoleConfig(request, base)).bot_prefix)
        .toBe(savedPrefix);
    } finally {
      await putConsoleConfig(request, base, original);
    }
  });
});
