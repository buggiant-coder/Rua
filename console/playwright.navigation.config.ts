import { defineConfig } from "@playwright/test";
import composer from "./playwright.composer.config";

export default defineConfig({
  ...composer,
  testMatch: "navigation-sidebar.spec.ts",
  outputDir: "test-results/navigation-sidebar",
  use: {
    ...composer.use,
    baseURL: "http://127.0.0.1:5191",
    viewport: { width: 1440, height: 900 },
  },
  projects: composer.projects?.map((project) => ({
    ...project,
    use: { ...project.use, viewport: { width: 1440, height: 900 } },
  })),
  webServer: {
    command: "npm run dev -- --host 127.0.0.1 --port 5191 --strictPort",
    url: "http://127.0.0.1:5191/@vite/client",
    reuseExistingServer: false,
    env: { VITE_PROXY_TARGET: "http://127.0.0.1:9" },
  },
});
