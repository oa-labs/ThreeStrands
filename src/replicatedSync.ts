import { invoke } from "@tauri-apps/api/core";
import type { FrontierCandidate, FrontierConflict } from "./FrontierConflictEditor";

export type { FrontierCandidate, FrontierConflict } from "./FrontierConflictEditor";

export type ReplicatedSyncTransportStatus = {
  instanceId: string;
  kind: string;
  location: string;
  health: string;
  headDiscovery: boolean;
  pending: number;
  delivered: number;
  failed: number;
  lastSuccessAt?: string | null;
  lastError?: string | null;
  storageBytes?: number | null;
};

export type IpfsRpcProbeReport = {
  versionOk: boolean;
  mfsAvailable: boolean;
};

export type EnrollmentStatus =
  | { state: "notStarted" }
  | { state: "awaitingGrant"; requestId: string; fingerprint: string; createdAt: string }
  | { state: "awaitingConfirmation"; requestId: string; fingerprint: string; approverFingerprint: string }
  | { state: "enrolled"; deviceCount: number };

export type IncomingEnrollmentRequest = {
  requestId: string;
  fingerprint: string;
  createdAt: string;
};

export type DeviceRosterEntry = {
  deviceId: string;
  status: string;
  isSelf: boolean;
};

function isDesktop(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/** Whether replicated sync is active right now for this device: either the
 * `THREESTRANDS_REPLICATED_SYNC` dev/CI env-var override, or the user's own
 * "enable beta features" Settings toggle. The Settings section stays
 * visible either way, matching the existing Three Strands Account section's
 * "not configured" pattern, but only offers actions when this is true. */
export async function replicatedSyncEnabled(): Promise<boolean> {
  return isDesktop() ? invoke("replicated_sync_enabled") : false;
}

/** The persisted state of the "enable beta features" toggle itself, as
 * opposed to {@link replicatedSyncEnabled}'s combined (env-var-or-toggle)
 * check — this is what the checkbox in Settings should reflect. */
export async function replicatedSyncBetaEnabled(): Promise<boolean> {
  return isDesktop() ? invoke("replicated_sync_beta_enabled") : false;
}

export async function replicatedSyncSetBetaEnabled(on: boolean): Promise<void> {
  return invoke("replicated_sync_set_beta_enabled", { on });
}

export async function replicatedSyncEnrollmentStatus(): Promise<EnrollmentStatus> {
  return invoke("replicated_sync_enrollment_status");
}

export async function replicatedSyncPendingRequests(): Promise<IncomingEnrollmentRequest[]> {
  return invoke("replicated_sync_pending_requests");
}

export async function replicatedSyncDeviceRoster(): Promise<DeviceRosterEntry[]> {
  return invoke("replicated_sync_device_roster");
}

/** Starts a brand-new sync space on this device and returns the recovery
 * phrase. Shown to the user exactly once — it is never persisted anywhere,
 * on this device or any other, so losing it here means losing it. */
export async function replicatedSyncBeginGenesis(): Promise<string> {
  return invoke("replicated_sync_begin_genesis");
}

/** Publishes a signed enrollment request for this (new) device and returns
 * its fingerprint — the value to compare against what an approving device
 * displays for the same request. */
export async function replicatedSyncRequestEnrollment(): Promise<string> {
  return invoke("replicated_sync_request_enrollment");
}

export async function replicatedSyncApproveRequest(requestId: string): Promise<void> {
  return invoke("replicated_sync_approve_request", { requestId });
}

export async function replicatedSyncRejectRequest(requestId: string): Promise<void> {
  return invoke("replicated_sync_reject_request", { requestId });
}

/** The joining device's explicit action after visually comparing the
 * fingerprint shown here against the one the approving device showed. */
export async function replicatedSyncConfirmEnrollment(requestId: string): Promise<void> {
  return invoke("replicated_sync_confirm_enrollment", { requestId });
}

/** Rotates the active epoch, optionally revoking a device (identified by
 * its `deviceId` from {@link replicatedSyncDeviceRoster}) in the same step. */
export async function replicatedSyncRotateEpoch(revokeDeviceId: string | null): Promise<void> {
  return invoke("replicated_sync_rotate_epoch", { revokeDeviceId });
}

/** Joins an existing sync space using only a recovery phrase — no peer
 * device needs to be online. */
export async function replicatedSyncJoinWithRecoveryPhrase(phrase: string): Promise<void> {
  return invoke("replicated_sync_join_with_recovery_phrase", { phrase });
}

export async function replicatedSyncStatus(): Promise<ReplicatedSyncTransportStatus[]> {
  return isDesktop() ? invoke("replicated_sync_status") : [];
}

/** Opens a native folder picker and configures the selected folder as a
 * transport. Resolves to `null` if the user cancels the picker. */
export async function replicatedSyncAddFolder(): Promise<ReplicatedSyncTransportStatus | null> {
  return invoke("replicated_sync_add_folder");
}

/** Configures a user-supplied Kubo-compatible IPFS RPC endpoint (e.g.
 * Filebase) as a transport. The token, if given, never touches SQLite — it
 * goes straight to the OS keychain on the Rust side. */
export async function replicatedSyncAddIpfsRpc(
  baseUrl: string,
  token: string | null,
): Promise<ReplicatedSyncTransportStatus | null> {
  return invoke("replicated_sync_add_ipfs_rpc", { baseUrl, token });
}

/** Validates a candidate endpoint without persisting anything — the "test
 * connection" step Settings runs before letting the user enable a replica. */
export async function replicatedSyncProbeIpfsRpc(
  baseUrl: string,
  token: string | null,
): Promise<IpfsRpcProbeReport> {
  return invoke("replicated_sync_probe_ipfs_rpc", { baseUrl, token });
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
