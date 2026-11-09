import { test, expect } from "./fixtures";

type MarketProvider = {
  key: string;
  label: string;
  available: boolean;
  reason?: string | null;
};

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

test.describe("skill market", () => {
  test("market provider registry and page mount on the real Rust server", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = apiBase(baseURL);
    const providersResponse = await request.get(`${base}/api/market/providers`);
    expect(providersResponse.ok()).toBeTruthy();
    const providers = (await providersResponse.json()) as MarketProvider[];
    expect(providers.length).toBeGreaterThan(0);
    for (const provider of providers) {
      expect(provider.key).toEqual(expect.any(String));
      expect(provider.key.length).toBeGreaterThan(0);
      expect(provider.label).toEqual(expect.any(String));
      expect(provider.label.length).toBeGreaterThan(0);
      expect(typeof provider.available).toBe("boolean");
    }

    const unknownProvider = `e2e_unknown_market_${Date.now()}`;
    const unknownResponse = await request.post(`${base}/api/market/search`, {
      data: {
        query: "anything",
        provider_pages: { [unknownProvider]: 1 },
        limit: 5,
        lang: "en",
      },
    });
    expect(unknownResponse.status()).toBe(400);
    await expect(await unknownResponse.json()).toEqual(
      expect.objectContaining({
        detail: expect.stringContaining(unknownProvider),
      }),
    );

    await page.goto("/market", { waitUntil: "domcontentloaded" });
    await expect(page.getByText(/Skill Market|技能市场/).first()).toBeVisible();
    await expect(
      page.getByPlaceholder(/Search the skill market|搜索技能市场/),
    ).toBeVisible();
    await expect(
      page
        .getByText(
          /Type a keyword above to search the skill market|在上方输入关键字以搜索技能市场/,
        )
        .first(),
    ).toBeVisible();

    const firstProvider = providers[0];
    await expect(
      page.getByRole("button", { name: firstProvider.label }).first(),
    ).toBeVisible();

    const aliyun = providers.find((provider) => provider.key === "aliyun");
    if (aliyun && !aliyun.available) {
      await page.getByRole("button", { name: aliyun.label }).click();
      await expect(
        page.getByRole("dialog").getByText(/Aliyun configuration|阿里云配置/),
      ).toBeVisible();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: /Cancel|取消/ })
        .click();
      await expect(page.getByRole("dialog")).toBeHidden();
    }
  });
});
