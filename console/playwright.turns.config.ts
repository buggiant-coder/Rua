import { defineConfig } from "@playwright/test";
import composer from "./playwright.composer.config";

export default defineConfig({
  ...composer,
  testMatch: "owned-turn-residency.spec.ts",
  outputDir: "test-results/turns",
  use: { ...composer.use, baseURL: "http://127.0.0.1:5186" },
  webServer: {
    command: "npm run dev -- --host 127.0.0.1 --port 5186 --strictPort",
    url: "http://127.0.0.1:5186/@vite/client",
    reuseExistingServer: !process.env.CI,
  },
});
