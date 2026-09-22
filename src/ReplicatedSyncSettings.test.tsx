import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./replicatedSync");

import * as sync from "./replicatedSync";
import { pendingRecoveryPhrase } from "./RecoveryPhraseDialog";
import { currentSetupStep, ReplicatedSyncSettings, syncOverview } from "./ReplicatedSyncSettings";

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

function setUp({
  transports = [folder],
  status = { state: "notStarted" },
}: { transports?: sync.ReplicatedSyncTransportStatus[]; status?: sync.EnrollmentStatus } = {}) {
  vi.mocked(sync.replicatedSyncEnabled).mockResolvedValue(true);
  vi.mocked(sync.replicatedSyncBetaEnabled).mockResolvedValue(true);
  vi.mocked(sync.replicatedSyncStatus).mockResolvedValue(transports);
  vi.mocked(sync.replicatedSyncConflicts).mockResolvedValue([]);
  vi.mocked(sync.replicatedSyncEnrollmentStatus).mockResolvedValue(status);
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

  it("does not inspect or offer the choice before any location is configured", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("button", { name: "Add a sync folder" })).toBeInTheDocument();
    expect(sync.replicatedSyncInspectSpace).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Create a new sync space" })).not.toBeInTheDocument();
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

function stepState(title: string): string | null {
  const step = screen.getByRole("listitem", { name: title });
  return step.getAttribute("aria-current") === "step" ? "current" : within(step).getByText(/^(Done|Up next)$/).textContent;
}

describe("replicated sync setup steps", () => {
  it("starts at choosing a location, with the shared-folder requirement and IPFS behind a disclosure", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("list", { name: "Replicated sync setup" })).toBeInTheDocument();
    expect(stepState("Choose where to sync")).toBe("current");
    expect(stepState("Join or create a sync space")).toBe("Up next");
    expect(stepState("Verify this device")).toBe("Up next");
    expect(screen.getByText(/Every device you sync must use the same location/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a sync folder" })).toHaveClass("primary-action");
    const ipfs = screen.getByText("Use an IPFS RPC endpoint instead").closest("details")!;
    expect(ipfs).not.toHaveAttribute("open");
    expect(within(ipfs).getByRole("button", { name: "Add IPFS RPC endpoint" })).toBeInTheDocument();
  });

  it("marks the location done and summarizes it once one is configured", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("none");
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("button", { name: "Create a new sync space" })).toBeInTheDocument();
    expect(stepState("Choose where to sync")).toBe("Done");
    expect(stepState("Join or create a sync space")).toBe("current");
    const done = screen.getByRole("listitem", { name: "Choose where to sync" });
    expect(within(done).getByText(folder.location, { selector: "summary" })).toBeInTheDocument();
    expect(within(done).getByRole("button", { name: "Add another sync folder" })).toBeInTheDocument();
  });

  it("waits for approval on the verify step and can check for it", async () => {
    setUp({ status: { state: "awaitingGrant", requestId: "req-1", fingerprint: "AB12-CD34-EF56-0789", createdAt: "2026-09-22T10:00:00Z" } });
    vi.mocked(sync.replicatedSyncNow).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText("AB12-CD34-EF56-0789")).toBeInTheDocument();
    expect(stepState("Join or create a sync space")).toBe("Done");
    expect(stepState("Verify this device")).toBe("current");
    fireEvent.click(screen.getByRole("button", { name: "Check for approval" }));
    await waitFor(() => expect(sync.replicatedSyncNow).toHaveBeenCalled());
  });

  it("confirms an arrived approval from the verify step", async () => {
    setUp({ status: { state: "awaitingConfirmation", requestId: "req-1", fingerprint: "AB12-CD34-EF56-0789", approverFingerprint: "FFFF-0000-1111-2222" } });
    vi.mocked(sync.replicatedSyncConfirmEnrollment).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Confirm — fingerprints match" }));
    await waitFor(() => expect(sync.replicatedSyncConfirmEnrollment).toHaveBeenCalledWith("req-1"));
    expect(stepState("Verify this device")).toBe("current");
  });

  it("keeps the privacy explanation and beta toggle out of the main flow", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    const explanation = (await screen.findByText("How replicated sync works")).closest("details")!;
    expect(explanation).not.toHaveAttribute("open");
    expect(within(explanation).getByText(/never sees the plaintext/)).toBeInTheDocument();
    const advanced = screen.getByText("Advanced", { selector: "summary" }).closest("details")!;
    expect(within(advanced).getByRole("checkbox", { name: "Enable beta features" })).toBeChecked();
    expect(screen.getAllByRole("checkbox", { name: "Enable beta features" })).toHaveLength(1);
  });

  it("shows only the beta toggle until replicated sync is turned on", async () => {
    vi.mocked(sync.replicatedSyncEnabled).mockResolvedValue(false);
    vi.mocked(sync.replicatedSyncBetaEnabled).mockResolvedValue(false);
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("checkbox", { name: "Enable beta features" })).not.toBeChecked();
    expect(screen.queryByRole("list", { name: "Replicated sync setup" })).not.toBeInTheDocument();
  });
});

describe("currentSetupStep", () => {
  it("follows locations until enrollment is in progress", () => {
    expect(currentSetupStep({ state: "notStarted" }, 0)).toBe("location");
    expect(currentSetupStep({ state: "notStarted" }, 1)).toBe("choose");
    expect(currentSetupStep(null, 1)).toBe("choose");
    expect(currentSetupStep({ state: "awaitingGrant", requestId: "r", fingerprint: "f", createdAt: "t" }, 0)).toBe("verify");
    expect(currentSetupStep({ state: "awaitingConfirmation", requestId: "r", fingerprint: "f", approverFingerprint: "a" }, 1)).toBe("verify");
  });
});

describe("enrolled overview", () => {
  const enrolled: sync.EnrollmentStatus = { state: "enrolled", deviceCount: 2 };
  const roster: sync.DeviceRosterEntry[] = [
    { deviceId: "device-self", status: "active", isSelf: true },
    { deviceId: "device-other", status: "active", isSelf: false },
  ];

  it("replaces the setup steps with a status summary and grouped sections", async () => {
    setUp({ status: enrolled, transports: [{ ...folder, lastSuccessAt: "2026-09-22T10:00:00Z" }] });
    vi.mocked(sync.replicatedSyncDeviceRoster).mockResolvedValue(roster);
    render(<ReplicatedSyncSettings />);

    const summary = await screen.findByRole("status", { name: "Sync status" });
    expect(summary).toHaveTextContent(/Syncing · 2 devices · last synced/);
    expect(screen.queryByRole("list", { name: "Replicated sync setup" })).not.toBeInTheDocument();
    expect(screen.getByText("Devices (2)").closest("details")).toHaveAttribute("open");
    expect(screen.getByText("Sync locations (1)").closest("details")).not.toHaveAttribute("open");
    expect(screen.getByText("device-other")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Revoke…" })).toBeInTheDocument();
  });

  it("opens sync locations when one needs attention", async () => {
    setUp({ status: enrolled, transports: [{ ...folder, health: "unavailable: folder missing" }] });
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("status", { name: "Sync status" })).toHaveTextContent("1 sync location needs attention");
    expect(screen.getByText("Sync locations (1)").closest("details")).toHaveAttribute("open");
  });

  it("puts devices waiting to join above the other sections and approves them", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([{ requestId: "req-9", fingerprint: "9999-8888-7777-6666", createdAt: "2026-09-22T10:00:00Z" }]);
    vi.mocked(sync.replicatedSyncApproveRequest).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    const waiting = await screen.findByRole("list", { name: "Devices waiting to join" });
    expect(waiting.closest("details")).toBeNull();
    fireEvent.click(within(waiting).getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(sync.replicatedSyncApproveRequest).toHaveBeenCalledWith("req-9"));
  });
});

describe("syncOverview", () => {
  it("reports a missing location as not syncing", () => {
    expect(syncOverview([], 1)).toEqual({ tone: "attention", text: expect.stringMatching(/^Not syncing · 1 device · /) });
  });

  it("counts degraded or failing locations as needing attention", () => {
    const failing = { ...folder, instanceId: "b", failed: 2 };
    const degraded = { ...folder, instanceId: "c", health: "degraded: recent transient failures" };
    expect(syncOverview([folder, failing, degraded], 3)).toEqual({ tone: "attention", text: "3 devices · 2 sync locations need attention" });
  });

  it("uses the most recent success across healthy locations", () => {
    const older = { ...folder, lastSuccessAt: "2026-09-20T10:00:00Z" };
    const newer = { ...folder, instanceId: "b", lastSuccessAt: "2026-09-22T10:00:00Z" };
    expect(syncOverview([older, newer], 2).text).toBe(`Syncing · 2 devices · last synced ${new Date("2026-09-22T10:00:00Z").toLocaleString()}`);
    expect(syncOverview([folder], 2).text).toBe("Syncing · 2 devices · not synced yet");
  });
});
