import { invoke } from "@tauri-apps/api/core";
import type { FrontierCandidate, FrontierConflict } from "./FrontierConflictEditor";

export type { FrontierCandidate, FrontierConflict } from "./FrontierConflictEditor";

export type ReplicatedSyncTransportStatus = {
  instanceId: string;
  path: string;
  health: string;
  pending: number;
  delivered: number;
  failed: number;
  lastSuccessAt?: string | null;
  lastError?: string | null;
  storageBytes?: number | null;
};

function isDesktop(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/** Whether this build has the replicated-sync engine enabled at all (a
 * development/beta flag, off by default). The Settings section stays
 * visible either way, matching the existing Three Strands Account section's
 * "not configured" pattern, but only offers actions when this is true. */
export async function replicatedSyncEnabled(): Promise<boolean> {
  return isDesktop() ? invoke("replicated_sync_enabled") : false;
}

export async function replicatedSyncStatus(): Promise<ReplicatedSyncTransportStatus[]> {
  return isDesktop() ? invoke("replicated_sync_status") : [];
}

/** Opens a native folder picker and configures the selected folder as a
 * transport. Resolves to `null` if the user cancels the picker. */
export async function replicatedSyncAddFolder(): Promise<ReplicatedSyncTransportStatus | null> {
  return invoke("replicated_sync_add_folder");
}

export async function replicatedSyncRemoveTransport(instanceId: string, deleteData: boolean): Promise<void> {
  return invoke("replicated_sync_remove_transport", { instanceId, deleteData });
}

export async function replicatedSyncNow(): Promise<void> {
  return invoke("replicated_sync_now");
}

export async function replicatedSyncConflicts(): Promise<FrontierConflict[]> {
  return isDesktop() ? invoke("replicated_sync_conflicts") : [];
}

/** Resolves a field conflict: an ordinary local write naming the entire
 * current frontier as parents and carrying the chosen candidate's value —
 * see `Database::resolve_frontier_conflict` on the Rust side. */
export async function replicatedSyncResolveConflict(
  conflict: FrontierConflict,
  chosen: FrontierCandidate,
): Promise<void> {
  return invoke("replicated_sync_resolve_conflict", {
    entityType: conflict.entityType,
    entityId: conflict.entityId,
    field: conflict.field,
    operationId: chosen.operationId,
  });
}
