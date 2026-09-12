import { afterEach, describe, expect, it } from "vitest";
import {
  clearAiApiKey,
  DEFAULT_AI_FEATURES,
  isAiApiKeyConfigured,
  readAiFeatures,
  readAiProvider,
  saveAiFeatures,
  saveAiProvider,
  setAiApiKey,
} from "./aiSettings";

describe("AI provider preferences", () => {
  afterEach(() => {
    localStorage.clear();
  });

  it("defaults to no provider and restores a saved choice", () => {
    expect(readAiProvider()).toBe("none");
    saveAiProvider("openai");
    expect(readAiProvider()).toBe("openai");
  });

  it("ignores an invalid stored provider", () => {
    localStorage.setItem("dispatch.settings.ai.provider", "not-a-provider");
    expect(readAiProvider()).toBe("none");
  });

  it("defaults every feature flag to off and merges a partial saved value", () => {
    expect(readAiFeatures()).toEqual(DEFAULT_AI_FEATURES);
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, summarize: true });
    expect(readAiFeatures()).toEqual({ ...DEFAULT_AI_FEATURES, summarize: true });
  });
});

describe("AI API key storage (non-Tauri fallback)", () => {
  it("is not configured until a key is set, and clears back to unconfigured", async () => {
    expect(await isAiApiKeyConfigured()).toBe(false);
    await setAiApiKey("sk-demo-key");
    expect(await isAiApiKeyConfigured()).toBe(true);
    await clearAiApiKey();
    expect(await isAiApiKeyConfigured()).toBe(false);
  });

  it("treats a blank key the same as clearing it", async () => {
    await setAiApiKey("sk-demo-key");
    await setAiApiKey("   ");
    expect(await isAiApiKeyConfigured()).toBe(false);
  });
});
