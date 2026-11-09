import { test, expect } from "./fixtures";

type TokenUsageSummary = {
  total_prompt_tokens: number;
  total_completion_tokens: number;
  total_calls: number;
  by_model: Record<
    string,
    {
      provider_id?: string;
      model?: string;
      prompt_tokens?: number;
      completion_tokens?: number;
      call_count?: number;
    }
  >;
  by_date: Record<
    string,
    {
      prompt_tokens?: number;
      completion_tokens?: number;
      call_count?: number;
    }
  >;
};

test.describe("token usage", () => {
  const isoDateDaysAgo = (days: number) => {
    const date = new Date();
    date.setUTCDate(date.getUTCDate() - days);
    return date.toISOString().slice(0, 10);
  };

  test("summary API and page empty state are wired", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const response = await request.get(
      `${base}/api/token-usage?start_date=2025-05-10&end_date=2025-05-11`,
    );
    expect(response.ok()).toBeTruthy();

    const body = (await response.json()) as TokenUsageSummary;
    expect(typeof body.total_prompt_tokens).toBe("number");
    expect(typeof body.total_completion_tokens).toBe("number");
    expect(typeof body.total_calls).toBe("number");
    expect(body.by_model && typeof body.by_model).toBe("object");
    expect(body.by_date && typeof body.by_date).toBe("object");

    await page.goto("/token-usage", { waitUntil: "domcontentloaded" });
    await expect(
      page.getByText(/Token Usage|Token 消耗/).first(),
    ).toBeVisible();

    const refreshButton = page
      .getByRole("button", { name: /Refresh|刷新/ })
      .or(
        page
          .locator("button")
          .filter({ has: page.locator("svg") })
          .first(),
      )
      .first();
    await expect(refreshButton).toBeVisible();
    await refreshButton.click();

    await expect(
      page
        .getByText(
          /No token usage data in the selected period|所选时间段内暂无 Token 消耗数据|Prompt Tokens|输入 Token|By Model|按模型/,
        )
        .first(),
    ).toBeVisible();
  });

  test("renders seeded non-empty usage by model and date", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const startDate = isoDateDaysAgo(30);
    const endDate = isoDateDaysAgo(0);
    const response = await request.get(
      `${base}/api/token-usage?start_date=${startDate}&end_date=${endDate}`,
    );
    expect(response.ok()).toBeTruthy();

    const body = (await response.json()) as TokenUsageSummary;
    test.skip(
      !body.by_model["dashscope:qwen-plus"],
      "requires the scheduled E2E token usage seed fixture",
    );

    expect(body.total_calls).toBeGreaterThanOrEqual(3);
    expect(body.by_model["dashscope:qwen-plus"].provider_id).toBe("dashscope");
    expect(body.by_model["dashscope:qwen-plus"].model).toBe("qwen-plus");
    expect(body.by_model["dashscope:qwen-plus"].prompt_tokens).toBe(200);

    await page.goto("/token-usage", { waitUntil: "domcontentloaded" });

    await expect(
      page.getByText(/Prompt Tokens|输入 Token/).first(),
    ).toBeVisible();
    await expect(
      page.getByText(/Completion Tokens|输出 Token/).first(),
    ).toBeVisible();
    await expect(page.getByText(/By Model|按模型/).first()).toBeVisible();
    await expect(page.getByText(/By Date|按日期/).first()).toBeVisible();
    await expect(page.getByText("dashscope").first()).toBeVisible();
    await expect(page.getByText("qwen-plus").first()).toBeVisible();
    await expect(page.getByText("openai").first()).toBeVisible();
    await expect(page.getByText("gpt-4o").first()).toBeVisible();
  });
});
