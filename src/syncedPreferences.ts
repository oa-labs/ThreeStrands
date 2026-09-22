import { invoke } from "@tauri-apps/api/core";
import { applyExportablePreferences, readExportablePreferences, type ExportablePreferences } from "./userPreferences";

export type CloudProfile = {
  id: string;
  email: string;
  displayName?: string | null;
  avatarUrl?: string | null;
};

export type CloudAccountStatus = {
  configured: boolean;
  signedIn: boolean;
  profile?: CloudProfile | null;
  syncEntitled: boolean;
  enrollmentConfirmed: boolean;
  lastSuccessfulSync?: string | null;
  error?: string | null;
  pendingOperations: number;
  conflictCount: number;
};

export type CloudDevice = {
  id: string;
  name: string;
  createdAt: string;
  lastSeenAt: string;
  current: boolean;
};

export type CloudConflict = {
  id: string;
  entityType: string;
  entityId: string;
  currentVersion: number;
  overlappingFields: string[];
  cloudPayload: Record<string, unknown> | null;
  cloudDeleted: boolean;
  devicePatch: Record<string, unknown> | null;
  deviceDeleted: boolean;
  createdAt: string;
};

const browserStatus: CloudAccountStatus = {
  configured: false,
  signedIn: false,
  syncEntitled: false,
  enrollmentConfirmed: false,
  pendingOperations: 0,
  conflictCount: 0,
};

function isDesktop(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export async function cloudAccountStatus(): Promise<CloudAccountStatus> {
  return isDesktop() ? invoke("cloud_account_status") : browserStatus;
}

export async function cloudSignIn(): Promise<CloudAccountStatus> {
  return invoke("cloud_sign_in");
}

export async function cloudSignOut(): Promise<void> {
  return invoke("cloud_sign_out");
}

export async function cloudDeleteAccount(): Promise<void> {
  return invoke("cloud_delete_account");
}

export async function cloudDevices(): Promise<CloudDevice[]> {
  return invoke("cloud_devices");
}

export async function cloudRevokeDevice(id: string): Promise<void> {
  return invoke("cloud_revoke_device", { id });
}

export async function removeSyncedMailAccount(email: string): Promise<void> {
  return invoke("remove_synced_mail_account", { email });
}

export async function removeSyncedCalendarAccount(email: string): Promise<void> {
  return invoke("remove_synced_calendar_account", { email });
}

export async function confirmCloudEnrollment(): Promise<void> {
  const { selectedAccountId: _deviceNavigation, ...portable } = readExportablePreferences();
  return invoke("cloud_confirm_enrollment", { preferences: portable });
}

export async function retryCloudSync(): Promise<void> {
  return invoke("cloud_retry_sync");
}

export function queuePortablePreferences(): void {
  if (!isDesktop()) return;
  const { selectedAccountId: _deviceNavigation, ...preferences } = readExportablePreferences();
  void invoke("cloud_update_preferences", { preferences });
}

export async function pullCloudPreferences(): Promise<boolean> {
  if (!isDesktop()) return false;
  const preferences = await invoke<Omit<ExportablePreferences, "selectedAccountId"> | null>("cloud_synced_preferences");
  if (!preferences) return false;
  applyExportablePreferences({ ...preferences, selectedAccountId: readExportablePreferences().selectedAccountId });
  return true;
}

export async function cloudConflicts(): Promise<CloudConflict[]> {
  return invoke("cloud_conflicts");
}

export async function resolveCloudConflict(
  id: string,
  currentVersion: number,
  resolvedPayload: Record<string, unknown> | null,
  deleted = false,
): Promise<void> {
  return invoke("cloud_resolve_conflict", {
    id,
    request: { currentVersion, resolvedPayload, deleted },
  });
}
