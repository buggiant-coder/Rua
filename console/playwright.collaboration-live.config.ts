import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  testMatch: "collaboration-live.spec.ts",
  workers: 1,
  retries: 0,
  reporter: "list",
  outputDir: "test-results/collaboration-live",
  timeout: 60_000,
  use: {
    baseURL: "http://127.0.0.1:5191",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "npm run dev -- --host 127.0.0.1 --port 5191 --strictPort",
    url: "http://127.0.0.1:5191/@vite/client",
    reuseExistingServer: false,
  },
});
