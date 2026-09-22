import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./replicatedSync");

import * as sync from "./replicatedSync";
import { pendingRecoveryPhrase } from "./RecoveryPhraseDialog";
import { ReplicatedSyncSettings } from "./SettingsPanel";

const folder: sync.ReplicatedSyncTransportStatus = {
  instanceId: "folder-1",
  kind: "folder",
  location: "/Users/me/Shared/ThreeStrands",
  health: "healthy",
  headDiscovery: true,
  pending: 0,
  delivered: 0,
  failed: 0,
};

function setUp({ transports = [folder] }: { transports?: sync.ReplicatedSyncTransportStatus[] } = {}) {
  vi.mocked(sync.replicatedSyncEnabled).mockResolvedValue(true);
  vi.mocked(sync.replicatedSyncBetaEnabled).mockResolvedValue(true);
  vi.mocked(sync.replicatedSyncStatus).mockResolvedValue(transports);
  vi.mocked(sync.replicatedSyncConflicts).mockResolvedValue([]);
  vi.mocked(sync.replicatedSyncEnrollmentStatus).mockResolvedValue({ state: "notStarted" });
  vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([]);
  vi.mocked(sync.replicatedSyncDeviceRoster).mockResolvedValue([]);
}

beforeEach(() => {
  vi.clearAllMocks();
  setUp();
});
afterEach(cleanup);

describe("replicated sync setup choice", () => {
  it("steers toward joining when the location already holds a space", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("existing");
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/Another device already set up a sync space/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create a new sync space" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Request to join from an existing device" })).toHaveClass("primary-action");
    expect(screen.getByRole("button", { name: "Join with recovery phrase" })).toBeInTheDocument();
  });

  it("creates a separate space over an existing one only after explicit confirmation", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("existing");
    vi.mocked(sync.replicatedSyncBeginGenesis).mockResolvedValue("separate space phrase");
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Create a separate sync space instead…" }));
    expect(sync.replicatedSyncBeginGenesis).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Create a separate sync space instead…" }));
    fireEvent.click(screen.getByRole("button", { name: "Create separate space" }));

    await waitFor(() => expect(sync.replicatedSyncBeginGenesis).toHaveBeenCalledWith(true));
    await waitFor(() => expect(pendingRecoveryPhrase()).toBe("separate space phrase"));
  });

  it("offers creation first when the location is empty, and holds the phrase instead of showing it inline", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("none");
    vi.mocked(sync.replicatedSyncBeginGenesis).mockResolvedValue("fresh space phrase");
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/No sync space found here yet/)).toBeInTheDocument();
    const create = screen.getByRole("button", { name: "Create a new sync space" });
    expect(create).toHaveClass("primary-action");
    expect(screen.getByRole("button", { name: "Request to join from an existing device" })).not.toHaveClass("primary-action");
    fireEvent.click(create);

    await waitFor(() => expect(sync.replicatedSyncBeginGenesis).toHaveBeenCalledWith(false));
    await waitFor(() => expect(pendingRecoveryPhrase()).toBe("fresh space phrase"));
    expect(screen.queryByText("fresh space phrase")).not.toBeInTheDocument();
  });

  it("keeps creation disabled while the check is still running", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockReturnValue(new Promise(() => {}));
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/Checking your sync locations/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create a new sync space" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Request to join from an existing device" })).toBeEnabled();
  });

  it("warns but still allows creation when a location could not be checked", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockRejectedValue(new Error("offline"));
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/Couldn’t check every sync location/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create a new sync space" })).toBeEnabled();
  });

  it("does not inspect before any location is configured", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/Set this device up as the first device/)).toBeInTheDocument();
    expect(sync.replicatedSyncInspectSpace).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Create a new sync space" })).toBeDisabled();
  });

  it("re-inspects after the native guard refuses a space that appeared since the last check", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValueOnce("none").mockResolvedValue("existing");
    vi.mocked(sync.replicatedSyncBeginGenesis).mockRejectedValue("A sync space already exists in this location.");
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Create a new sync space" }));

    expect(await screen.findByText(/Another device already set up a sync space/)).toBeInTheDocument();
    expect(screen.getByText("A sync space already exists in this location.")).toBeInTheDocument();
    expect(sync.replicatedSyncBeginGenesis).toHaveBeenCalledWith(false);
    expect(screen.queryByRole("button", { name: "Create a new sync space" })).not.toBeInTheDocument();
  });
});
