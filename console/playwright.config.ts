import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/visual",
  fullyParallel: false,
  retries: 0,
  workers: 1,
  reporter: [["list"], ["html", { open: "never" }]],
  use: {
    baseURL: "http://127.0.0.1:8090",
    viewport: { width: 1440, height: 900 },

    actionTimeout: 10_000,
    navigationTimeout: 30_000,
  },
  expect: {
    toHaveScreenshot: {
      maxDiffPixels: 50,
      threshold: 0.15,
      animations: "disabled",
      caret: "hide",
    },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
