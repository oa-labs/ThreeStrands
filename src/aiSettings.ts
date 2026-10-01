import { invoke } from "@tauri-apps/api/core";

export const AI_PROVIDERS = [
  { id: "none", label: "None", modelPlaceholder: "", modelSuggestions: [] },
  { id: "openai", label: "OpenAI", modelPlaceholder: "gpt-4o", modelSuggestions: ["gpt-4o", "gpt-4o-mini", "gpt-4.1-mini", "o3-mini"] },
  { id: "anthropic", label: "Anthropic", modelPlaceholder: "claude-sonnet-5", modelSuggestions: ["claude-sonnet-5", "claude-3-7-sonnet-latest", "claude-3-5-haiku-latest"] },
  { id: "openrouter", label: "OpenRouter", modelPlaceholder: "openai/gpt-4o", modelSuggestions: ["openai/gpt-4o", "anthropic/claude-3.5-sonnet", "google/gemini-2.0-flash-001"] },
  { id: "fireworks", label: "Fireworks", modelPlaceholder: "accounts/fireworks/models/llama-v3p1-70b-instructh", modelSuggestions: ["accounts/fireworks/models/glm-5p3-flash", "accounts/fireworks/models/deepseek-v4p1-flash"] },
  { id: "custom", label: "Custom endpoint", modelPlaceholder: "model name", modelSuggestions: [] },
] as const;

export type AiProvider = (typeof AI_PROVIDERS)[number]["id"];

export type AiFeatureFlags = {
  draftAssist: boolean;
  summarize: boolean;
  actionExtraction: boolean;
  contactEnrichment: boolean;
  /** Prepares the brief after the reader stays on a conversation. */
  proactiveBriefs: boolean;
  /** Limits proactive briefs to senders the user has emailed. */
  proactiveKnownSendersOnly: boolean;
  /** Lets the reader ask questions about the open conversation. */
  threadChat: boolean;
};

export const AI_PROVIDER_OPTIONS: { value: AiProvider; label: string }[] =
  AI_PROVIDERS.map(({ id, label }) => ({ value: id, label }));

export const DEFAULT_AI_FEATURES: AiFeatureFlags = {
  draftAssist: false,
  summarize: false,
  actionExtraction: false,
  contactEnrichment: false,
  proactiveBriefs: false,
  proactiveKnownSendersOnly: false,
  threadChat: false,
};

export const AI_MODEL_PLACEHOLDERS = Object.fromEntries(
  AI_PROVIDERS.map(({ id, modelPlaceholder }) => [id, modelPlaceholder]),
) as Record<AiProvider, string>;

export const AI_MODEL_SUGGESTIONS = Object.fromEntries(
  AI_PROVIDERS.map(({ id, modelSuggestions }) => [id, modelSuggestions]),
) as unknown as Record<AiProvider, readonly string[]>;

function isAiProvider(value: string | null): value is AiProvider {
  return AI_PROVIDERS.some(({ id }) => id === value);
}

/**
 * A blank model field falls back to a sensible default for every provider
 * except `custom`, where there's no way to guess a real model name — the
 * placeholder there ("model name") is just a hint, not a usable value.
 */
export function resolveAiModel(provider: AiProvider, model: string): string {
  const trimmed = model.trim();
  if (trimmed) return trimmed;
  return provider === "custom" ? "" : AI_MODEL_PLACEHOLDERS[provider];
}

export type AiRequestConfig = {
  provider: Exclude<AiProvider, "none">;
  model: string;
  /** Only set for the `custom` provider, where it is required. */
  endpoint: string | null;
};

/** The AI work the app sends to a provider, each served by one model tier. */
export type AiModelUse = "summary" | "replyDraft" | "contactEnrichment" | "actionExtraction" | "brief" | "threadChat";

/**
 * Which model serves each use. Reading and copying (summaries, reply drafts,
 * pulling facts out of a signature) gains little from reasoning and pays for
 * it in latency, so it goes to the fast model. Resolving dates and time zones,
 * choosing between a meeting and a task, and multi-step questions do benefit,
 * so they keep the main reasoning model.
 */
export const AI_MODEL_TIERS: Record<AiModelUse, "fast" | "reasoning"> = {
  summary: "fast",
  replyDraft: "fast",
  contactEnrichment: "fast",
  actionExtraction: "reasoning",
  brief: "reasoning",
  threadChat: "reasoning",
};

/**
 * The model for `use`. A blank fast model falls back to the reasoning model,
 * so a single configured model keeps serving every feature.
 */
export function resolveAiModelFor(provider: AiProvider, use: AiModelUse, model: string, fastModel: string): string {
  const reasoning = resolveAiModel(provider, model);
  return AI_MODEL_TIERS[use] === "fast" ? fastModel.trim() || reasoning : reasoning;
}

/**
 * Reads the saved provider settings for an AI request, throwing a message
 * that names `action` (e.g. "summarizing") when they are incomplete. `use`
 * picks the fast or reasoning model.
 */
export function readAiRequestConfig(action: string, use: AiModelUse): AiRequestConfig {
  const provider = readAiProvider();
  if (provider === "none") throw new Error(`Choose an AI provider in AI settings before ${action}.`);
  const model = resolveAiModelFor(provider, use, readAiModel(), readAiFastModel());
  if (!model) throw new Error(`Set a model in AI settings before ${action}.`);
  const endpoint = provider === "custom" ? readAiEndpoint().trim() : null;
  if (provider === "custom" && !endpoint) throw new Error(`Set an endpoint URL in AI settings before ${action}.`);
  return { provider, model, endpoint };
}

const PROVIDER_KEY = "threestrands.settings.ai.provider";
const MODEL_KEY = "threestrands.settings.ai.model";
const FAST_MODEL_KEY = "threestrands.settings.ai.fastModel";
const ENDPOINT_KEY = "threestrands.settings.ai.endpoint";
const FEATURES_KEY = "threestrands.settings.ai.features";
const PRICES_KEY = "threestrands.settings.ai.prices";

export function readAiProvider(): AiProvider {
  try {
    const saved = localStorage.getItem(PROVIDER_KEY);
    if (isAiProvider(saved)) return saved;
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return "none";
}

export function saveAiProvider(value: AiProvider): void {
  try {
    localStorage.setItem(PROVIDER_KEY, value);
  } catch {
    // The choice still applies for this session when storage is unavailable.
  }
}

export function readAiModel(): string {
  try {
    return localStorage.getItem(MODEL_KEY) ?? "";
  } catch {
    return "";
  }
}

export function saveAiModel(value: string): void {
  try {
    localStorage.setItem(MODEL_KEY, value);
  } catch {
    // Ignored; see readAiModel.
  }
}

/** The optional fast model; blank means every feature uses the main model. */
export function readAiFastModel(): string {
  try {
    return localStorage.getItem(FAST_MODEL_KEY) ?? "";
  } catch {
    return "";
  }
}

export function saveAiFastModel(value: string): void {
  try {
    localStorage.setItem(FAST_MODEL_KEY, value);
  } catch {
    // Ignored; see readAiFastModel.
  }
}

export function readAiEndpoint(): string {
  try {
    return localStorage.getItem(ENDPOINT_KEY) ?? "";
  } catch {
    return "";
  }
}

export function saveAiEndpoint(value: string): void {
  try {
    localStorage.setItem(ENDPOINT_KEY, value);
  } catch {
    // Ignored; see readAiEndpoint.
  }
}

export function readAiFeatures(): AiFeatureFlags {
  try {
    const saved = JSON.parse(localStorage.getItem(FEATURES_KEY) ?? "null") as Partial<AiFeatureFlags> | null;
    if (saved && typeof saved === "object") {
      return {
        draftAssist: saved.draftAssist === true,
        summarize: saved.summarize === true,
        actionExtraction: saved.actionExtraction === true,
        contactEnrichment: saved.contactEnrichment === true,
        proactiveBriefs: saved.proactiveBriefs === true,
        proactiveKnownSendersOnly: saved.proactiveKnownSendersOnly === true,
        threadChat: saved.threadChat === true,
      };
    }
  } catch {
    // Ignored; fall through to the default below.
  }
  return { ...DEFAULT_AI_FEATURES };
}

export function saveAiFeatures(value: AiFeatureFlags): void {
  try {
    localStorage.setItem(FEATURES_KEY, JSON.stringify(value));
  } catch {
    // Ignored; see readAiFeatures.
  }
}

/** A model's price in US dollars per million tokens, as the user entered it. */
export type AiModelPrice = { inputPerMillion: number; outputPerMillion: number };

export const aiPriceKey = (provider: AiProvider, model: string) => `${provider}:${model}`;

/**
 * User-entered prices keyed by `aiPriceKey`. They stay on this device: they
 * are an estimate aid, not part of the portable preference allowlist.
 */
export function readAiPrices(): Record<string, AiModelPrice> {
  try {
    const saved = JSON.parse(localStorage.getItem(PRICES_KEY) ?? "null") as Record<string, Partial<AiModelPrice>> | null;
    if (!saved || typeof saved !== "object") return {};
    const valid = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0;
    return Object.fromEntries(Object.entries(saved).flatMap(([key, price]) =>
      price && valid(price.inputPerMillion) && valid(price.outputPerMillion)
        ? [[key, { inputPerMillion: price.inputPerMillion, outputPerMillion: price.outputPerMillion }]]
        : []));
  } catch {
    return {};
  }
}

/** Saves or, with `null`, clears the price for one provider and model. */
export function saveAiPrice(provider: AiProvider, model: string, price: AiModelPrice | null): void {
  const prices = readAiPrices();
  if (price) prices[aiPriceKey(provider, model)] = price;
  else delete prices[aiPriceKey(provider, model)];
  try {
    localStorage.setItem(PRICES_KEY, JSON.stringify(prices));
  } catch {
    // Ignored; see readAiFeatures.
  }
}

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

// The API key is a secret: it must never be written to localStorage or the mail
// SQLite database. The Tauri build stores it in the OS keychain (see
// src-tauri/src/ai.rs), the same way the Google OAuth token is stored. Outside
// Tauri (browser preview, tests) it only ever lives in memory for the session.
let demoKeyConfigured = false;

export async function isAiApiKeyConfigured(): Promise<boolean> {
  if (!isTauri()) return demoKeyConfigured;
  return invoke("ai_api_key_configured");
}

export async function setAiApiKey(key: string): Promise<void> {
  if (!isTauri()) {
    demoKeyConfigured = key.trim().length > 0;
    return;
  }
  await invoke("set_ai_api_key", { key });
}

export async function clearAiApiKey(): Promise<void> {
  return setAiApiKey("");
}

export async function testAiConnection(
  provider: AiProvider,
  model: string,
  endpoint: string,
): Promise<void> {
  if (!isTauri()) return;
  await invoke("ai_test_connection", {
    provider,
    model,
    endpoint: endpoint.trim() || null,
  });
}
