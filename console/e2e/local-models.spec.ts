import { test, expect } from "./fixtures";

type LocalServerStatus = {
  available: boolean;
  installable: boolean;
  installed: boolean;
  port: number | null;
  model_name: string | null;
  message: string | null;
};

type LocalModelInfo = {
  id: string;
  name: string;
  size_bytes: number;
  downloaded: boolean;
  source: "huggingface" | "modelscope" | "auto";
};

test.describe("local models", () => {
  test("API and models page expose no-download local model management", async ({
    page,
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";

    const statusResponse = await request.get(`${base}/api/local-models/server`);
    expect(statusResponse.ok()).toBeTruthy();
    const status = (await statusResponse.json()) as LocalServerStatus;
    expect(typeof status.available).toBe("boolean");
    expect(typeof status.installable).toBe("boolean");
    expect(typeof status.installed).toBe("boolean");

    const modelsResponse = await request.get(`${base}/api/local-models/models`);
    expect(modelsResponse.ok()).toBeTruthy();
    const models = (await modelsResponse.json()) as LocalModelInfo[];
    expect(Array.isArray(models)).toBeTruthy();

    await page.goto("/models", { waitUntil: "domcontentloaded" });
    await expect(page.getByText(/Default LLM|默认 LLM/).first()).toBeVisible();

    const search = page.getByPlaceholder(/Search providers|搜索提供商/);
    await expect(search).toBeVisible();
    await search.fill("Rua Local");

    await expect(page.getByText("Rua Local").first()).toBeVisible();
    await page
      .getByRole("button", { name: /^Models$|^模型$/ })
      .first()
      .click();

    await expect(
      page.getByText(/Rua Local.*Local Models|Rua Local.*本地模型/).first(),
    ).toBeVisible();
    await expect(
      page.getByText(/Inference engine|推理引擎/).first(),
    ).toBeVisible();
    await expect(page.getByText(/Powered by Llama\.cpp/).first()).toBeVisible();
    await expect(page.getByText(/Install|安装/).first()).toBeVisible();
    await expect(page.getByText(/Status|状态/).first()).toBeVisible();
  });
});
