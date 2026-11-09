import { test, expect } from "./fixtures";
import type { APIRequestContext } from "@playwright/test";

type PromptFile = {
  filename: string;
  content?: string;
  modified_time?: string;
};

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

async function writePromptFile(
  request: APIRequestContext,
  baseURL: string | undefined,
  filename: string,
  content: string,
): Promise<void> {
  const response = await request.put(
    `${apiBase(baseURL)}/api/prompt-library/files/${encodeURIComponent(
      filename,
    )}`,
    { data: { content } },
  );
  expect(response.ok()).toBeTruthy();
}

async function readPromptFile(
  request: APIRequestContext,
  baseURL: string | undefined,
  filename: string,
): Promise<PromptFile> {
  const response = await request.get(
    `${apiBase(baseURL)}/api/prompt-library/files/${encodeURIComponent(
      filename,
    )}`,
  );
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as PromptFile;
}

async function readPromptContentIfPresent(
  request: APIRequestContext,
  baseURL: string | undefined,
  filename: string,
): Promise<string | undefined> {
  const response = await request.get(
    `${apiBase(baseURL)}/api/prompt-library/files/${encodeURIComponent(
      filename,
    )}`,
  );
  if (!response.ok()) return undefined;
  return ((await response.json()) as PromptFile).content;
}

async function deletePromptFile(
  request: APIRequestContext,
  baseURL: string | undefined,
  filename: string,
): Promise<void> {
  await request.delete(
    `${apiBase(baseURL)}/api/prompt-library/files/${encodeURIComponent(
      filename,
    )}`,
  );
}

test.describe("prompt library", () => {
  test("shared prompt library API and page render seeded Rust prompt files", async ({
    page,
    request,
    baseURL,
  }) => {
    const filename = `e2e_prompt_${Date.now()}.md`;
    const content = [
      "---",
      "description: e2e prompt library",
      "---",
      "# E2E Prompt Library",
      "",
      "Seeded by Playwright against the real Rust prompt-library API.",
    ].join("\n");

    await writePromptFile(request, baseURL, filename, content);
    try {
      const listResponse = await request.get(
        `${apiBase(baseURL)}/api/prompt-library/files`,
      );
      expect(listResponse.ok()).toBeTruthy();
      const files = (await listResponse.json()) as PromptFile[];
      expect(files.some((file) => file.filename === filename)).toBeTruthy();

      const subdirsResponse = await request.get(
        `${apiBase(baseURL)}/api/prompt-library/subdirs-info`,
      );
      expect(subdirsResponse.ok()).toBeTruthy();

      await page.goto("/prompt-library", { waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/Prompt Library|提示词库/).first(),
      ).toBeVisible();

      const search = page.getByPlaceholder(/Search prompts|搜索提示词/);
      await expect(search).toBeVisible();
      await search.fill(filename);

      await expect(page.getByText(filename).first()).toBeVisible();
      await page.getByText(filename).first().click();

      await expect(
        page.getByText(/E2E Prompt Library|e2e prompt library/).first(),
      ).toBeVisible();

      const fetched = await readPromptFile(request, baseURL, filename);
      expect(fetched).toEqual(
        expect.objectContaining({
          filename,
          content,
        }),
      );
    } finally {
      await deletePromptFile(request, baseURL, filename);
    }
  });

  test("prompt library page creates and saves a Rust prompt file", async ({
    page,
    request,
    baseURL,
  }) => {
    const stem = `e2e_created_prompt_${Date.now()}`;
    const filename = `${stem}.md`;
    const content = [
      "---",
      "description: created from prompt library page",
      "---",
      "# Created Prompt",
      "",
      "Saved through the owned Rust prompt-library editor.",
    ].join("\n");

    await deletePromptFile(request, baseURL, filename);

    try {
      await page.goto("/prompt-library", { waitUntil: "domcontentloaded" });
      await expect(
        page.getByText(/Prompt Library|提示词库/).first(),
      ).toBeVisible();

      await page.getByRole("button", { name: /Create|创建/ }).click();
      await expect
        .poll(() => page.evaluate(() => document.activeElement?.tagName))
        .toBe("INPUT");
      await page.keyboard.press("ControlOrMeta+A");
      await page.keyboard.type(stem);
      await page.keyboard.press("Enter");

      await page.locator("textarea").first().fill(content);
      await page
        .getByRole("button", { name: /Save|保存/ })
        .last()
        .click();

      await expect
        .poll(
          async () => readPromptContentIfPresent(request, baseURL, filename),
          { timeout: 10_000 },
        )
        .toBe(content);

      await page.getByPlaceholder(/Search prompts|搜索提示词/).fill(filename);
      await expect(page.getByText(filename).first()).toBeVisible();
    } finally {
      await deletePromptFile(request, baseURL, filename);
    }
  });
});
