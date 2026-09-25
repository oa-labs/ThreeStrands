import { invoke } from "@tauri-apps/api/core";
import { applyExportablePreferences, readExportablePreferences, type ExportablePreferences } from "./userPreferences";

function isDesktop(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

const pendingWrites = new Set<Promise<void>>();

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
  const earlierWrites = [...pendingWrites];
  const write = (async () => {
    // Keep rapid setting changes in the order the user made them.
    if (earlierWrites.length) await Promise.allSettled(earlierWrites);
    await invoke<void>("update_synced_preferences", { preferences });
  })();
  pendingWrites.add(write);
  try {
    await write;
  } finally {
    pendingWrites.delete(write);
  }
}

/** Applies portable preferences synchronized from another device, if any. */
export async function pullSyncedPreferences(): Promise<boolean> {
  if (!isDesktop()) return false;
  // A status event can arrive while a settings change is still being written.
  // Read after that write, so the pull cannot restore the previous value.
  while (pendingWrites.size) await Promise.allSettled([...pendingWrites]);
  const beforePull = readExportablePreferences();
  // Older replicas may still contain theme, accent, and selectedAccountId.
  // Keep all three local even when reading a preference record produced
  // before this change.
  const preferences = await invoke<Partial<ExportablePreferences> | null>("synced_preferences");
  if (!preferences) return false;
  const local = readExportablePreferences();
  // A settings edit can happen while the native read is in flight. Preserve
  // those newer local values instead of applying the snapshot that started
  // before the edit (for example, turning Contact Enrichment back off).
  const changedDuringPull = (Object.keys(beforePull) as (keyof ExportablePreferences)[])
    .filter((key) => JSON.stringify(beforePull[key]) !== JSON.stringify(local[key]));
  const merged: ExportablePreferences = {
    ...local,
    ...preferences,
    theme: local.theme,
    accent: local.accent,
    selectedAccountId: local.selectedAccountId,
  };
  // Older devices may send an aiFeatures object without newer flags. An
  // omitted flag carries no change, so retain its value on this device.
  if (preferences.aiFeatures) {
    merged.aiFeatures = { ...local.aiFeatures, ...preferences.aiFeatures };
  }
  for (const key of changedDuringPull) Object.assign(merged, { [key]: local[key] });
  applyExportablePreferences(merged);
  return true;
}
