import { defineConfig } from "@playwright/test";

/**
 * Marketing screenshots, kept apart from `pnpm test:e2e`: runs the browser
 * preview against the fictional showcase dataset on its own port, so it never
 * reuses a dev server that is serving the default test data.
 */
const port = 1421;

export default defineConfig({
  testDir: "./tests/screenshots",
  fullyParallel: true,
  reporter: "list",
  timeout: 60_000,
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    browserName: "chromium",
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    locale: "en-US",
    timezoneId: "America/Los_Angeles",
  },
  webServer: {
    command: `node node_modules/vite/bin/vite.js --host 127.0.0.1 --port ${port}`,
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: false,
    env: { VITE_DEMO_DATASET: "showcase" },
  },
});
