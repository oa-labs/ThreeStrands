import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

if (process.env.VITEST) {
  // Date and Intl code must not pass or fail depending on the machine. Set
  // before workers start so they inherit it. New York observes DST, so
  // transition edges remain reachable in tests that construct them.
  process.env.TZ = "America/New_York";
  process.env.LANG = "en_US.UTF-8";
  process.env.LC_ALL = "en_US.UTF-8";
}

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: process.env.TAURI_ENV_DEBUG ? false : "oxc",
    sourcemap: true,
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    exclude: ["tests/e2e/**", "node_modules/**", "dist/**", ".delta/**/node_modules/**"],
  },
});
