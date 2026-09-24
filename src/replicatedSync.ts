import { invoke } from "@tauri-apps/api/core";
import type { FrontierCandidate, FrontierConflict } from "./FrontierConflictEditor";

export type { FrontierCandidate, FrontierConflict } from "./FrontierConflictEditor";

/** Every connector kind the native side can configure. */
export type ConnectorKind = "folder" | "ipfs_rpc" | "s3";

export type ReplicatedSyncTransportStatus = {
  instanceId: string;
  kind: ConnectorKind;
  /** The user-chosen name, if any. */
  label?: string | null;
  location: string;
  /** Whether "delete files and disconnect" can remove this connector's
   * synchronized data (folders and S3 buckets; not IPFS pins). */
  supportsDeleteData: boolean;
  /** An S3 connector's non-secret settings, for re-testing replacement
   * credentials. Never includes credentials. */
  s3Config?: S3ConnectorConfig | null;
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
  headDiscoveryAvailable: boolean;
};

/** The non-secret settings of an S3-compatible connector. Mirrors
 * `S3Config` in `s3_transport.rs`, which validates every field. */
export type S3ConnectorConfig = {
  /** `https://…`; `http://` only for this machine (e.g. local MinIO). */
  endpoint: string;
  /** Use `"auto"` where the provider says so (e.g. Cloudflare R2). */
  region: string;
  bucket: string;
  /** An optional folder inside the bucket. */
  prefix?: string;
  /** Required for IP-address or localhost endpoints. */
  pathStyle?: boolean;
  label?: string | null;
};

/** An S3 access key. Sent to the native side once, stored only in the OS
 * keychain, and never returned to the frontend. */
export type S3Credentials = {
  accessKeyId: string;
  secretAccessKey: string;
  sessionToken?: string | null;
};

/** Replacement credentials for an existing connector, tagged by kind. */
export type ConnectorCredentials =
  | ({ kind: "s3" } & S3Credentials)
  | { kind: "ipfs_rpc"; token: string };

/** What "Test connection" found for a candidate S3 connector. Each check
 * after the first failure stays `false`. */
export type S3ConnectionTest = {
  /** Whether the endpoint answered at all. */
  reachable: boolean;
  canList: boolean;
  canWrite: boolean;
  canRead: boolean;
  canDelete: boolean;
  /** `true` when bucket versioning keeps old versions of deleted and
   * replaced files; `null` when the key can't read that setting. */
  versioningEnabled?: boolean | null;
  /** The first failure, in plain language. */
  error?: string | null;
  /** Whether this bucket and prefix already hold a sync group; `null` when
   * the key couldn't list and read, so nothing could be checked. */
  spacePresence?: SyncSpacePresence | null;
};

/** Join-code lifetimes Settings offers, in hours. The native side accepts
 * any whole number from 1 to 168 (`MIN_JOIN_CODE_HOURS`/`MAX_JOIN_CODE_HOURS`
 * in `join_codes.rs`). */
export const JOIN_CODE_LIFETIME_HOURS = [1, 24, 168] as const;

/** One connector to include in a new join code. */
export type JoinCodeConnectorChoice = { instanceId: string; includeCredentials: boolean };

/** A join code this device created. Never includes the code text, which
 * this device doesn't keep. */
export type OutstandingJoinCode = {
  invitationCid: string;
  createdAt: string;
  expiresAt: string;
  status: "open" | "redeemed" | "expired" | "cancelled";
  redeemedByDeviceId?: string | null;
  redeemedByName?: string | null;
  /** Attempts refused because the code was already used, expired, or cancelled. */
  rejectedAttempts: number;
};

export type JoinCodeConnectorPreview = {
  index: number;
  kind: string;
  /** `false` for a connector kind this app version doesn't know; skipped when joining. */
  supported: boolean;
  location: string;
  label?: string | null;
  credentialsIncluded: boolean;
  /** A shared folder: this device must choose its own copy. */
  needsFolder: boolean;
  folderName?: string | null;
  /** Credentials were left out of the code and this connector needs them. */
  needsCredentials: boolean;
};

/** What a pasted join code contains, before anything is saved. */
export type JoinCodePreview = {
  inviterName: string;
  expiresAt: string;
  /** By this device's clock. */
  expired: boolean;
  connectors: JoinCodeConnectorPreview[];
};

export type JoinFolderChoice = { connectorIndex: number; path: string };
export type JoinCredentialsChoice = { connectorIndex: number; credentials: ConnectorCredentials };

/** `joined`: a device joined with a join code. `rejectedAttempt`: a device
 * tried one of this device's codes after it was used, expired, or cancelled. */
export type JoinCodeNotice = {
  redemptionCid: string;
  kind: "joined" | "rejectedAttempt";
  deviceId: string;
  deviceName: string;
  inviterDeviceId?: string | null;
  inviterName?: string | null;
  at: string;
};

export type EnrollmentStatus =
  | { state: "notStarted" }
  | { state: "awaitingGrant"; requestId: string; fingerprint: string; createdAt: string }
  | { state: "awaitingConfirmation"; requestId: string; fingerprint: string; approverFingerprint: string }
  | { state: "rejected"; requestId: string; fingerprint: string }
  | {
      state: "enrolled";
      deviceCount: number;
      /** The inviter's name while this device, having joined with a join
       * code, waits for that device to finish admitting it. */
      awaitingAdmissionFrom?: string | null;
    };

export type IncomingEnrollmentRequest = {
  requestId: string;
  deviceId?: string | null;
  fingerprint: string;
  createdAt: string;
};

export type DeviceRosterEntry = {
  deviceId: string;
  status: string;
  isSelf: boolean;
  /** The shared device name, synchronized to the other devices. */
  label?: string | null;
  /** When this device last recorded (itself) or received (a peer) a change
   * from that device — not a liveness signal. */
  lastChangeAt?: string | null;
  /** Whether this device joined the group with a join code. */
  joinedWithJoinCode?: boolean;
};

/** Mirrors `MAX_DEVICE_LABEL_CHARS` in `enrollment.rs`, which enforces it. */
export const MAX_DEVICE_LABEL_CHARS = 60;

export type RecoveryPhraseCheck = {
  wordCount: number;
  unknownWordPositions: number[];
  valid: boolean;
};

function isDesktop(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/** Whether replicated sync is active right now for this device: either the
 * `THREESTRANDS_REPLICATED_SYNC` dev/CI env-var override, or the user's own
 * "enable beta features" Settings toggle. The Settings section stays
 * visible either way, but only offers actions when this is true. */
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

/** Whether the configured transports already hold a sync space: `unknown`
 * when nothing is configured or a transport could not be scanned fully. */
/** `legacy`: a group created by an earlier, incompatible test build (or a
 * shared folder that hasn't finished syncing one). It can't be joined or
 * created over. */
export type SyncSpacePresence = "existing" | "none" | "unknown" | "legacy";

export async function replicatedSyncInspectSpace(): Promise<SyncSpacePresence> {
  return invoke("replicated_sync_inspect_space");
}

/** Starts a brand-new sync space on this device and returns the recovery
 * phrase. Shown to the user exactly once — it is never persisted anywhere,
 * on this device or any other, so losing it here means losing it. The
 * native side refuses when a configured transport already holds a space,
 * unless `allowExistingSpace` records the user's explicit choice to start
 * a separate one. */
export async function replicatedSyncBeginGenesis(allowExistingSpace = false): Promise<string> {
  return invoke("replicated_sync_begin_genesis", { allowExistingSpace });
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

/** Sets the shared name for a device; blank restores this device's hostname or clears a peer name. */
export async function replicatedSyncSetDeviceLabel(deviceId: string, label: string): Promise<void> {
  return invoke("replicated_sync_set_device_label", { deviceId, label });
}

/** Leaves the sync space on this device only: forgets its keys and
 * replication history, keeps local data and sync locations. */
export async function replicatedSyncLeave(): Promise<void> {
  return invoke("replicated_sync_leave");
}

/** Word-by-word validity of a (possibly partial) recovery phrase. */
export async function replicatedSyncCheckRecoveryPhrase(phrase: string): Promise<RecoveryPhraseCheck> {
  return invoke("replicated_sync_check_recovery_phrase", { phrase });
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

/** Tests a candidate S3 connector without saving anything. Rejects only
 * when a field is invalid before any request is made. */
export async function replicatedSyncProbeS3(config: S3ConnectorConfig, credentials: S3Credentials): Promise<S3ConnectionTest> {
  return invoke("replicated_sync_probe_s3", { config, credentials });
}

/** Saves an S3 connector: its settings in the app database, its access key
 * in the OS keychain only. */
export async function replicatedSyncAddS3(
  config: S3ConnectorConfig,
  credentials: S3Credentials,
): Promise<ReplicatedSyncTransportStatus | null> {
  return invoke("replicated_sync_add_s3", { config, credentials });
}

/** Renames a connector and/or replaces its credentials, keeping what it has
 * already synced. Omit `label` to keep the name; pass `""` to clear it. */
export async function replicatedSyncUpdateConnector(
  instanceId: string,
  changes: { label?: string; credentials?: ConnectorCredentials },
): Promise<ReplicatedSyncTransportStatus | null> {
  return invoke("replicated_sync_update_connector", {
    instanceId,
    label: changes.label ?? null,
    credentials: changes.credentials ?? null,
  });
}

/** Creates a join code for another device. The returned text is shown once
 * and never stored on this device. */
export async function replicatedSyncCreateJoinCode(
  expiresInHours: number,
  connectors: JoinCodeConnectorChoice[],
): Promise<string> {
  return invoke("replicated_sync_create_join_code", { expiresInHours, connectors });
}

export async function replicatedSyncListJoinCodes(): Promise<OutstandingJoinCode[]> {
  return isDesktop() ? invoke("replicated_sync_list_join_codes") : [];
}

/** Cancels an open join code; this rotates the group's keys. */
export async function replicatedSyncCancelJoinCode(invitationCid: string): Promise<void> {
  return invoke("replicated_sync_cancel_join_code", { invitationCid });
}

/** Parses pasted join code text without saving or contacting anything.
 * Rejects with a message written for the person pasting it. */
export async function replicatedSyncPreviewJoinCode(code: string): Promise<JoinCodePreview> {
  return invoke("replicated_sync_preview_join_code", { code });
}

/** Opens the native folder picker for a join code's shared folder.
 * Resolves to `null` if the user cancels. */
export async function replicatedSyncPickJoinFolder(): Promise<string | null> {
  return invoke("replicated_sync_pick_join_folder");
}

/** Joins the sync group a join code invites this device to, setting up its
 * connectors here. */
export async function replicatedSyncJoinWithCode(
  code: string,
  choices: { folders?: JoinFolderChoice[]; credentials?: JoinCredentialsChoice[] } = {},
): Promise<void> {
  return invoke("replicated_sync_join_with_code", {
    code,
    folders: choices.folders ?? [],
    credentials: choices.credentials ?? [],
  });
}

export async function replicatedSyncJoinCodeNotices(): Promise<JoinCodeNotice[]> {
  return isDesktop() ? invoke("replicated_sync_join_code_notices") : [];
}

export async function replicatedSyncDismissJoinCodeNotice(redemptionCid: string): Promise<void> {
  return invoke("replicated_sync_dismiss_join_code_notice", { redemptionCid });
}

/** Whether this update reset sync because the device belonged to a group
 * created by an earlier, incompatible test build. */
export async function replicatedSyncProtocolResetNotice(): Promise<boolean> {
  return isDesktop() ? invoke("replicated_sync_protocol_reset_notice") : false;
}

export async function replicatedSyncDismissProtocolResetNotice(): Promise<void> {
  return invoke("replicated_sync_dismiss_protocol_reset_notice");
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

/** Removes a mail account on every enrolled device, not just this one. */
export async function removeSyncedMailAccount(email: string): Promise<void> {
  return invoke("remove_synced_mail_account", { email });
}

/** Removes a calendar account on every enrolled device, not just this one. */
export async function removeSyncedCalendarAccount(email: string): Promise<void> {
  return invoke("remove_synced_calendar_account", { email });
}
