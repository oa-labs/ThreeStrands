import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clearAiApiKey,
  DEFAULT_AI_FEATURES,
  AI_MODEL_SUGGESTIONS,
  isAiApiKeyConfigured,
  readAiFeatures,
  readAiProvider,
  readAiRequestConfig,
  saveAiEndpoint,
  saveAiFeatures,
  saveAiModel,
  saveAiProvider,
  setAiApiKey,
  testAiConnection,
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

  it("restores OpenRouter and Fireworks as first-class providers", () => {
    saveAiProvider("openrouter");
    expect(readAiProvider()).toBe("openrouter");
    saveAiProvider("fireworks");
    expect(readAiProvider()).toBe("fireworks");
  });

  it("offers provider-specific model suggestions", () => {
    expect(AI_MODEL_SUGGESTIONS.openai).toContain("gpt-4o-mini");
    expect(AI_MODEL_SUGGESTIONS.anthropic.length).toBeGreaterThan(0);
    expect(AI_MODEL_SUGGESTIONS.custom).toEqual([]);
  });

  it("ignores an invalid stored provider", () => {
    localStorage.setItem("threestrands.settings.ai.provider", "not-a-provider");
    expect(readAiProvider()).toBe("none");
  });

  it("requires a provider before building an AI request", () => {
    expect(() => readAiRequestConfig("summarizing")).toThrow("Choose an AI provider in AI settings before summarizing.");
  });

  it("falls back to the provider's default model and omits the endpoint for hosted providers", () => {
    saveAiProvider("openai");
    saveAiEndpoint("https://ignored.example.com");
    expect(readAiRequestConfig("summarizing")).toEqual({ provider: "openai", model: "gpt-4o", endpoint: null });

    saveAiModel("  gpt-4o-mini  ");
    expect(readAiRequestConfig("summarizing").model).toBe("gpt-4o-mini");
  });

  it("requires both a model and an endpoint for a custom provider", () => {
    saveAiProvider("custom");
    expect(() => readAiRequestConfig("drafting a reply")).toThrow("Set a model in AI settings before drafting a reply.");

    saveAiModel("local-llama");
    saveAiEndpoint("   ");
    expect(() => readAiRequestConfig("drafting a reply")).toThrow("Set an endpoint URL in AI settings before drafting a reply.");

    saveAiEndpoint(" http://localhost:8080/v1 ");
    expect(readAiRequestConfig("drafting a reply")).toEqual({
      provider: "custom",
      model: "local-llama",
      endpoint: "http://localhost:8080/v1",
    });
  });

  it("defaults every feature flag to off and merges a partial saved value", () => {
    expect(readAiFeatures()).toEqual(DEFAULT_AI_FEATURES);
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, summarize: true });
    expect(readAiFeatures()).toEqual({ ...DEFAULT_AI_FEATURES, summarize: true });
  });

  it("keeps contact enrichment disabled when restoring older feature settings",()=>{
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({draftAssist:true,summarize:false,actionExtraction:true}));
    expect(readAiFeatures().contactEnrichment).toBe(false);
  });
});

describe("AI API key storage (non-Tauri fallback)", () => {
  afterEach(async () => {
    await clearAiApiKey();
    vi.restoreAllMocks();
    localStorage.clear();
    sessionStorage.clear();
  });

  it("tracks whether a key is configured for the session without writing the key to web storage", async () => {
    const secret = "sk-demo-key-7f3a";
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    expect(await isAiApiKeyConfigured()).toBe(false);
    await setAiApiKey(secret);
    expect(await isAiApiKeyConfigured()).toBe(true);

    const stored = [localStorage, sessionStorage].flatMap((storage) =>
      Array.from({ length: storage.length }, (_, index) => {
        const key = storage.key(index) ?? "";
        return `${key}=${storage.getItem(key) ?? ""}`;
      }));
    expect(stored.some((entry) => entry.includes(secret))).toBe(false);
    expect(setItem.mock.calls.some((args) => args.some((value) => String(value).includes(secret)))).toBe(false);

    await clearAiApiKey();
    expect(await isAiApiKeyConfigured()).toBe(false);
  });

  it("treats a blank key the same as clearing it", async () => {
    await setAiApiKey("sk-demo-key");
    await setAiApiKey("   ");
    expect(await isAiApiKeyConfigured()).toBe(false);
  });

  it("keeps connection testing a no-op in the browser preview", async () => {
    await expect(testAiConnection("openai", "gpt-4o", "")).resolves.toBeUndefined();
  });
});
