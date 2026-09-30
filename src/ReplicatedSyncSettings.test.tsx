import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./replicatedSync");

import * as sync from "./replicatedSync";
import { pendingRecoveryPhrase } from "./RecoveryPhraseDialog";
import { currentSetupStep, describeTransportHealth, recoveryPhraseFeedback, ReplicatedSyncSettings, syncOverview } from "./ReplicatedSyncSettings";

const folder: sync.ReplicatedSyncTransportStatus = {
  instanceId: "folder-1",
  kind: "folder",
  location: "/Users/me/Shared/ThreeStrands",
  supportsDeleteData: true,
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
  vi.mocked(sync.replicatedSyncListJoinCodes).mockResolvedValue([]);
  vi.mocked(sync.replicatedSyncJoinCodeNotices).mockResolvedValue([]);
  vi.mocked(sync.replicatedSyncProtocolResetNotice).mockResolvedValue(false);
}

beforeEach(() => {
  vi.clearAllMocks();
  setUp();
});
afterEach(cleanup);

describe("replicated sync setup choice", () => {
  it("steers toward the recovery phrase when the connector already holds a group", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("existing");
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/Another device already set up a sync group/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create a new sync group" })).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Enter the recovery phrase" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Join with recovery phrase" })).toHaveClass("primary-action");
    expect(screen.getByRole("button", { name: "Ask another device to approve this one" })).not.toHaveClass("primary-action");
    expect(screen.getByRole("button", { name: "Paste a join code" })).toBeInTheDocument();
  });

  it("points back to the join-code field from the setup choice", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("existing");
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Paste a join code" }));
    expect(screen.getByRole("textbox", { name: "Join code" })).toHaveFocus();
  });

  it("creates a separate group over an existing one only after explicit confirmation", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("existing");
    vi.mocked(sync.replicatedSyncBeginGenesis).mockResolvedValue("separate space phrase");
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Create a separate sync group instead…" }));
    expect(sync.replicatedSyncBeginGenesis).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Create a separate sync group instead…" }));
    fireEvent.click(screen.getByRole("button", { name: "Create separate group" }));

    await waitFor(() => expect(sync.replicatedSyncBeginGenesis).toHaveBeenCalledWith(true));
    await waitFor(() => expect(pendingRecoveryPhrase()).toBe("separate space phrase"));
  });

  it("offers creation first when the connector is empty, and holds the phrase instead of showing it inline", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("none");
    vi.mocked(sync.replicatedSyncBeginGenesis).mockResolvedValue("fresh space phrase");
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/No sync group found here yet/)).toBeInTheDocument();
    const create = screen.getByRole("button", { name: "Create a new sync group" });
    expect(create).toHaveClass("primary-action");
    expect(screen.getByRole("button", { name: "Ask another device to approve this one" })).not.toHaveClass("primary-action");
    expect(screen.getByRole("textbox", { name: "Or join with a recovery phrase" })).toBeInTheDocument();
    fireEvent.click(create);

    await waitFor(() => expect(sync.replicatedSyncBeginGenesis).toHaveBeenCalledWith(false));
    await waitFor(() => expect(pendingRecoveryPhrase()).toBe("fresh space phrase"));
    expect(screen.queryByText("fresh space phrase")).not.toBeInTheDocument();
  });

  it("keeps creation disabled while the check is still running", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockReturnValue(new Promise(() => {}));
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/Checking your connectors/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create a new sync group" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Ask another device to approve this one" })).toBeEnabled();
  });

  it("warns but still allows creation when a connector could not be checked", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockRejectedValue(new Error("offline"));
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/Couldn’t check every connector/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create a new sync group" })).toBeEnabled();
  });

  it("offers no way to create or join over a group from an earlier test version", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("legacy");
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/sync group from an earlier test version/)).toBeInTheDocument();
    expect(screen.getByText(/wait for your sync app to finish/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create a new sync group" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Join with recovery phrase" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Ask another device to approve this one" })).not.toBeInTheDocument();
  });

  it("does not inspect or offer the choice before any connector is configured", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("button", { name: /^Shared folder/ })).toBeInTheDocument();
    expect(sync.replicatedSyncInspectSpace).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Create a new sync group" })).not.toBeInTheDocument();
  });

  it("re-inspects after the native guard refuses a group that appeared since the last check", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValueOnce("none").mockResolvedValue("existing");
    vi.mocked(sync.replicatedSyncBeginGenesis).mockRejectedValue("A sync space already exists in this location.");
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Create a new sync group" }));

    expect(await screen.findByText(/Another device already set up a sync group/)).toBeInTheDocument();
    expect(screen.getByText("A sync space already exists in this location.")).toBeInTheDocument();
    expect(sync.replicatedSyncBeginGenesis).toHaveBeenCalledWith(false);
    expect(screen.queryByRole("button", { name: "Create a new sync group" })).not.toBeInTheDocument();
  });
});

function stepState(title: string): string | null {
  const step = screen.getByRole("listitem", { name: title });
  return step.getAttribute("aria-current") === "step" ? "current" : within(step).getByText(/^(Done|Up next)$/).textContent;
}

describe("replicated sync setup steps", () => {
  it("starts at adding a connector, offering the three kinds and each kind's form", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("list", { name: "Replicated sync setup" })).toBeInTheDocument();
    expect(stepState("Add a connector")).toBe("current");
    expect(stepState("Join your sync group")).toBe("Up next");
    expect(stepState("Verify this device")).toBe("Up next");
    const kinds = within(screen.getByRole("group", { name: "Connector type" })).getAllByRole("button");
    expect(kinds.map((button) => button.querySelector("strong")?.textContent)).toEqual(["Shared folder", "S3 storage", "IPFS"]);

    fireEvent.click(screen.getByRole("button", { name: /^IPFS/ }));
    const ipfs = screen.getByRole("group", { name: "Add IPFS" });
    expect(within(ipfs).getByRole("button", { name: "Add IPFS RPC endpoint" })).toBeInTheDocument();
    expect(within(ipfs).getByText(/bucket-specific RPC token/)).toBeInTheDocument();
    fireEvent.click(within(ipfs).getByRole("button", { name: "← Back" }));
    expect(screen.getByRole("group", { name: "Connector type" })).toBeInTheDocument();
  });

  it("offers joining with a code above the manual steps, but not while verifying", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);
    const panel = await screen.findByRole("group", { name: "Join with a code from another device" });
    expect(within(panel).getByRole("textbox", { name: "Join code" })).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "Join sync group" })).toBeDisabled();
    cleanup();

    setUp({ status: { state: "awaitingGrant", requestId: "req-1", fingerprint: "AB12-CD34-EF56-0789", createdAt: "2026-09-22T10:00:00Z" } });
    render(<ReplicatedSyncSettings />);
    expect(await screen.findByText("AB12-CD34-EF56-0789")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Join with a code from another device" })).not.toBeInTheDocument();
  });

  it("shows an empty RPC URL as empty and enables the connection test once the Filebase URL is filled in", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: /^IPFS/ }));
    const ipfs = screen.getByRole("group", { name: "Add IPFS" });
    const url = within(ipfs).getByRole("textbox", { name: "RPC base URL" });
    expect(url).toHaveValue("");
    expect(url).not.toHaveAttribute("placeholder", "https://rpc.filebase.io");
    expect(within(ipfs).getByRole("button", { name: "Test connection" })).toBeDisabled();

    fireEvent.click(within(ipfs).getByRole("button", { name: "Fill in Filebase URL" }));
    expect(url).toHaveValue("https://rpc.filebase.io");
    expect(within(ipfs).getByRole("button", { name: "Test connection" })).toBeEnabled();
  });

  it("marks the connector step done and summarizes it once one is configured", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("none");
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("button", { name: "Create a new sync group" })).toBeInTheDocument();
    expect(stepState("Add a connector")).toBe("Done");
    expect(stepState("Join your sync group")).toBe("current");
    const done = screen.getByRole("listitem", { name: "Add a connector" });
    expect(within(done).getByText(folder.location, { selector: "summary" })).toBeInTheDocument();
    expect(within(done).getByRole("button", { name: "Add another connector" })).toBeInTheDocument();
  });

  it("waits for approval on the verify step and can check for it", async () => {
    setUp({ status: { state: "awaitingGrant", requestId: "req-1", fingerprint: "AB12-CD34-EF56-0789", createdAt: "2026-09-22T10:00:00Z" } });
    vi.mocked(sync.replicatedSyncNow).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText("AB12-CD34-EF56-0789")).toBeInTheDocument();
    expect(stepState("Join your sync group")).toBe("Done");
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

  it("lays out disclosure content in a spaced body and keeps the beta checkbox beside its label", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    const explanation = (await screen.findByText("How replicated sync works")).closest("details")!;
    const body = explanation.querySelector(":scope > .settings-disclosure-body");
    expect(body).not.toBeNull();
    expect(body).toContainElement(screen.getByText(/never sees the plaintext/));
    for (const details of document.querySelectorAll("details.settings-disclosure")) {
      expect(Array.from(details.children).map((child) => child.tagName)).toEqual(["SUMMARY", "DIV"]);
    }
    const betaLabel = screen.getByRole("checkbox", { name: "Enable beta features" }).closest("label")!;
    expect(betaLabel).toHaveClass("settings-checkbox");
    expect(betaLabel).not.toHaveClass("settings-field-inline");
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
    expect(currentSetupStep({ state: "rejected", requestId: "r", fingerprint: "f" }, 1)).toBe("verify");
  });
});

describe("shared enrollment rejection", () => {
  it("explains that the request was rejected by the group and offers a fresh request", async () => {
    setUp({ status: { state: "rejected", requestId: "req-1", fingerprint: "AB12-CD34-EF56-0789" } });
    vi.mocked(sync.replicatedSyncLeave).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText(/A rejection response arrived/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Start a new request…" }));
    fireEvent.click(screen.getByRole("button", { name: "Start over" }));
    await waitFor(() => expect(sync.replicatedSyncLeave).toHaveBeenCalled());
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
    expect(screen.getByText("Connectors (1)").closest("details")).not.toHaveAttribute("open");
    expect(screen.getByRole("button", { name: "Add a device" })).toBeEnabled();
    const devices = within(screen.getByRole("list", { name: "Devices" })).getAllByRole("listitem");
    expect(devices[0]).toHaveTextContent("This device");
    expect(devices[1]).toHaveTextContent("Unlabeled device");
    expect(within(devices[1]!).getByText("device-other")).toBeInTheDocument();
    expect(within(devices[1]!).getByRole("button", { name: "Revoke…" })).toBeInTheDocument();
  });

  it("warns when the group approaches its historical key handoff limit", async () => {
    setUp({
      status: {
        ...enrolled,
        historyHandoffWarning: { used: 820, limit: 1024 },
      },
    });
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("status", { name: "Sync history capacity" }))
      .toHaveTextContent("This group has used 820 of 1,024 historical key slots");
    expect(screen.getByRole("status", { name: "Sync history capacity" }))
      .toHaveTextContent("new devices can’t receive the full sync history");
  });

  it("opens connectors when one needs attention", async () => {
    setUp({ status: enrolled, transports: [{ ...folder, health: "unavailable: folder missing" }] });
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("status", { name: "Sync status" })).toHaveTextContent("1 connector needs attention");
    expect(screen.getByText("Connectors (1)").closest("details")).toHaveAttribute("open");
  });

  it("offers deleting files for a folder connector and keeping data for one that cannot delete", async () => {
    // Per-kind confirmation wording and the absent delete action for IPFS are
    // covered on ConnectorCard in SyncConnectors.test.tsx; this checks the
    // enrolled overview wires each card to its own transport's capability.
    const ipfs: sync.ReplicatedSyncTransportStatus = {
      ...folder,
      instanceId: "ipfs-1",
      kind: "ipfs_rpc",
      location: "https://rpc.filebase.io",
      supportsDeleteData: false,
    };
    setUp({ status: enrolled, transports: [folder, ipfs] });
    render(<ReplicatedSyncSettings />);

    const [folderCard, ipfsCard] = within(await screen.findByRole("list", { name: "Connectors" })).getAllByRole("listitem");
    fireEvent.click(within(folderCard!).getByRole("button", { name: "Disconnect…" }));
    expect(within(folderCard!).getByRole("button", { name: "Delete files and disconnect" })).toBeInTheDocument();

    fireEvent.click(within(ipfsCard!).getByRole("button", { name: "Disconnect…" }));
    expect(within(ipfsCard!).getByRole("button", { name: "Disconnect and keep data" })).toBeInTheDocument();
  });

  it("shows the waiting-for-admission banner above the sync status after joining with a code", async () => {
    setUp({ status: { state: "enrolled", deviceCount: 2, awaitingAdmissionFrom: "Work laptop" } });
    render(<ReplicatedSyncSettings />);

    const banner = await screen.findByRole("status", { name: "Joining" });
    expect(banner).toHaveTextContent("Waiting for Work laptop to finish adding this device");
    expect(banner.compareDocumentPosition(screen.getByRole("status", { name: "Sync status" })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("shows join-code notices, how each device joined, and opens the add-a-device panel", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncDeviceRoster).mockResolvedValue([
      ...roster,
      { deviceId: "device-code", status: "active", isSelf: false, label: "Phone", joinedWithJoinCode: true },
    ]);
    vi.mocked(sync.replicatedSyncJoinCodeNotices).mockResolvedValue([
      { redemptionCid: "r1", kind: "joined", deviceId: "device-code", deviceName: "Phone", inviterDeviceId: "device-self", inviterName: "Desk", at: "2026-09-22T10:00:00Z" },
    ]);
    render(<ReplicatedSyncSettings />);

    const notices = await screen.findByRole("list", { name: "Join code notices" });
    expect(notices).toHaveTextContent("Phone joined with a join code from Desk.");
    const devices = within(screen.getByRole("list", { name: "Devices" })).getAllByRole("listitem");
    expect(devices[2]).toHaveTextContent("Joined with a join code");
    expect(devices[1]).not.toHaveTextContent("Joined with a join code");

    fireEvent.click(screen.getByRole("button", { name: "Add a device" }));
    expect(screen.getByRole("group", { name: "Add a device" })).toBeInTheDocument();
  });

  it("puts devices waiting to join above the device and connector sections", async () => {
    // Approving from this list is covered in "approving a device from an existing device".
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([{ requestId: "req-9", fingerprint: "9999-8888-7777-6666", createdAt: "2026-09-22T10:00:00Z" }]);
    render(<ReplicatedSyncSettings />);

    const waiting = await screen.findByRole("list", { name: "Devices waiting to join" });
    expect(waiting.closest("details")).toBeNull();
    const follows = (later: Element) => waiting.compareDocumentPosition(later) & Node.DOCUMENT_POSITION_FOLLOWING;
    expect(screen.getByRole("status", { name: "Sync status" }).compareDocumentPosition(waiting) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(follows(screen.getByText("Devices (0)", { selector: "summary" }))).toBeTruthy();
    expect(follows(screen.getByText("Connectors (1)", { selector: "summary" }))).toBeTruthy();
    expect(follows(screen.getByText("Advanced", { selector: "summary" }))).toBeTruthy();
  });

  it("revokes another device only after confirmation, rotating the epoch for that device", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncDeviceRoster).mockResolvedValue(roster);
    vi.mocked(sync.replicatedSyncRotateEpoch).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    const [self, other] = within(await screen.findByRole("list", { name: "Devices" })).getAllByRole("listitem");
    expect(within(self!).queryByRole("button", { name: "Revoke…" })).not.toBeInTheDocument();

    fireEvent.click(within(other!).getByRole("button", { name: "Revoke…" }));
    const confirm = within(other!).getByRole("group", { name: "Revoke device confirmation" });
    expect(confirm).toHaveTextContent("future writes from it will no longer be trusted");
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(within(other!).queryByRole("group", { name: "Revoke device confirmation" })).not.toBeInTheDocument();
    expect(sync.replicatedSyncRotateEpoch).not.toHaveBeenCalled();

    vi.mocked(sync.replicatedSyncDeviceRoster).mockClear();
    fireEvent.click(within(other!).getByRole("button", { name: "Revoke…" }));
    fireEvent.click(within(other!).getByRole("button", { name: "Revoke device" }));
    await waitFor(() => expect(sync.replicatedSyncRotateEpoch).toHaveBeenCalledWith("device-other"));
    expect(sync.replicatedSyncRotateEpoch).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(sync.replicatedSyncDeviceRoster).toHaveBeenCalled());
  });

  it("does not show revoke for a device that is already revoked", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncDeviceRoster).mockResolvedValue([roster[0]!, { ...roster[1]!, status: "revoked" }]);
    render(<ReplicatedSyncSettings />);

    const devices = within(await screen.findByRole("list", { name: "Devices" })).getAllByRole("listitem");
    expect(devices[1]).toHaveTextContent("Revoked");
    expect(screen.queryByRole("button", { name: "Revoke…" })).not.toBeInTheDocument();
  });

  it("turns beta features off from Advanced and reloads the section", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncSetBetaEnabled).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    const advanced = (await screen.findByText("Advanced", { selector: "summary" })).closest("details")!;
    const beta = within(advanced).getByRole("checkbox", { name: "Enable beta features" });
    expect(beta).toBeChecked();
    vi.mocked(sync.replicatedSyncBetaEnabled).mockClear();
    fireEvent.click(beta);

    await waitFor(() => expect(sync.replicatedSyncSetBetaEnabled).toHaveBeenCalledWith(false));
    expect(sync.replicatedSyncSetBetaEnabled).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(sync.replicatedSyncBetaEnabled).toHaveBeenCalled());
  });

  it("resolves a conflict with the chosen candidate and drops it once resolved", async () => {
    const conflict: sync.FrontierConflict = {
      entityType: "split_inbox",
      entityId: "inbox-1",
      field: "name",
      candidates: [
        { operationId: "op-a", deviceId: "device-self-0000", value: "Work" },
        { operationId: "op-b", deviceId: "device-other-000", value: "Office" },
      ],
    };
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncConflicts).mockResolvedValueOnce([conflict]).mockResolvedValue([]);
    vi.mocked(sync.replicatedSyncResolveConflict).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByText("split inbox conflict: name")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: /"Office"/ }));
    fireEvent.click(screen.getByRole("button", { name: "Resolve conflict" }));

    await waitFor(() => expect(sync.replicatedSyncResolveConflict).toHaveBeenCalledWith(conflict, conflict.candidates[1]));
    expect(sync.replicatedSyncResolveConflict).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByText("split inbox conflict: name")).not.toBeInTheDocument());
  });

  it("keeps an unresolved conflict and shows the failure beside it", async () => {
    const conflict: sync.FrontierConflict = {
      entityType: "task",
      entityId: "task-1",
      field: "title",
      candidates: [
        { operationId: "op-a", deviceId: "device-self-0000", value: "Draft" },
        { operationId: "op-b", deviceId: "device-other-000", value: "Final" },
      ],
    };
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncConflicts).mockResolvedValue([conflict]);
    vi.mocked(sync.replicatedSyncResolveConflict).mockRejectedValue("The conflict changed.");
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Resolve conflict" }));

    expect(await screen.findByText("The conflict changed.")).toBeInTheDocument();
    expect(sync.replicatedSyncResolveConflict).toHaveBeenCalledWith(conflict, conflict.candidates[0]);
    expect(screen.getByText("task conflict: title")).toBeInTheDocument();
  });
});

describe("syncOverview", () => {
  it("reports a missing connector as not syncing", () => {
    expect(syncOverview([], 1)).toEqual({ tone: "attention", text: expect.stringMatching(/^Not syncing · 1 device · /) });
  });

  it("counts degraded or failing connectors as needing attention", () => {
    const failing = { ...folder, instanceId: "b", failed: 2 };
    const degraded = { ...folder, instanceId: "c", health: "degraded: recent transient failures" };
    expect(syncOverview([folder, failing, degraded], 3)).toEqual({ tone: "attention", text: "3 devices · 2 connectors need attention" });
  });

  it("uses the most recent success across healthy connectors", () => {
    const older = { ...folder, lastSuccessAt: "2026-09-20T10:00:00Z" };
    const newer = { ...folder, instanceId: "b", lastSuccessAt: "2026-09-22T10:00:00Z" };
    expect(syncOverview([older, newer], 2).text).toBe(`Syncing · 2 devices · last synced ${new Date("2026-09-22T10:00:00Z").toLocaleString()}`);
    expect(syncOverview([folder], 2).text).toBe("Syncing · 2 devices · not synced yet");
  });
});

describe("approving a device from an existing device", () => {
  const enrolled: sync.EnrollmentStatus = { state: "enrolled", deviceCount: 1 };
  const request: sync.IncomingEnrollmentRequest = { requestId: "req-9", deviceId: "device-new", fingerprint: "9999-8888-7777-6666", createdAt: "2026-09-22T10:00:00Z" };

  it("shows the code to compare before approval is possible", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request]);
    render(<ReplicatedSyncSettings />);

    const waiting = await screen.findByRole("list", { name: "Devices waiting to join" });
    expect(within(waiting).queryByText(request.fingerprint)).not.toBeInTheDocument();
    expect(within(waiting).queryByRole("button", { name: "Codes match — approve" })).not.toBeInTheDocument();

    fireEvent.click(within(waiting).getByRole("button", { name: "Review…" }));
    const panel = within(waiting).getByRole("group", { name: "Approve device confirmation" });
    expect(within(panel).getByText(request.fingerprint)).toBeInTheDocument();
    expect(within(panel).getByText(/If the codes don’t match, reject the request/)).toBeInTheDocument();

    fireEvent.click(within(panel).getByRole("button", { name: "Cancel" }));
    expect(within(waiting).queryByRole("group", { name: "Approve device confirmation" })).not.toBeInTheDocument();
    expect(sync.replicatedSyncApproveRequest).not.toHaveBeenCalled();
  });

  it("sets a shared name for the approved device when a name is given", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request]);
    vi.mocked(sync.replicatedSyncApproveRequest).mockResolvedValue(undefined);
    vi.mocked(sync.replicatedSyncSetDeviceLabel).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Review…" }));
    fireEvent.change(screen.getByRole("textbox", { name: /Name this device/ }), { target: { value: "Work laptop" } });
    fireEvent.click(screen.getByRole("button", { name: "Codes match — approve" }));

    await waitFor(() => expect(sync.replicatedSyncSetDeviceLabel).toHaveBeenCalledWith("device-new", "Work laptop"));
    expect(sync.replicatedSyncApproveRequest).toHaveBeenCalledWith("req-9");
  });

  it("shows an approval failure on that request, not at the top of the section", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request]);
    vi.mocked(sync.replicatedSyncApproveRequest).mockRejectedValue("The request expired.");
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Review…" }));
    fireEvent.click(screen.getByRole("button", { name: "Codes match — approve" }));

    const card = screen.getByRole("list", { name: "Devices waiting to join" });
    expect(await within(card).findByText("The request expired.")).toBeInTheDocument();
    expect(screen.getAllByText("The request expired.")).toHaveLength(1);
    expect(sync.replicatedSyncSetDeviceLabel).not.toHaveBeenCalled();
  });

  it("confirms that rejection is shared with every device", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request]);
    vi.mocked(sync.replicatedSyncRejectRequest).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    const waiting = await screen.findByRole("list", { name: "Devices waiting to join" });
    fireEvent.click(within(waiting).getByRole("button", { name: "Reject…" }));
    const confirmation = within(waiting).getByRole("group", { name: "Reject device request confirmation" });
    expect(confirmation).toHaveTextContent("on all your sync devices");
    fireEvent.click(within(confirmation).getByRole("button", { name: "Reject on all devices" }));
    await waitFor(() => expect(sync.replicatedSyncRejectRequest).toHaveBeenCalledWith(request.requestId));
  });
});

describe("device names and activity", () => {
  const enrolled: sync.EnrollmentStatus = { state: "enrolled", deviceCount: 2 };

  it("shows shared names and the last change from each device", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncDeviceRoster).mockResolvedValue([
      { deviceId: "device-self", status: "active", isSelf: true, label: "Desk", lastChangeAt: "2026-09-22T10:00:00Z" },
      { deviceId: "device-other", status: "active", isSelf: false, label: null, lastChangeAt: null },
    ]);
    render(<ReplicatedSyncSettings />);

    const devices = within(await screen.findByRole("list", { name: "Devices" })).getAllByRole("listitem");
    expect(devices[0]).toHaveTextContent("Desk");
    expect(devices[0]).toHaveTextContent("This device");
    expect(devices[0]).toHaveTextContent(`Last change ${new Date("2026-09-22T10:00:00Z").toLocaleString()}`);
    expect(devices[1]).toHaveTextContent("No changes yet");
  });

  it("renames a device, limited to the shared length", async () => {
    setUp({ status: enrolled });
    vi.mocked(sync.replicatedSyncDeviceRoster).mockResolvedValue([{ deviceId: "device-self", status: "active", isSelf: true }]);
    vi.mocked(sync.replicatedSyncSetDeviceLabel).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Name" }));
    expect(screen.getByText("device-self")).toBeInTheDocument();
    const input = screen.getByRole("textbox", { name: /Name \(shared with your other devices\)/ });
    expect(input).toHaveAttribute("maxLength", String(sync.MAX_DEVICE_LABEL_CHARS));
    fireEvent.change(input, { target: { value: "Desk" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(sync.replicatedSyncSetDeviceLabel).toHaveBeenCalledWith("device-self", "Desk"));
  });
});

describe("describeTransportHealth", () => {
  it("translates native health into plain language", () => {
    expect(describeTransportHealth(folder)).toEqual({ tone: "ok", label: "Up to date", detail: null });
    expect(describeTransportHealth({ ...folder, pending: 3 }).label).toBe("Uploading 3 changes");
    expect(describeTransportHealth({ ...folder, failed: 1, lastError: "disk full" })).toEqual({ tone: "attention", label: "1 change couldn’t be uploaded", detail: "disk full" });
    expect(describeTransportHealth({ ...folder, health: "degraded: recent transient failures" })).toEqual({ tone: "attention", label: "Having trouble, retrying automatically", detail: "recent transient failures" });
    expect(describeTransportHealth({ ...folder, health: "unavailable: folder missing: /x" })).toEqual({ tone: "attention", label: "Can’t reach this connector", detail: "folder missing: /x" });
    expect(describeTransportHealth({ ...folder, health: "unavailable: not configured" }).label).toMatch(/Not set up correctly/);
    expect(describeTransportHealth({ ...folder, health: "mystery" })).toEqual({ tone: "attention", label: "Status unknown", detail: "mystery" });
  });

  it("shows the plain label on the connector card instead of the raw status", async () => {
    setUp({ status: { state: "enrolled", deviceCount: 1 }, transports: [{ ...folder, health: "unavailable: folder missing" }] });
    render(<ReplicatedSyncSettings />);

    const locations = await screen.findByRole("list", { name: "Connectors" });
    expect(within(locations).getByText("Can’t reach this connector")).toBeInTheDocument();
    expect(within(locations).getByText("folder missing")).toBeInTheDocument();
    expect(within(locations).queryByText(/unavailable:/)).not.toBeInTheDocument();
  });
});

describe("recovery phrase entry", () => {
  const check = (overrides: Partial<sync.RecoveryPhraseCheck>): sync.RecoveryPhraseCheck => ({ wordCount: 0, unknownWordPositions: [], valid: false, ...overrides });

  it("does not flag the word still being typed", () => {
    expect(recoveryPhraseFeedback("abandon abil", check({ wordCount: 2, unknownWordPositions: [1] }))).toEqual({ tone: "ok", text: "2 of 24 words" });
    expect(recoveryPhraseFeedback("abandon abil ", check({ wordCount: 2, unknownWordPositions: [1] }))?.text).toBe("Word 2 isn’t a recovery phrase word. Check its spelling.");
  });

  it("names every misspelled word, too many words, and a bad checksum", () => {
    expect(recoveryPhraseFeedback("a b c ", check({ wordCount: 3, unknownWordPositions: [0, 2] }))?.text).toBe("Words 1 and 3 aren’t recovery phrase words. Check their spelling.");
    expect(recoveryPhraseFeedback("x", check({ wordCount: 25 }))?.text).toBe("That’s 25 words. A recovery phrase has 24.");
    expect(recoveryPhraseFeedback("x", check({ wordCount: 24 }))?.text).toMatch(/don’t form a valid phrase/);
    expect(recoveryPhraseFeedback("x", check({ wordCount: 24, valid: true }))).toEqual({ tone: "ok", text: "Recovery phrase looks right." });
    expect(recoveryPhraseFeedback("", null)).toBeNull();
  });

  it("enables joining only once the phrase checks out, and normalizes spacing", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("existing");
    vi.mocked(sync.replicatedSyncCheckRecoveryPhrase).mockImplementation(async (phrase) =>
      phrase.includes("typo") ? check({ wordCount: 2, unknownWordPositions: [1] }) : check({ wordCount: 24, valid: true }));
    vi.mocked(sync.replicatedSyncJoinWithRecoveryPhrase).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    const input = await screen.findByRole("textbox", { name: "Enter the recovery phrase" });
    const join = screen.getByRole("button", { name: "Join with recovery phrase" });
    fireEvent.change(input, { target: { value: "abandon typo " } });
    expect(await screen.findByText(/Word 2 isn’t a recovery phrase word/)).toBeInTheDocument();
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(join).toBeDisabled();

    fireEvent.change(input, { target: { value: "  good   words  " } });
    expect(await screen.findByText("Recovery phrase looks right.")).toBeInTheDocument();
    expect(join).toBeEnabled();
    fireEvent.click(join);
    await waitFor(() => expect(sync.replicatedSyncJoinWithRecoveryPhrase).toHaveBeenCalledWith("good words"));
  });
});

describe("inline status messages", () => {
  it("shows a cancelled folder picker next to the choose-folder button", async () => {
    setUp({ transports: [] });
    vi.mocked(sync.replicatedSyncAddFolder).mockResolvedValue(null);
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: /^Shared folder/ }));
    const add = screen.getByRole("button", { name: "Choose folder…" });
    fireEvent.click(add);
    const message = await screen.findByText("No folder selected.");
    expect(add.nextElementSibling).toBe(message);
  });

  it("shows a join-request failure beside that button", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("existing");
    vi.mocked(sync.replicatedSyncRequestEnrollment).mockRejectedValue("No location accepted the request.");
    render(<ReplicatedSyncSettings />);

    const request = await screen.findByRole("button", { name: "Ask another device to approve this one" });
    fireEvent.click(request);
    const message = await screen.findByText("No location accepted the request.");
    expect(request.nextElementSibling).toBe(message);
  });
});

describe("leaving the sync group", () => {
  it("leaves from Advanced only after confirmation", async () => {
    setUp({ status: { state: "enrolled", deviceCount: 2 } });
    vi.mocked(sync.replicatedSyncLeave).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Leave this sync group…" }));
    const confirm = screen.getByRole("group", { name: "Leave this sync group?" });
    expect(confirm).toHaveTextContent(/data stay on this device/);
    expect(confirm).toHaveTextContent(/until you revoke it from one of them/);
    fireEvent.click(within(confirm).getByRole("button", { name: "Keep" }));
    expect(sync.replicatedSyncLeave).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Leave this sync group…" }));
    fireEvent.click(screen.getByRole("button", { name: "Leave sync group" }));
    await waitFor(() => expect(sync.replicatedSyncLeave).toHaveBeenCalledTimes(1));
  });

  it("lets a device waiting for approval cancel and start over", async () => {
    setUp({ status: { state: "awaitingGrant", requestId: "req-1", fingerprint: "AB12-CD34-EF56-0789", createdAt: "2026-09-22T10:00:00Z" } });
    vi.mocked(sync.replicatedSyncLeave).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Cancel and start over…" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel request" }));
    await waitFor(() => expect(sync.replicatedSyncLeave).toHaveBeenCalledTimes(1));
  });
});

describe("replicated sync protocol reset notice", () => {
  it("explains once that the update reset sync, and can be dismissed", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("none");
    vi.mocked(sync.replicatedSyncProtocolResetNotice).mockResolvedValueOnce(true).mockResolvedValue(false);
    vi.mocked(sync.replicatedSyncDismissProtocolResetNotice).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    const notice = await screen.findByText(/This update reset sync on this device/);
    expect(notice).toHaveTextContent(/Your data here is kept/);
    fireEvent.click(within(notice.closest(".notice--warning") as HTMLElement).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(sync.replicatedSyncDismissProtocolResetNotice).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByText(/This update reset sync on this device/)).not.toBeInTheDocument());
  });

  it("says nothing when sync wasn't reset", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("none");
    render(<ReplicatedSyncSettings />);

    expect(await screen.findByRole("button", { name: "Create a new sync group" })).toBeInTheDocument();
    expect(screen.queryByText(/This update reset sync/)).not.toBeInTheDocument();
  });
});
