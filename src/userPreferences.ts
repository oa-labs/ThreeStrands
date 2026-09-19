import { invoke } from "@tauri-apps/api/core";
import {
  readAiEndpoint,
  readAiFeatures,
  readAiModel,
  readAiProvider,
  saveAiEndpoint,
  saveAiFeatures,
  saveAiModel,
  saveAiProvider,
  type AiFeatureFlags,
  type AiProvider,
} from "./aiSettings";
import { readFontScale, saveFontScale } from "./fontScale";
import {
  readAutoReadDelaySeconds,
  readAvailabilityPreferences,
  readFontFamily,
  readLoadRemoteImages,
  readSelectedAccountId,
  saveAutoReadDelaySeconds,
  saveAvailabilityPreferences,
  saveFontFamily,
  saveLoadRemoteImages,
  saveSelectedAccountId,
} from "./settings";
import { readTheme, saveTheme, type Theme } from "./theme";
import type { AvailabilityPreferences } from "./domain";

/**
 * The complete allowlist of webview-owned preferences that may cross devices.
 * Secrets, diagnostics, cached mail, window geometry, and transient UI layout
 * are intentionally absent. Native-owned account metadata, Split Inboxes, and
 * retention are added by the trusted layer when it creates the bundle.
 */
export type ExportablePreferences = {
  theme: Theme;
  fontScale: number;
  fontFamily: string;
  autoReadDelaySeconds: number;
  loadRemoteImages: boolean;
  selectedAccountId: string | null;
  aiProvider: AiProvider;
  aiModel: string;
  aiEndpoint: string;
  aiFeatures: AiFeatureFlags;
  availabilityPreferences: AvailabilityPreferences;
};

export type SettingsImportResult = {
  preferences: ExportablePreferences;
  accountCount: number;
  splitInboxCount: number;
};

export function readExportablePreferences(): ExportablePreferences {
  return {
    theme: readTheme(),
    fontScale: readFontScale(),
    fontFamily: readFontFamily(),
    autoReadDelaySeconds: readAutoReadDelaySeconds(),
    loadRemoteImages: readLoadRemoteImages(),
    selectedAccountId: readSelectedAccountId(),
    aiProvider: readAiProvider(),
    aiModel: readAiModel(),
    aiEndpoint: readAiEndpoint(),
    aiFeatures: readAiFeatures(),
    availabilityPreferences: readAvailabilityPreferences(),
  };
}

export function applyExportablePreferences(preferences: ExportablePreferences): void {
  saveTheme(preferences.theme);
  saveFontScale(preferences.fontScale);
  saveFontFamily(preferences.fontFamily);
  saveAutoReadDelaySeconds(preferences.autoReadDelaySeconds);
  saveLoadRemoteImages(preferences.loadRemoteImages);
  saveSelectedAccountId(preferences.selectedAccountId);
  saveAiProvider(preferences.aiProvider);
  saveAiModel(preferences.aiModel);
  saveAiEndpoint(preferences.aiEndpoint);
  saveAiFeatures(preferences.aiFeatures);
  saveAvailabilityPreferences(preferences.availabilityPreferences);
}

export async function exportSettings(password: string): Promise<string | null> {
  return invoke("export_settings", {
    preferences: readExportablePreferences(),
    password,
  });
}

export async function importSettings(password: string): Promise<SettingsImportResult | null> {
  const result = await invoke<SettingsImportResult | null>("import_settings", { password });
  if (result) applyExportablePreferences(result.preferences);
  return result;
}
