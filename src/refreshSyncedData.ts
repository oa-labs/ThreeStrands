import { replicatedSyncEnabled, replicatedSyncNow } from "./replicatedSync";

/** Manual refresh checks peers when device sync is enabled; local-only use still works. */
export async function refreshSyncedData(): Promise<void> {
  if (await replicatedSyncEnabled()) await replicatedSyncNow();
}
