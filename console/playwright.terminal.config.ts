import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/terminal-canary",
  fullyParallel: false,
  retries: 0,
  workers: 1,
  reporter: [["list"]],
  outputDir: "test-results/terminal-compat",
  use: {
    baseURL: "http://127.0.0.1:4177",
    viewport: { width: 900, height: 600 },
    actionTimeout: 10_000,
    navigationTimeout: 20_000,
    trace: "off",
    screenshot: "off",
    video: "off",
  },
  webServer: {
    command: "npm run dev -- --host 127.0.0.1 --port 4177 --strictPort",
    url: "http://127.0.0.1:4177/tests/terminal-canary/",
    reuseExistingServer: false,
    timeout: 30_000,
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "mobile-chromium",
      use: { ...devices["Pixel 7"] },
    },
  ],
  timeout: 30_000,
});
