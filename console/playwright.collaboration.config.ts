import { defineConfig } from "@playwright/test";
import composer from "./playwright.composer.config";

export default defineConfig({
  ...composer,
  testMatch: "collaboration-drawer.spec.ts",
  outputDir: "test-results/collaboration-drawer",
  use: { ...composer.use, baseURL: "http://127.0.0.1:5189" },
  webServer: {
    command: "npm run dev -- --host 127.0.0.1 --port 5189 --strictPort",
    url: "http://127.0.0.1:5189/@vite/client",
    reuseExistingServer: false,
  },
});
