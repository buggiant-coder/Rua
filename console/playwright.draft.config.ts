import { defineConfig } from "@playwright/test";
import composer from "./playwright.composer.config";

export default defineConfig({
  ...composer,
  testMatch: "agent-draft-lifecycle.spec.ts",
  outputDir: "test-results/draft-lifecycle",
});
