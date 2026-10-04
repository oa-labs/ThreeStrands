import { afterEach, describe, expect, it, vi } from "vitest";
import type { UserConfig } from "vite";
import viteConfig from "../vite.config";

function resolve(command: "build" | "serve"): UserConfig {
  if (typeof viteConfig !== "function") throw new Error("vite.config.ts must export a config function");
  return viteConfig({ command, mode: command === "build" ? "production" : "development" }) as UserConfig;
}

describe("demo client build flag", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("leaves the demo client out of release builds", () => {
    vi.stubEnv("DEMO_CLIENT", "");
    expect(resolve("build").define?.__DEMO_CLIENT__).toBe("false");
  });

  it("includes the demo client in builds made with DEMO_CLIENT=1", () => {
    vi.stubEnv("DEMO_CLIENT", "1");
    expect(resolve("build").define?.__DEMO_CLIENT__).toBe("true");
  });

  it("keeps the demo client in the dev server used by e2e and screenshot runs", () => {
    vi.stubEnv("DEMO_CLIENT", "");
    expect(resolve("serve").define?.__DEMO_CLIENT__).toBe("true");
  });
});

describe("bundle splitting", () => {
  it("keeps large third-party libraries in their own chunks", () => {
    const output = resolve("build").build?.rolldownOptions?.output;
    const groups = (Array.isArray(output) ? output[0] : output)?.codeSplitting;
    const names = typeof groups === "object" && groups ? (groups.groups ?? []).map((group) => group.name) : [];
    expect(names).toEqual(expect.arrayContaining(["react", "sanitizer"]));
  });
});
