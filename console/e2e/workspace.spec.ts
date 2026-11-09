import { test, expect } from "./fixtures";
import type { APIRequestContext } from "@playwright/test";

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

async function putWorkspaceFile(
  request: APIRequestContext,
  baseURL: string | undefined,
  agentId: string,
  stem: string,
  content: string,
): Promise<void> {
  const response = await request.put(
    `${apiBase(baseURL)}/api/agents/${agentId}/files/${encodeURIComponent(
      stem,
    )}`,
    { data: { content } },
  );
  expect(response.ok()).toBeTruthy();
}

async function deleteWorkspaceFile(
  request: APIRequestContext,
  baseURL: string | undefined,
  agentId: string,
  filename: string,
): Promise<void> {
  await request.delete(
    `${apiBase(baseURL)}/api/agents/${agentId}/files/${encodeURIComponent(
      filename,
    )}`,
  );
}

async function getSystemPromptFiles(
  request: APIRequestContext,
  baseURL: string | undefined,
  agentId: string,
): Promise<string[]> {
  const response = await request.get(
    `${apiBase(baseURL)}/api/agents/${agentId}/system-prompt-files`,
  );
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as string[];
}

async function setSystemPromptFiles(
  request: APIRequestContext,
  baseURL: string | undefined,
  agentId: string,
  files: string[],
): Promise<void> {
  const response = await request.put(
    `${apiBase(baseURL)}/api/agents/${agentId}/system-prompt-files`,
    { data: files },
  );
  expect(response.ok()).toBeTruthy();
}

test.describe("workspace", () => {
  test("workspace page opens a file written through the real Rust API", async ({
    page,
    request,
    baseURL,
  }) => {
    const agentId = "default";
    const stem = `E2E_WORKSPACE_${Date.now()}`;
    const filename = `${stem}.md`;
    const content = "QwenPaw alignment workspace e2e";

    try {
      await putWorkspaceFile(request, baseURL, agentId, stem, content);

      await page.goto("/workspace", { waitUntil: "domcontentloaded" });
      const fileEntry = page.getByText(filename, { exact: true }).first();
      await expect(fileEntry).toBeVisible();

      await fileEntry.click();
      await expect(page.getByText(content)).toBeVisible();
    } finally {
      await deleteWorkspaceFile(request, baseURL, agentId, filename);
    }
  });

  test("workspace prompt enable toggle persists through the Rust system-prompt-files API", async ({
    page,
    request,
    baseURL,
  }) => {
    const agentId = "default";
    const stem = `E2E_TOGGLE_${Date.now()}`;
    const filename = `${stem}.md`;
    const content = "QwenPaw alignment workspace toggle e2e";
    const originalPromptFiles = await getSystemPromptFiles(
      request,
      baseURL,
      agentId,
    );

    try {
      await putWorkspaceFile(request, baseURL, agentId, stem, content);
      await setSystemPromptFiles(
        request,
        baseURL,
        agentId,
        originalPromptFiles.filter((item) => item !== filename),
      );

      await page.goto("/workspace", { waitUntil: "domcontentloaded" });
      const row = page
        .getByText(stem)
        .first()
        .locator("xpath=ancestor::div[contains(@class, 'fileItem')][1]");
      await expect(row).toBeVisible();

      await row.locator(".rua-toggle").first().click();
      await expect
        .poll(() => getSystemPromptFiles(request, baseURL, agentId), {
          timeout: 10_000,
        })
        .toContain(filename);

      await page.reload({ waitUntil: "domcontentloaded" });
      await expect(
        page
          .locator("div[class*='loadedPromptItem']")
          .filter({ hasText: stem }),
      ).toBeVisible();

      await page
        .locator("div[class*='loadedPromptItem']")
        .filter({ hasText: stem })
        .locator(".rua-toggle")
        .first()
        .click();
      await expect
        .poll(() => getSystemPromptFiles(request, baseURL, agentId), {
          timeout: 10_000,
        })
        .not.toContain(filename);
    } finally {
      await setSystemPromptFiles(
        request,
        baseURL,
        agentId,
        originalPromptFiles,
      );
      await deleteWorkspaceFile(request, baseURL, agentId, filename);
    }
  });

  test("workspace loaded prompt order persists through the Rust system-prompt-files API", async ({
    page,
    request,
    baseURL,
  }) => {
    const agentId = "default";
    const suffix = Date.now();
    const firstStem = `E2E_ORDER_A_${suffix}`;
    const secondStem = `E2E_ORDER_B_${suffix}`;
    const firstFile = `${firstStem}.md`;
    const secondFile = `${secondStem}.md`;
    const originalPromptFiles = await getSystemPromptFiles(
      request,
      baseURL,
      agentId,
    );

    try {
      await putWorkspaceFile(
        request,
        baseURL,
        agentId,
        firstStem,
        "QwenPaw alignment ordered prompt A",
      );
      await putWorkspaceFile(
        request,
        baseURL,
        agentId,
        secondStem,
        "QwenPaw alignment ordered prompt B",
      );

      await setSystemPromptFiles(request, baseURL, agentId, [
        secondFile,
        firstFile,
      ]);
      await expect
        .poll(() => getSystemPromptFiles(request, baseURL, agentId), {
          timeout: 10_000,
        })
        .toEqual([secondFile, firstFile]);

      await page.goto("/workspace", { waitUntil: "domcontentloaded" });
      const loadedNames = page.locator("div[class*='loadedPromptName']");
      await expect(loadedNames.nth(0)).toContainText(secondStem);
      await expect(loadedNames.nth(1)).toContainText(firstStem);

      await setSystemPromptFiles(request, baseURL, agentId, [
        firstFile,
        secondFile,
      ]);
      await page.reload({ waitUntil: "domcontentloaded" });
      await expect(loadedNames.nth(0)).toContainText(firstStem);
      await expect(loadedNames.nth(1)).toContainText(secondStem);
    } finally {
      await setSystemPromptFiles(
        request,
        baseURL,
        agentId,
        originalPromptFiles,
      );
      await deleteWorkspaceFile(request, baseURL, agentId, firstFile);
      await deleteWorkspaceFile(request, baseURL, agentId, secondFile);
    }
  });
});
