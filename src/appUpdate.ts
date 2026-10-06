import { invokeWithPolicy, WAIT_FOR_NATIVE_COMPLETION } from "./invoke";

/** How this install can take an update; see `app_update.rs`. */
export type InstallMode = "inPlace" | "moveToApplications" | "download";

export interface AvailableUpdate {
  version: string;
  currentVersion: string;
  releaseUrl: string;
  installMode: InstallMode;
}

function isDesktop(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/** Resolves to the newer release, or null when this build is current. */
export async function checkForAppUpdate(): Promise<AvailableUpdate | null> {
  if (!isDesktop()) return null;
  return invokeWithPolicy<AvailableUpdate | null>("check_for_app_update", undefined, WAIT_FOR_NATIVE_COMPLETION);
}

/** Downloads, verifies, and installs the update the last check found, then relaunches. */
export async function installAppUpdate(): Promise<void> {
  return invokeWithPolicy<void>("install_app_update", undefined, WAIT_FOR_NATIVE_COMPLETION);
}
