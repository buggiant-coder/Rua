import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  retries: 0,
  workers: 1,
  reporter: [
    ["list"],
    ["html", { outputFolder: "playwright-report", open: "never" }],
  ],
  outputDir: process.env.RUA_PLAYWRIGHT_OUTPUT_DIR ?? "test-results",
  use: {
    baseURL: process.env.RUA_E2E_BASE_URL ?? "http://127.0.0.1:8090",
    viewport: { width: 1440, height: 900 },
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  expect: {
    timeout: 10_000,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "project-sidebar-webkit",
      testMatch: "project-resources-sidebar.spec.ts",
      use: { ...devices["Desktop Safari"] },
    },
  ],

  timeout: 180_000,
});
