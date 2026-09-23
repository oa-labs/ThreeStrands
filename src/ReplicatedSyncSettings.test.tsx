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
    expect(screen.getByText(/one dedicated Filebase bucket for the sync space/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add a sync folder" })).toHaveClass("primary-action");
    const ipfs = screen.getByText("Use an IPFS RPC endpoint instead").closest("details")!;
    expect(ipfs).not.toHaveAttribute("open");
    expect(within(ipfs).getByRole("button", { name: "Add IPFS RPC endpoint" })).toBeInTheDocument();
    expect(within(ipfs).getByText(/bucket-specific RPC token/)).toBeInTheDocument();
  });

  it("shows an empty RPC URL as empty and enables the connection test once the Filebase URL is filled in", async () => {
    setUp({ transports: [] });
    render(<ReplicatedSyncSettings />);

    const ipfs = (await screen.findByText("Use an IPFS RPC endpoint instead")).closest("details")!;
    const url = within(ipfs).getByRole("textbox", { name: "RPC base URL" });
    expect(url).toHaveValue("");
    expect(url).not.toHaveAttribute("placeholder", "https://rpc.filebase.io");
    expect(within(ipfs).getByRole("button", { name: "Test connection" })).toBeDisabled();

    fireEvent.click(within(ipfs).getByRole("button", { name: "Fill in Filebase URL" }));
    expect(url).toHaveValue("https://rpc.filebase.io");
    expect(within(ipfs).getByRole("button", { name: "Test connection" })).toBeEnabled();
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
    const devices = within(screen.getByRole("list", { name: "Devices" })).getAllByRole("listitem");
    expect(devices[0]).toHaveTextContent("This device");
    expect(devices[1]).toHaveTextContent("Unnamed device");
    expect(devices[1]).toHaveTextContent("ID device-o");
    expect(within(devices[1]!).getByRole("button", { name: "Revoke…" })).toBeInTheDocument();
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
    fireEvent.click(within(waiting).getByRole("button", { name: "Review…" }));
    fireEvent.click(within(waiting).getByRole("button", { name: "Codes match — approve" }));
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
    expect(describeTransportHealth({ ...folder, health: "unavailable: folder missing: /x" })).toEqual({ tone: "attention", label: "Can’t reach this location", detail: "folder missing: /x" });
    expect(describeTransportHealth({ ...folder, health: "unavailable: not configured" }).label).toMatch(/Not set up correctly/);
    expect(describeTransportHealth({ ...folder, health: "mystery" })).toEqual({ tone: "attention", label: "Status unknown", detail: "mystery" });
  });

  it("shows the plain label on the location card instead of the raw status", async () => {
    setUp({ status: { state: "enrolled", deviceCount: 1 }, transports: [{ ...folder, health: "unavailable: folder missing" }] });
    render(<ReplicatedSyncSettings />);

    const locations = await screen.findByRole("list", { name: "Sync locations" });
    expect(within(locations).getByText("Can’t reach this location")).toBeInTheDocument();
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

    const input = await screen.findByRole("textbox", { name: "Or join with a recovery phrase" });
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
  it("shows a cancelled folder picker next to the add-folder button", async () => {
    setUp({ transports: [] });
    vi.mocked(sync.replicatedSyncAddFolder).mockResolvedValue(null);
    render(<ReplicatedSyncSettings />);

    const add = await screen.findByRole("button", { name: "Add a sync folder" });
    fireEvent.click(add);
    const message = await screen.findByText("No folder selected.");
    expect(add.nextElementSibling).toBe(message);
  });

  it("shows a join-request failure beside that button", async () => {
    vi.mocked(sync.replicatedSyncInspectSpace).mockResolvedValue("existing");
    vi.mocked(sync.replicatedSyncRequestEnrollment).mockRejectedValue("No location accepted the request.");
    render(<ReplicatedSyncSettings />);

    const request = await screen.findByRole("button", { name: "Request to join from an existing device" });
    fireEvent.click(request);
    const message = await screen.findByText("No location accepted the request.");
    expect(request.nextElementSibling).toBe(message);
  });
});

describe("leaving the sync space", () => {
  it("leaves from Advanced only after confirmation", async () => {
    setUp({ status: { state: "enrolled", deviceCount: 2 } });
    vi.mocked(sync.replicatedSyncLeave).mockResolvedValue(undefined);
    render(<ReplicatedSyncSettings />);

    fireEvent.click(await screen.findByRole("button", { name: "Leave this sync space…" }));
    const confirm = screen.getByRole("group", { name: "Leave this sync space?" });
    expect(confirm).toHaveTextContent(/data stay on this device/);
    expect(confirm).toHaveTextContent(/until you revoke it from one of them/);
    fireEvent.click(within(confirm).getByRole("button", { name: "Keep" }));
    expect(sync.replicatedSyncLeave).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Leave this sync space…" }));
    fireEvent.click(screen.getByRole("button", { name: "Leave sync space" }));
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
