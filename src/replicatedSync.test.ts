import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import {
  replicatedSyncAddFolder,
  replicatedSyncAddIpfsRpc,
  replicatedSyncApproveRequest,
  replicatedSyncBeginGenesis,
  replicatedSyncBetaEnabled,
  replicatedSyncConfirmEnrollment,
  replicatedSyncConflicts,
  replicatedSyncDeviceRoster,
  replicatedSyncEnabled,
  replicatedSyncEnrollmentStatus,
  replicatedSyncInspectSpace,
  replicatedSyncCheckRecoveryPhrase,
  replicatedSyncLeave,
  replicatedSyncSetDeviceLabel,
  replicatedSyncJoinWithRecoveryPhrase,
  replicatedSyncNow,
  replicatedSyncPendingRequests,
  replicatedSyncProbeIpfsRpc,
  replicatedSyncProbeS3,
  replicatedSyncAddS3,
  replicatedSyncUpdateConnector,
  replicatedSyncRejectRequest,
  replicatedSyncRemoveTransport,
  replicatedSyncResolveConflict,
  replicatedSyncRotateEpoch,
  replicatedSyncRequestEnrollment,
  replicatedSyncSetBetaEnabled,
  replicatedSyncStatus,
  removeSyncedCalendarAccount,
  removeSyncedMailAccount,
  type FrontierConflict,
  type S3ConnectorConfig,
  type S3Credentials,
} from "./replicatedSync";

describe("replicated sync invoke wrappers", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });

  it("reports disabled outside a desktop build without invoking anything", async () => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    expect(await replicatedSyncEnabled()).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("checks whether the engine is enabled in this build", async () => {
    vi.mocked(invoke).mockResolvedValue(true);
    expect(await replicatedSyncEnabled()).toBe(true);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_enabled");
  });

  it("fetches transport status", async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    await replicatedSyncStatus();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_status");
  });

  it("adds a folder with no arguments beyond the command", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    expect(await replicatedSyncAddFolder()).toBeNull();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_add_folder");
  });

  it("adds an IPFS RPC endpoint with the base URL and token", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    await replicatedSyncAddIpfsRpc("https://rpc.filebase.io", "secret-token");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_add_ipfs_rpc", {
      baseUrl: "https://rpc.filebase.io",
      token: "secret-token",
    });
  });

  it("adds an IPFS RPC endpoint with a null token when none is given", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    await replicatedSyncAddIpfsRpc("https://rpc.filebase.io", null);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_add_ipfs_rpc", {
      baseUrl: "https://rpc.filebase.io",
      token: null,
    });
  });

  it("probes an IPFS RPC endpoint without persisting anything", async () => {
    vi.mocked(invoke).mockResolvedValue({ versionOk: true, headDiscoveryAvailable: true });
    const report = await replicatedSyncProbeIpfsRpc("https://rpc.filebase.io", "secret-token");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_probe_ipfs_rpc", {
      baseUrl: "https://rpc.filebase.io",
      token: "secret-token",
    });
    expect(report).toEqual({ versionOk: true, headDiscoveryAvailable: true });
  });

  const s3Config: S3ConnectorConfig = {
    endpoint: "https://s3.us-east-1.amazonaws.com",
    region: "us-east-1",
    bucket: "sync-bucket",
    prefix: "threestrands",
  };
  const s3Credentials: S3Credentials = { accessKeyId: "AKIAEXAMPLE", secretAccessKey: "secret" };

  it("tests an S3 connector with its config and credentials without saving it", async () => {
    const result = { reachable: true, canList: true, canWrite: true, canRead: true, canDelete: true, spacePresence: "none" };
    vi.mocked(invoke).mockResolvedValue(result);
    expect(await replicatedSyncProbeS3(s3Config, s3Credentials)).toEqual(result);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_probe_s3", { config: s3Config, credentials: s3Credentials });
  });

  it("adds an S3 connector with its config and credentials", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    await replicatedSyncAddS3(s3Config, s3Credentials);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_add_s3", { config: s3Config, credentials: s3Credentials });
  });

  it("updates a connector's name or credentials, sending null for what stays unchanged", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    await replicatedSyncUpdateConnector("s3-1", { label: "Personal R2" });
    expect(invoke).toHaveBeenCalledWith("replicated_sync_update_connector", {
      instanceId: "s3-1",
      label: "Personal R2",
      credentials: null,
    });

    await replicatedSyncUpdateConnector("s3-1", { credentials: { kind: "s3", ...s3Credentials } });
    expect(invoke).toHaveBeenCalledWith("replicated_sync_update_connector", {
      instanceId: "s3-1",
      label: null,
      credentials: { kind: "s3", accessKeyId: "AKIAEXAMPLE", secretAccessKey: "secret" },
    });

    await replicatedSyncUpdateConnector("ipfs-1", { label: "" });
    expect(invoke).toHaveBeenCalledWith("replicated_sync_update_connector", { instanceId: "ipfs-1", label: "", credentials: null });
  });

  it("passes instanceId and deleteData when removing a transport", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await replicatedSyncRemoveTransport("folder-1", true);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_remove_transport", {
      instanceId: "folder-1",
      deleteData: true,
    });
  });

  it("triggers a manual sync", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await replicatedSyncNow();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_now");
  });

  it("reports no conflicts outside a desktop build without invoking anything", async () => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    expect(await replicatedSyncConflicts()).toEqual([]);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("fetches frontier conflicts", async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    await replicatedSyncConflicts();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_conflicts");
  });

  it("resolves a conflict with the entity/field identity and the chosen operation id", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    const conflict: FrontierConflict = {
      entityType: "snippet",
      entityId: "s1",
      field: "name",
      candidates: [
        { operationId: "op-a", deviceId: "device-a", value: "From A" },
        { operationId: "op-b", deviceId: "device-b", value: "From B" },
      ],
    };
    await replicatedSyncResolveConflict(conflict, conflict.candidates[1]);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_resolve_conflict", {
      entityType: "snippet",
      entityId: "s1",
      field: "name",
      operationId: "op-b",
    });
  });

  it("reports the beta toggle off outside a desktop build without invoking anything", async () => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    expect(await replicatedSyncBetaEnabled()).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("reads and writes the beta features toggle", async () => {
    vi.mocked(invoke).mockResolvedValue(true);
    expect(await replicatedSyncBetaEnabled()).toBe(true);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_beta_enabled");

    vi.mocked(invoke).mockResolvedValue(undefined);
    await replicatedSyncSetBetaEnabled(true);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_set_beta_enabled", { on: true });
  });

  it("fetches enrollment status", async () => {
    vi.mocked(invoke).mockResolvedValue({ state: "notStarted" });
    const status = await replicatedSyncEnrollmentStatus();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_enrollment_status");
    expect(status).toEqual({ state: "notStarted" });
  });

  it("fetches pending incoming requests and the device roster", async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    await replicatedSyncPendingRequests();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_pending_requests");

    vi.mocked(invoke).mockResolvedValue([]);
    await replicatedSyncDeviceRoster();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_device_roster");
  });

  it("labels devices, leaves the space, and checks recovery phrases", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await replicatedSyncSetDeviceLabel("device-b", "Work laptop");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_set_device_label", { deviceId: "device-b", label: "Work laptop" });

    await replicatedSyncLeave();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_leave");

    vi.mocked(invoke).mockResolvedValue({ wordCount: 2, unknownWordPositions: [1], valid: false });
    expect(await replicatedSyncCheckRecoveryPhrase("abandon nope")).toEqual({ wordCount: 2, unknownWordPositions: [1], valid: false });
    expect(invoke).toHaveBeenCalledWith("replicated_sync_check_recovery_phrase", { phrase: "abandon nope" });
  });

  it("inspects the configured transports for an existing sync space", async () => {
    vi.mocked(invoke).mockResolvedValue("existing");
    expect(await replicatedSyncInspectSpace()).toBe("existing");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_inspect_space");
  });

  it("begins genesis without overriding the existing-space guard unless asked, and requests enrollment", async () => {
    vi.mocked(invoke).mockResolvedValue("twenty four words...");
    const phrase = await replicatedSyncBeginGenesis();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_begin_genesis", { allowExistingSpace: false });
    expect(phrase).toBe("twenty four words...");

    await replicatedSyncBeginGenesis(true);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_begin_genesis", { allowExistingSpace: true });

    vi.mocked(invoke).mockResolvedValue("AB12-CD34-EF56-0789");
    const fingerprint = await replicatedSyncRequestEnrollment();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_request_enrollment");
    expect(fingerprint).toBe("AB12-CD34-EF56-0789");
  });

  it("approves, rejects, and confirms enrollment requests by id", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await replicatedSyncApproveRequest("req-1");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_approve_request", { requestId: "req-1" });

    await replicatedSyncRejectRequest("req-2");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_reject_request", { requestId: "req-2" });

    await replicatedSyncConfirmEnrollment("req-3");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_confirm_enrollment", { requestId: "req-3" });
  });

  it("rotates the epoch with an optional device id to revoke", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await replicatedSyncRotateEpoch("device-b");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_rotate_epoch", { revokeDeviceId: "device-b" });

    await replicatedSyncRotateEpoch(null);
    expect(invoke).toHaveBeenCalledWith("replicated_sync_rotate_epoch", { revokeDeviceId: null });
  });

  it("joins with a recovery phrase", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await replicatedSyncJoinWithRecoveryPhrase("abandon ability able...");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_join_with_recovery_phrase", { phrase: "abandon ability able..." });
  });

  it("removes mail and calendar accounts on every enrolled device", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await removeSyncedMailAccount("you@example.com");
    await removeSyncedCalendarAccount("cal@example.com");
    expect(invoke).toHaveBeenCalledWith("remove_synced_mail_account", { email: "you@example.com" });
    expect(invoke).toHaveBeenCalledWith("remove_synced_calendar_account", { email: "cal@example.com" });
  });
});
