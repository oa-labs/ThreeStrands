import { invoke } from "@tauri-apps/api/core";

export type AiProvider = "none" | "openai" | "anthropic" | "openrouter" | "fireworks" | "custom";

export type AiFeatureFlags = {
  draftAssist: boolean;
  summarize: boolean;
  classify: boolean;
};

export const AI_PROVIDER_OPTIONS: { value: AiProvider; label: string }[] = [
  { value: "none", label: "None" },
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "openrouter", label: "OpenRouter" },
  { value: "fireworks", label: "Fireworks" },
  { value: "custom", label: "Custom endpoint" },
];

export const DEFAULT_AI_FEATURES: AiFeatureFlags = {
  draftAssist: false,
  summarize: false,
  classify: false,
};

export const AI_MODEL_PLACEHOLDERS: Record<AiProvider, string> = {
  none: "",
  openai: "gpt-4o",
  anthropic: "claude-sonnet-5",
  openrouter: "openai/gpt-4o",
  fireworks: "accounts/fireworks/models/llama-v3p1-70b-instruct",
  custom: "model name",
};

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

const PROVIDER_KEY = "dispatch.settings.ai.provider";
const MODEL_KEY = "dispatch.settings.ai.model";
const ENDPOINT_KEY = "dispatch.settings.ai.endpoint";
const FEATURES_KEY = "dispatch.settings.ai.features";

export function readAiProvider(): AiProvider {
  try {
    const saved = localStorage.getItem(PROVIDER_KEY);
    if (
      saved === "none" ||
      saved === "openai" ||
      saved === "anthropic" ||
      saved === "openrouter" ||
      saved === "fireworks" ||
      saved === "custom"
    ) {
      return saved;
    }
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
      return { ...DEFAULT_AI_FEATURES, ...saved };
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
