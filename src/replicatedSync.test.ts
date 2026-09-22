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
  replicatedSyncJoinWithRecoveryPhrase,
  replicatedSyncNow,
  replicatedSyncPendingRequests,
  replicatedSyncProbeIpfsRpc,
  replicatedSyncRejectRequest,
  replicatedSyncRemoveTransport,
  replicatedSyncResolveConflict,
  replicatedSyncRotateEpoch,
  replicatedSyncRequestEnrollment,
  replicatedSyncSetBetaEnabled,
  replicatedSyncStatus,
  type FrontierConflict,
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
    vi.mocked(invoke).mockResolvedValue({ versionOk: true, mfsAvailable: false });
    const report = await replicatedSyncProbeIpfsRpc("https://rpc.filebase.io", "secret-token");
    expect(invoke).toHaveBeenCalledWith("replicated_sync_probe_ipfs_rpc", {
      baseUrl: "https://rpc.filebase.io",
      token: "secret-token",
    });
    expect(report).toEqual({ versionOk: true, mfsAvailable: false });
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

  it("begins genesis and requests enrollment with no arguments beyond the command", async () => {
    vi.mocked(invoke).mockResolvedValue("twenty four words...");
    const phrase = await replicatedSyncBeginGenesis();
    expect(invoke).toHaveBeenCalledWith("replicated_sync_begin_genesis");
    expect(phrase).toBe("twenty four words...");

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
});
