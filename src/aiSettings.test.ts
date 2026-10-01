import { afterEach, describe, expect, it, vi } from "vitest";
import {
  clearAiApiKey,
  DEFAULT_AI_FEATURES,
  AI_MODEL_SUGGESTIONS,
  isAiApiKeyConfigured,
  readAiFeatures,
  readAiProvider,
  readAiRequestConfig,
  AI_MODEL_TIERS,
  saveAiEndpoint,
  saveAiFastModel,
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
    expect(() => readAiRequestConfig("summarizing", "summary")).toThrow("Choose an AI provider in AI settings before summarizing.");
  });

  it("falls back to the provider's default model and omits the endpoint for hosted providers", () => {
    saveAiProvider("openai");
    saveAiEndpoint("https://ignored.example.com");
    expect(readAiRequestConfig("summarizing", "summary")).toEqual({ provider: "openai", model: "gpt-4o", endpoint: null });

    saveAiModel("  gpt-4o-mini  ");
    expect(readAiRequestConfig("summarizing", "summary").model).toBe("gpt-4o-mini");
  });

  it("requires both a model and an endpoint for a custom provider", () => {
    saveAiProvider("custom");
    expect(() => readAiRequestConfig("drafting a reply", "replyDraft")).toThrow("Set a model in AI settings before drafting a reply.");

    saveAiModel("local-llama");
    saveAiEndpoint("   ");
    expect(() => readAiRequestConfig("drafting a reply", "replyDraft")).toThrow("Set an endpoint URL in AI settings before drafting a reply.");

    saveAiEndpoint(" http://localhost:8080/v1 ");
    expect(readAiRequestConfig("drafting a reply", "replyDraft")).toEqual({
      provider: "custom",
      model: "local-llama",
      endpoint: "http://localhost:8080/v1",
    });
  });

  it("sends reading and copying work to the fast model and reasoning work to the main model", () => {
    expect(AI_MODEL_TIERS).toEqual({
      summary: "fast",
      replyDraft: "fast",
      contactEnrichment: "fast",
      actionExtraction: "reasoning",
      brief: "reasoning",
      threadChat: "reasoning",
    });
    saveAiProvider("fireworks");
    saveAiModel("example/reasoning");
    saveAiFastModel("  example/fast  ");
    const modelFor = (use: keyof typeof AI_MODEL_TIERS) => readAiRequestConfig("testing", use).model;
    expect(["summary", "replyDraft", "contactEnrichment"].map((use) => modelFor(use as keyof typeof AI_MODEL_TIERS)))
      .toEqual(["example/fast", "example/fast", "example/fast"]);
    expect(["actionExtraction", "brief", "threadChat"].map((use) => modelFor(use as keyof typeof AI_MODEL_TIERS)))
      .toEqual(["example/reasoning", "example/reasoning", "example/reasoning"]);
  });

  it("uses the main model for every feature while the fast model is blank", () => {
    saveAiProvider("openai");
    saveAiFastModel("   ");
    expect(readAiRequestConfig("summarizing", "summary").model).toBe("gpt-4o");
    saveAiModel("gpt-4.1-mini");
    expect(readAiRequestConfig("summarizing", "summary").model).toBe("gpt-4.1-mini");
    expect(readAiRequestConfig("getting a brief", "brief").model).toBe("gpt-4.1-mini");
  });

  it("still requires a main model for a custom provider when only the fast model is set", () => {
    saveAiProvider("custom");
    saveAiEndpoint("http://localhost:8080/v1");
    saveAiFastModel("local-fast");
    expect(readAiRequestConfig("summarizing", "summary").model).toBe("local-fast");
    expect(() => readAiRequestConfig("getting a brief", "brief")).toThrow("Set a model in AI settings before getting a brief.");
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
