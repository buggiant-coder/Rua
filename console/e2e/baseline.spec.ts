import { test, expect, goChat } from "./fixtures";

test.describe("baseline", () => {
  test("backend serves /api/version (or returns < 500)", async ({
    request,
    baseURL,
  }) => {
    const base = baseURL ?? "http://127.0.0.1:8090";
    const candidates = ["/api/version", "/api/agents", "/"];
    const results: { url: string; status: number | string }[] = [];
    for (const path of candidates) {
      try {
        const r = await request.get(`${base}${path}`, { timeout: 5_000 });
        results.push({ url: path, status: r.status() });
      } catch (e) {
        results.push({ url: path, status: `err: ${(e as Error).message}` });
      }
    }

    const okHit = results.find(
      (r) => typeof r.status === "number" && (r.status as number) < 500,
    );
    expect(okHit, JSON.stringify(results, null, 2)).toBeTruthy();
  });

  test("Landing mounts the Agent Chat input textarea", async ({ page }) => {
    await goChat(page);
    const ta = page.locator(".rua-sender textarea, textarea").first();
    await expect(ta).toBeVisible();
  });
});
