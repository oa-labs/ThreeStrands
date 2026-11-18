import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 2 : 0,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:1420",
    // Wide enough for Mail's three columns; below 1360px the right column is a drawer.
    viewport: { width: 1440, height: 720 },
    trace: "on-first-retry",
  },
  webServer: {
    command: "node node_modules/vite/bin/vite.js --host 127.0.0.1",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !process.env.CI,
  },
  projects: [
    {
      name: "webkit-editor-shortcuts",
      testMatch: /(?:compose|notes)\.spec\.ts/,
      grep: /selection formatting shortcuts/,
      use: { browserName: "webkit" },
    },
    {
      name: "webkit-email-layout",
      testMatch: "email-rendering.spec.ts",
      grep: /fits the reader/,
      use: { browserName: "webkit" },
    },
    {
      // Real pointer drags behave differently in WebKit (Tauri's macOS web view): text selection and native drags cancel pointer events.
      name: "webkit-today-drag",
      testMatch: "today.spec.ts",
      grep: /drags a Planning Inbox item/,
      use: { browserName: "webkit" },
    },
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
});
