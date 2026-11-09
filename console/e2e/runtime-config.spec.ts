import { test, expect } from "./fixtures";
import type { APIRequestContext, Page } from "@playwright/test";

type RunningConfig = {
  max_iters: number;
  max_input_length: number;
  auto_continue_on_text_only?: boolean;
  llm_retry_enabled: boolean;
  llm_max_retries: number;
  context_compact: Record<string, unknown>;
  [key: string]: unknown;
};

async function createAgent(
  request: APIRequestContext,
  base: string,
  id: string,
): Promise<void> {
  const response = await request.post(`${base}/api/agents`, {
    data: {
      id,
      name: `E2E Runtime Config ${id}`,
      description: "temporary runtime config e2e agent",
    },
  });
  expect(response.status()).toBe(201);
}

async function deleteAgent(
  request: APIRequestContext,
  base: string,
  id: string,
): Promise<void> {
  await request.delete(`${base}/api/agents/${encodeURIComponent(id)}`);
}

async function getRunningConfig(
  request: APIRequestContext,
  base: string,
  agentId: string,
): Promise<RunningConfig> {
  const response = await request.get(
    `${base}/api/agents/${agentId}/running-config`,
  );
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as RunningConfig;
}

async function putRunningConfig(
  request: APIRequestContext,
  base: string,
  agentId: string,
  config: RunningConfig,
): Promise<RunningConfig> {
  const response = await request.put(
    `${base}/api/agents/${agentId}/running-config`,
    { data: config },
  );
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as RunningConfig;
}

async function selectAgentOnPage(page: Page, agentId: string): Promise<void> {
  await page.addInitScript((selectedAgent: string) => {
    sessionStorage.setItem(
      "rua-agent-storage",
      JSON.stringify({
        state: { selectedAgent },
        version: 0,
      }),
    );
  }, agentId);
}

test.describe("runtime config", () => {
  test("agent config API and page mount on the real Rust server", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const configResponse = await request.get(
      `${base}/api/agents/default/running-config`,
    );
    expect(configResponse.ok()).toBeTruthy();

    const config = (await configResponse.json()) as RunningConfig;
    expect(typeof config.max_iters).toBe("number");
    expect(typeof config.max_input_length).toBe("number");
    expect(typeof config.llm_retry_enabled).toBe("boolean");
    expect(typeof config.llm_max_retries).toBe("number");
    expect(config.context_compact && typeof config.context_compact).toBe(
      "object",
    );

    const languageResponse = await request.get(
      `${base}/api/agents/default/language`,
    );
    expect(languageResponse.ok()).toBeTruthy();
    expect(await languageResponse.json()).toEqual(
      expect.objectContaining({ language: expect.any(String) }),
    );

    const timezoneResponse = await request.get(
      `${base}/api/config/user-timezone`,
    );
    expect(timezoneResponse.ok()).toBeTruthy();
    expect(await timezoneResponse.json()).toEqual(
      expect.objectContaining({ timezone: expect.any(String) }),
    );

    await page.goto("/agent-config", { waitUntil: "domcontentloaded" });
    await expect(
      page.getByText(/Configuration|运行配置/).first(),
    ).toBeVisible();
    await expect(
      page.getByText(/ReAct Agent|ReAct 智能体/).first(),
    ).toBeVisible();
    await expect(
      page.getByText(/LLM Auto Retry|LLM 自动重试/).first(),
    ).toBeVisible();
    await expect(
      page.getByText(/LLM Rate Limiter|LLM 并发限流/).first(),
    ).toBeVisible();
    await expect(
      page.getByText(/Context Management|上下文管理配置/).first(),
    ).toBeVisible();
  });

  test("agent running config save persists through the Rust scoped API", async ({
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const agentId = `e2e-runtime-${Date.now()}`;

    try {
      await createAgent(request, base, agentId);

      const before = await getRunningConfig(request, base, agentId);
      expect(typeof before.max_iters).toBe("number");

      const nextMaxIters = before.max_iters + 1;
      expect(
        await putRunningConfig(request, base, agentId, {
          ...before,
          max_iters: nextMaxIters,
        }),
      ).toEqual(expect.objectContaining({ max_iters: nextMaxIters }));

      await expect(await getRunningConfig(request, base, agentId)).toEqual(
        expect.objectContaining({ max_iters: nextMaxIters }),
      );
    } finally {
      await deleteAgent(request, base, agentId);
    }
  });

  test("agent config page saves core running config for the selected Rust agent", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const agentId = `e2e-runtime-page-${Date.now()}`;

    try {
      await createAgent(request, base, agentId);
      const before = await getRunningConfig(request, base, agentId);
      const nextMaxIters = before.max_iters + 2;
      const nextAutoContinue = !before.auto_continue_on_text_only;

      await selectAgentOnPage(page, agentId);
      await page.goto("/agent-config", { waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/Configuration|运行配置/).first(),
      ).toBeVisible();

      await page
        .locator(".rua-number-input")
        .first()
        .fill(String(nextMaxIters));
      const autoContinueToggle = page
        .locator("input.rua-toggle-input")
        .first();
      if ((await autoContinueToggle.isChecked()) !== nextAutoContinue) {
        await autoContinueToggle.click();
      }
      await page.getByRole("button", { name: /Save|保存/ }).click();

      await expect
        .poll(
          async () => {
            const config = await getRunningConfig(request, base, agentId);
            return {
              auto_continue_on_text_only:
                config.auto_continue_on_text_only ?? false,
              max_iters: config.max_iters,
            };
          },
          { timeout: 10_000 },
        )
        .toEqual({
          auto_continue_on_text_only: nextAutoContinue,
          max_iters: nextMaxIters,
        });

      await page.reload({ waitUntil: "domcontentloaded" });
      await expect(page.locator(".rua-number-input").first()).toHaveValue(
        String(nextMaxIters),
      );
    } finally {
      await deleteAgent(request, base, agentId);
    }
  });
});
