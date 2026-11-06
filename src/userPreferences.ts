import { EMAIL_TEXT_COLORS, EMAIL_ZOOM, type EmailTextColors } from "./emailRenderingPolicy";
import { invoke } from "@tauri-apps/api/core";
import {
  readAiEndpoint,
  readAiFastModel,
  readAiFeatures,
  readAiModel,
  readAiProvider,
  saveAiEndpoint,
  saveAiFastModel,
  saveAiFeatures,
  saveAiModel,
  saveAiProvider,
  type AiFeatureFlags,
  type AiProvider,
} from "./aiSettings";
import { readAccent, saveAccent, type Accent } from "./accent";
import { readCalendarColors, saveCalendarColors, type CalendarColorMap } from "./calendarColors";
import { readFontScale, saveFontScale } from "./fontScale";
import {
  readShowStarredInbox, saveShowStarredInbox,
  readMarkReadOnArchive, saveMarkReadOnArchive,
  readMarkReadOnOpen, saveMarkReadOnOpen,
  readAutoReadDelaySeconds,
  readAvailabilityPreferences,
  readAvailabilityProfiles,
  readFontFamily,
  readEmailMinimumFontSize,
  readEmailTextColors,
  readEmailZoom,
  readLoadRemoteImages,
  readSelectedAccountId,
  saveAutoReadDelaySeconds,
  saveAvailabilityPreferences,
  saveAvailabilityProfiles,
  DEFAULT_AVAILABILITY_PROFILES,
  saveFontFamily,
  saveEmailMinimumFontSize,
  saveEmailTextColors,
  saveEmailZoom,
  saveLoadRemoteImages,
  saveSelectedAccountId,
} from "./settings";
import { readTheme, saveTheme, type Theme } from "./theme";
import { readWritingProfiles, saveWritingProfiles, type WritingProfileMap } from "./writingProfiles";
import type { AvailabilityPreferences, AvailabilityProfiles, MailProvider } from "./domain";

/**
 * The complete allowlist of webview-owned preferences that may cross devices.
 * Secrets, diagnostics, cached mail, window geometry, and transient UI layout
 * are intentionally absent. Native-owned account metadata, Split Inboxes, and
 * retention are added by the trusted layer when it creates the bundle.
 */
export type ExportablePreferences = {
  /** Earlier exports default to showing the automatic Starred tab. */
  showStarredInbox?: boolean;
  theme: Theme;
  accent: Accent;
  fontScale: number;
  fontFamily: string;
  /** Absent in exports made before email zoom; defaults to 100%. */
  emailZoom?: number;
  emailMinimumFontSize: number;
  /** Absent in exports made before email text colors; defaults to the contrast floor. */
  emailTextColors?: EmailTextColors;
  /** Earlier exports preserve unread status when archiving. */
  markReadOnArchive?: boolean;
  /** Earlier exports always mark opened conversations read. */
  markReadOnOpen?: boolean;
  autoReadDelaySeconds: number;
  loadRemoteImages: boolean;
  selectedAccountId: string | null;
  aiProvider: AiProvider;
  aiModel: string;
  /** Blank means every feature uses `aiModel`. */
  aiFastModel: string;
  aiEndpoint: string;
  aiFeatures: AiFeatureFlags;
  /** The shared timezone and slot increment, with the default profile's hours. */
  availabilityPreferences: AvailabilityPreferences;
  /** Optional on the wire: exports with only the default profile omit it. */
  availabilityProfiles?: AvailabilityProfiles;
  /** Optional on the wire: exports without colors omit it. */
  calendarColors?: CalendarColorMap;
  /** Optional on the wire: exports without writing profiles omit it. */
  writingProfiles?: WritingProfileMap;
};

export type SettingsImportResult = {
  preferences: ExportablePreferences;
  accountCount: number;
  splitInboxCount: number;
  contactCount: number;
  contactGroupCount: number;
  senderRuleCount: number;
};

export function readExportablePreferences(): ExportablePreferences {
  return {
    showStarredInbox: readShowStarredInbox(),
    theme: readTheme(),
    accent: readAccent(),
    fontScale: readFontScale(),
    fontFamily: readFontFamily(),
    emailZoom: readEmailZoom(),
    emailMinimumFontSize: readEmailMinimumFontSize(),
    emailTextColors: readEmailTextColors(),
    markReadOnArchive: readMarkReadOnArchive(),
    markReadOnOpen: readMarkReadOnOpen(),
    autoReadDelaySeconds: readAutoReadDelaySeconds(),
    loadRemoteImages: readLoadRemoteImages(),
    selectedAccountId: readSelectedAccountId(),
    aiProvider: readAiProvider(),
    aiModel: readAiModel(),
    aiFastModel: readAiFastModel(),
    aiEndpoint: readAiEndpoint(),
    aiFeatures: readAiFeatures(),
    availabilityPreferences: readAvailabilityPreferences(),
    availabilityProfiles: readAvailabilityProfiles(),
    calendarColors: readCalendarColors(),
    writingProfiles: readWritingProfiles(),
  };
}

export function applyExportablePreferences(preferences: ExportablePreferences): void {
  saveShowStarredInbox(preferences.showStarredInbox ?? true);
  saveTheme(preferences.theme);
  saveAccent(preferences.accent);
  saveFontScale(preferences.fontScale);
  saveFontFamily(preferences.fontFamily);
  saveEmailZoom(preferences.emailZoom ?? EMAIL_ZOOM.defaultPercent);
  saveEmailMinimumFontSize(preferences.emailMinimumFontSize ?? 0);
  saveEmailTextColors(preferences.emailTextColors ?? EMAIL_TEXT_COLORS.defaultMode);
  saveMarkReadOnArchive(preferences.markReadOnArchive ?? false);
  saveMarkReadOnOpen(preferences.markReadOnOpen ?? true);
  saveAutoReadDelaySeconds(preferences.autoReadDelaySeconds);
  saveLoadRemoteImages(preferences.loadRemoteImages);
  saveSelectedAccountId(preferences.selectedAccountId);
  saveAiProvider(preferences.aiProvider);
  saveAiModel(preferences.aiModel);
  saveAiFastModel(preferences.aiFastModel ?? "");
  saveAiEndpoint(preferences.aiEndpoint);
  saveAiFeatures(preferences.aiFeatures);
  saveAvailabilityPreferences(preferences.availabilityPreferences);
  // Absent from exports made before profiles, or with only the default profile.
  saveAvailabilityProfiles(preferences.availabilityProfiles ?? DEFAULT_AVAILABILITY_PROFILES);
  // Absent from exports made before calendar colors, or with none set.
  saveCalendarColors(preferences.calendarColors ?? {});
  // Absent from exports made before writing profiles, or with none written.
  saveWritingProfiles(preferences.writingProfiles ?? {});
}

export async function exportSettings(password: string): Promise<string | null> {
  return invoke("export_settings", {
    preferences: readExportablePreferences(),
    password,
  });
}

/** One account in a settings file, before the user picks where to set it up. */
export type SettingsImportAccount = {
  email: string;
  displayName: string | null;
  nickname: string | null;
  provider: MailProvider;
  /** Already connected on this device; importing never changes that. */
  setUpHere: boolean;
};

export type SettingsImportPreview = { accounts: SettingsImportAccount[] };

/** Chooses and decrypts a settings file without applying it. Resolves with
 * null when the file dialog is cancelled. */
export async function previewSettingsImport(password: string): Promise<SettingsImportPreview | null> {
  return invoke<SettingsImportPreview | null>("preview_settings_import", { password });
}

/** Applies the previewed file. Accounts in `notHere` are listed as set up on
 * other devices instead of waiting to be reconnected on this one. */
export async function applySettingsImport(notHere: string[]): Promise<SettingsImportResult> {
  const result = await invoke<SettingsImportResult>("apply_settings_import", { notHere });
  applyExportablePreferences(result.preferences);
  return result;
}

export async function cancelSettingsImport(): Promise<void> {
  await invoke("cancel_settings_import");
}
