import { invoke } from "@tauri-apps/api/core";
import { applyExportablePreferences, readExportablePreferences, type ExportablePreferences } from "./userPreferences";

function isDesktop(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/**
 * Records this device's portable preferences for cross-device sync. The
 * backend ignores the call unless replicated sync is active. Device
 * navigation never leaves the device.
 */
export function queuePortablePreferences(): void {
  if (!isDesktop()) return;
  const { selectedAccountId: _deviceNavigation, ...preferences } = readExportablePreferences();
  void invoke("update_synced_preferences", { preferences });
}

/** Applies portable preferences synchronized from another device, if any. */
export async function pullSyncedPreferences(): Promise<boolean> {
  if (!isDesktop()) return false;
  const preferences = await invoke<Omit<ExportablePreferences, "selectedAccountId"> | null>("synced_preferences");
  if (!preferences) return false;
  applyExportablePreferences({ ...preferences, selectedAccountId: readExportablePreferences().selectedAccountId });
  return true;
}
