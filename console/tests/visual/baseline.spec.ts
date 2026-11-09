import { test, expect, Page } from "@playwright/test";

const ROUTES: Array<{ path: string; name: string }> = [
  { path: "/", name: "chat" },
  { path: "/agents", name: "agents" },
  { path: "/agent-config", name: "agent-config" },
  { path: "/skills", name: "skills" },
  { path: "/skill-pool", name: "skill-pool" },
  { path: "/tools", name: "tools" },
  { path: "/mcp", name: "mcp" },
  { path: "/workspace", name: "workspace" },
  { path: "/prompt-library", name: "prompt-library" },
  { path: "/shared-mcp", name: "shared-mcp" },
  { path: "/sessions", name: "sessions" },
  { path: "/channels", name: "channels" },
  { path: "/cron-jobs", name: "cron-jobs" },
  { path: "/heartbeat", name: "heartbeat" },
  { path: "/models", name: "models" },
  { path: "/environments", name: "environments" },
  { path: "/security", name: "security" },

  { path: "/voice-transcription", name: "voice-transcription" },
];

async function setTheme(page: Page, mode: "light" | "dark") {
  await page.addInitScript((m: string) => {
    try {
      window.localStorage.setItem("rua-theme", m);
    } catch {}
  }, mode);
}

async function gotoStable(page: Page, path: string) {
  await page.goto(path, { waitUntil: "networkidle" });

  await page.waitForLoadState("domcontentloaded");

  await page.evaluate(async () => {
    await document.fonts?.ready;
  });

  await page.waitForTimeout(300);
}

for (const mode of ["light", "dark"] as const) {
  test.describe(`visual ${mode}`, () => {
    test.beforeEach(async ({ page }) => {
      await setTheme(page, mode);
    });

    for (const route of ROUTES) {
      test(`${route.name}`, async ({ page }) => {
        await gotoStable(page, route.path);
        await expect(page).toHaveScreenshot(`${route.name}-${mode}.png`, {
          fullPage: true,
        });
      });
    }
  });
}
