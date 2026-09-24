import { invoke } from "@tauri-apps/api/core";
import { applyExportablePreferences, readExportablePreferences, type ExportablePreferences } from "./userPreferences";

function isDesktop(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/**
 * Records this device's portable preferences for cross-device sync. The
 * backend ignores the call unless replicated sync is active. Appearance and
 * device navigation stay on this device.
 */
export function queuePortablePreferences(): void {
  if (!isDesktop()) return;
  void queuePortablePreferencesAndWait();
}

/** Queues the current portable preference record and waits for native storage. */
export async function queuePortablePreferencesAndWait(): Promise<void> {
  if (!isDesktop()) return;
  const {
    selectedAccountId: _deviceNavigation,
    theme: _deviceTheme,
    accent: _deviceAccent,
    ...preferences
  } = readExportablePreferences();
  await invoke("update_synced_preferences", { preferences });
}

/** Applies portable preferences synchronized from another device, if any. */
export async function pullSyncedPreferences(): Promise<boolean> {
  if (!isDesktop()) return false;
  // Older replicas may still contain theme, accent, and selectedAccountId.
  // Keep all three local even when reading a preference record produced
  // before this change.
  const preferences = await invoke<Partial<ExportablePreferences> | null>("synced_preferences");
  if (!preferences) return false;
  const local = readExportablePreferences();
  applyExportablePreferences({
    ...local,
    ...preferences,
    theme: local.theme,
    accent: local.accent,
    selectedAccountId: local.selectedAccountId,
  });
  return true;
}
