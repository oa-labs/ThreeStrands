import { defineConfig } from "@playwright/test";

/**
 * Tests for the marketing site (`site/`). Builds it with the current
 * SITE_BASE / SITE_URL and serves the production output, so CI tests exactly
 * what it deploys.
 */
const port = 1424;
const base = process.env.SITE_BASE ?? "/";
const vite = "node node_modules/vite/bin/vite.js";

export default defineConfig({
  testDir: "./tests/site",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 2 : 0,
  reporter: "list",
  use: {
    baseURL: `http://127.0.0.1:${port}${base}`,
    browserName: "chromium",
    viewport: { width: 1440, height: 900 },
  },
  webServer: {
    command: `${vite} build --config site/vite.config.ts && ${vite} preview --config site/vite.config.ts --host 127.0.0.1`,
    url: `http://127.0.0.1:${port}${base}`,
    reuseExistingServer: false,
    timeout: 180_000,
  },
});
