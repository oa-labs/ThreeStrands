import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import {
  replicatedSyncAddFolder,
  replicatedSyncConflicts,
  replicatedSyncEnabled,
  replicatedSyncNow,
  replicatedSyncRemoveTransport,
  replicatedSyncResolveConflict,
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
});
