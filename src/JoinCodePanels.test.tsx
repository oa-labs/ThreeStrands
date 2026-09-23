import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./replicatedSync", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./replicatedSync")>();
  const mocked = Object.fromEntries(
    Object.entries(actual).map(([name, value]) => [name, typeof value === "function" ? vi.fn() : value]),
  );
  return mocked;
});

import * as sync from "./replicatedSync";
import { AddDevicePanel, describeJoinCode, JoinCodeNotices, JoinCodePanel, joinCodeReadiness, OutstandingJoinCodes } from "./JoinCodePanels";
import { useSettingsOperation } from "./settingsOperations";
import { formatTimeUntil, type Operation } from "./syncSettingsParts";

const refresh = vi.fn(async () => {});

function Harness({ children }: { children: (operation: Operation) => ReactNode }) {
  const operation = useSettingsOperation(refresh);
  return <>{children(operation)}</>;
}

const inAnHour = () => new Date(Date.now() + 60 * 60_000).toISOString();

function preview(overrides: Partial<sync.JoinCodePreview> = {}): sync.JoinCodePreview {
  return {
    inviterName: "Work laptop",
    expiresAt: inAnHour(),
    expired: false,
    connectors: [
      {
        index: 0,
        kind: "folder",
        supported: true,
        location: "ThreeStrands",
        label: null,
        credentialsIncluded: false,
        needsFolder: true,
        folderName: "ThreeStrands",
        needsCredentials: false,
      },
      {
        index: 1,
        kind: "s3",
        supported: true,
        location: "https://s3.example.com · sync-bucket",
        label: "Team bucket",
        credentialsIncluded: false,
        needsFolder: false,
        folderName: null,
        needsCredentials: true,
      },
    ],
    ...overrides,
  };
}

const folderStatus: sync.ReplicatedSyncTransportStatus = {
  instanceId: "folder-1",
  kind: "folder",
  location: "/Users/me/Dropbox/ThreeStrands",
  supportsDeleteData: true,
  health: "healthy",
  headDiscovery: true,
  pending: 0,
  delivered: 0,
  failed: 0,
};
const s3Status: sync.ReplicatedSyncTransportStatus = { ...folderStatus, instanceId: "s3-1", kind: "s3", location: "https://s3.example.com · sync-bucket", label: "Team bucket" };

beforeEach(() => {
  vi.clearAllMocks();
  Object.defineProperty(navigator, "clipboard", { value: { writeText: vi.fn(async () => {}) }, configurable: true });
});
afterEach(cleanup);

describe("join code readiness", () => {
  it("waits for every folder choice and missing credential", () => {
    const code = preview();
    expect(joinCodeReadiness(null, {}, {})).toEqual({ ready: false, missing: [] });
    const nothing = joinCodeReadiness(code, {}, {});
    expect(nothing.ready).toBe(false);
    expect(nothing.missing).toEqual([
      "Choose this device’s copy of “ThreeStrands”.",
      "Enter the access key for https://s3.example.com · sync-bucket.",
    ]);
    const credentials = { 1: { accessKeyId: "AKIA", secretAccessKey: "s", sessionToken: "" } };
    expect(joinCodeReadiness(code, { 0: "/Users/me/Sync" }, credentials)).toEqual({ ready: true, missing: [] });
    expect(joinCodeReadiness(code, { 0: "/x" }, { 1: { ...credentials[1], secretAccessKey: " " } }).ready).toBe(false);
  });

  it("refuses an expired code or one with no usable connector", () => {
    expect(joinCodeReadiness(preview({ expired: true }), {}, {}).missing).toEqual(["This join code expired. Ask for a new one."]);
    const unsupported = preview({ connectors: [{ ...preview().connectors[0]!, kind: "pigeon", supported: false, needsFolder: false }] });
    expect(joinCodeReadiness(unsupported, {}, {}).missing[0]).toMatch(/Update this app/);
  });

  it("describes times and code states in words", () => {
    const now = Date.parse("2026-09-23T12:00:00Z");
    expect(formatTimeUntil("2026-09-23T12:05:00Z", now)).toBe("in 5 minutes");
    expect(formatTimeUntil("2026-09-23T17:00:00Z", now)).toBe("in 5 hours");
    expect(formatTimeUntil("2026-09-30T12:00:00Z", now)).toBe("in 7 days");
    expect(formatTimeUntil("2026-09-23T11:00:00Z", now)).toBe("now");
    const code: sync.OutstandingJoinCode = { invitationCid: "c", createdAt: "2026-09-23T11:00:00Z", expiresAt: "2026-09-23T15:00:00Z", status: "open", rejectedAttempts: 0 };
    expect(describeJoinCode(code, now)).toBe("Open · expires in 3 hours");
    expect(describeJoinCode({ ...code, status: "redeemed", redeemedByName: "Phone" })).toBe("Used by Phone");
    expect(describeJoinCode({ ...code, status: "expired" })).toBe("Expired unused");
    expect(describeJoinCode({ ...code, status: "cancelled" })).toBe("Cancelled");
  });
});

describe("joining with a pasted code", () => {
  function renderPanel() {
    render(<Harness>{(operation) => <JoinCodePanel operation={operation} refresh={refresh} />}</Harness>);
    return screen.getByRole("textbox", { name: "Join code" });
  }

  it("previews the code, collects what this device must supply, then joins", async () => {
    vi.mocked(sync.replicatedSyncPreviewJoinCode).mockResolvedValue(preview());
    vi.mocked(sync.replicatedSyncPickJoinFolder).mockResolvedValue("/Users/me/Dropbox/ThreeStrands");
    vi.mocked(sync.replicatedSyncJoinWithCode).mockResolvedValue(undefined);
    const input = renderPanel();
    const join = screen.getByRole("button", { name: "Join sync group" });

    fireEvent.change(input, { target: { value: "TSJOIN1-abc\n" } });
    const connectors = await screen.findByRole("list", { name: "Connectors in this join code" });
    expect(sync.replicatedSyncPreviewJoinCode).toHaveBeenCalledWith("TSJOIN1-abc\n");
    expect(screen.getByText(/^From Work laptop · expires in/)).toBeInTheDocument();
    const [folderRow, s3Row] = within(connectors).getAllByRole("listitem");
    expect(folderRow).toHaveTextContent("On Work laptop this folder is named “ThreeStrands”");
    expect(s3Row).toHaveTextContent("Team bucket");
    expect(s3Row).toHaveTextContent("You’ll enter credentials");
    expect(join).toBeDisabled();

    fireEvent.click(within(folderRow!).getByRole("button", { name: "Choose folder…" }));
    expect(await within(folderRow!).findByText("/Users/me/Dropbox/ThreeStrands")).toBeInTheDocument();
    expect(join).toBeDisabled();
    fireEvent.change(within(s3Row!).getByLabelText("Access key ID"), { target: { value: "AKIA" } });
    fireEvent.change(within(s3Row!).getByLabelText("Secret access key"), { target: { value: "secret" } });
    expect(join).toBeEnabled();

    fireEvent.click(join);
    await waitFor(() => expect(sync.replicatedSyncJoinWithCode).toHaveBeenCalledWith("TSJOIN1-abc\n", {
      folders: [{ connectorIndex: 0, path: "/Users/me/Dropbox/ThreeStrands" }],
      credentials: [{ connectorIndex: 1, credentials: { kind: "s3", accessKeyId: "AKIA", secretAccessKey: "secret", sessionToken: null } }],
    }));
    await waitFor(() => expect(input).toHaveValue(""));
    expect(refresh).toHaveBeenCalled();
  });

  it("joins a code that includes everything with no further input", async () => {
    vi.mocked(sync.replicatedSyncPreviewJoinCode).mockResolvedValue(
      preview({ connectors: [{ ...preview().connectors[1]!, index: 0, credentialsIncluded: true, needsCredentials: false }] }),
    );
    vi.mocked(sync.replicatedSyncJoinWithCode).mockResolvedValue(undefined);
    fireEvent.change(renderPanel(), { target: { value: "TSJOIN1-full" } });
    expect(await screen.findByText(/Credentials included/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Join sync group" }));
    await waitFor(() => expect(sync.replicatedSyncJoinWithCode).toHaveBeenCalledWith("TSJOIN1-full", { folders: [], credentials: [] }));
  });

  it("shows why a code can't be read, or a join failed, next to it", async () => {
    vi.mocked(sync.replicatedSyncPreviewJoinCode).mockRejectedValue("This join code is incomplete or mistyped. Copy it again.");
    const input = renderPanel();
    fireEvent.change(input, { target: { value: "TSJOIN1-trunc" } });
    expect(await screen.findByText("This join code is incomplete or mistyped. Copy it again.")).toBeInTheDocument();
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: "Join sync group" })).toBeDisabled();

    vi.mocked(sync.replicatedSyncPreviewJoinCode).mockResolvedValue(
      preview({ connectors: [{ ...preview().connectors[1]!, index: 0, credentialsIncluded: true, needsCredentials: false }] }),
    );
    vi.mocked(sync.replicatedSyncJoinWithCode).mockRejectedValue("Couldn't find this join code's invitation in its connectors.");
    fireEvent.change(input, { target: { value: "TSJOIN1-ok" } });
    const join = screen.getByRole("button", { name: "Join sync group" });
    await waitFor(() => expect(join).toBeEnabled());
    fireEvent.click(join);
    const failure = await screen.findByText("Couldn't find this join code's invitation in its connectors.");
    expect(screen.getByRole("button", { name: "Join sync group" }).nextElementSibling).toBe(failure);
    expect(input).toHaveValue("TSJOIN1-ok");
  });

  it("marks an expired code and a connector this version can't use", async () => {
    vi.mocked(sync.replicatedSyncPreviewJoinCode).mockResolvedValue(
      preview({
        expired: true,
        connectors: [{ ...preview().connectors[0]!, kind: "pigeon", supported: false, needsFolder: false, location: "", label: null }],
      }),
    );
    fireEvent.change(renderPanel(), { target: { value: "TSJOIN1-old" } });
    expect((await screen.findAllByText("This join code expired. Ask for a new one.")).length).toBeGreaterThan(0);
    expect(screen.getByText(/can’t use this connector, so it will be skipped/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Join sync group" })).toBeDisabled();
  });

  it("forgets the pasted code when the panel closes", async () => {
    vi.mocked(sync.replicatedSyncPreviewJoinCode).mockResolvedValue(preview());
    const { rerender } = render(<Harness>{(operation) => <JoinCodePanel operation={operation} refresh={refresh} />}</Harness>);
    fireEvent.change(screen.getByRole("textbox", { name: "Join code" }), { target: { value: "TSJOIN1-secret" } });
    rerender(<Harness>{() => null}</Harness>);
    rerender(<Harness>{(operation) => <JoinCodePanel operation={operation} refresh={refresh} />}</Harness>);
    expect(screen.getByRole("textbox", { name: "Join code" })).toHaveValue("");
  });
});

describe("adding a device", () => {
  function renderPanel(onClose = vi.fn()) {
    render(<Harness>{(operation) => <AddDevicePanel transports={[folderStatus, s3Status]} operation={operation} refresh={refresh} onClose={onClose} />}</Harness>);
    return screen.getByRole("group", { name: "Add a device" });
  }

  it("creates a code with the chosen lifetime, connectors, and credentials", async () => {
    vi.mocked(sync.replicatedSyncCreateJoinCode).mockResolvedValue("TSJOIN1-created");
    const panel = renderPanel();
    expect(within(panel).getByLabelText("Expires after")).toHaveValue("24");
    expect(within(panel).getByText("The other device will choose its own copy of this folder.")).toBeInTheDocument();
    fireEvent.change(within(panel).getByLabelText("Expires after"), { target: { value: "168" } });
    fireEvent.click(within(panel).getByRole("checkbox", { name: "Include credentials" }));
    fireEvent.click(within(panel).getByRole("button", { name: "Create join code" }));

    await waitFor(() => expect(sync.replicatedSyncCreateJoinCode).toHaveBeenCalledWith(168, [
      { instanceId: "folder-1", includeCredentials: false },
      { instanceId: "s3-1", includeCredentials: false },
    ]));
    expect(await within(panel).findByRole("textbox", { name: "Join code" })).toHaveValue("TSJOIN1-created");
    expect(panel).toHaveTextContent("Anyone with this code can join your sync group.");
    expect(panel).not.toHaveTextContent("included storage credentials");
    expect(panel).toHaveTextContent("It works once and expires in 7 days");
  });

  it("warns about included credentials, copies the code, and discards it when closed", async () => {
    vi.mocked(sync.replicatedSyncCreateJoinCode).mockResolvedValue("TSJOIN1-with-keys");
    const onClose = vi.fn();
    const panel = renderPanel(onClose);
    fireEvent.click(within(panel).getByRole("button", { name: "Create join code" }));
    await within(panel).findByRole("textbox", { name: "Join code" });
    expect(panel).toHaveTextContent("use the included storage credentials");
    expect(panel).toHaveTextContent("a key limited to this bucket is safest");

    fireEvent.click(within(panel).getByRole("button", { name: "Copy" }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("TSJOIN1-with-keys");
    expect(await within(panel).findByRole("button", { name: "Copied" })).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("button", { name: "Done" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("needs at least one connector", () => {
    const panel = renderPanel();
    for (const checkbox of within(panel).getAllByRole("checkbox").filter((box) => !box.closest(".sync-join-credentials"))) {
      fireEvent.click(checkbox);
    }
    expect(within(panel).getByRole("button", { name: "Create join code" })).toBeDisabled();
    expect(within(panel).getByRole("checkbox", { name: "Include credentials" })).toBeDisabled();
  });
});

describe("outstanding join codes and notices", () => {
  const open: sync.OutstandingJoinCode = { invitationCid: "c-open", createdAt: "2026-09-23T11:00:00Z", expiresAt: inAnHour(), status: "open", rejectedAttempts: 0 };

  it("lists open and recent codes and cancels only after confirmation", async () => {
    vi.mocked(sync.replicatedSyncCancelJoinCode).mockResolvedValue(undefined);
    const used: sync.OutstandingJoinCode = { ...open, invitationCid: "c-used", status: "redeemed", redeemedByName: "Phone", rejectedAttempts: 1 };
    render(<Harness>{(operation) => <OutstandingJoinCodes codes={[used, open]} operation={operation} />}</Harness>);
    const [openRow, usedRow] = within(screen.getByRole("list", { name: "Join codes" })).getAllByRole("listitem");
    expect(openRow).toHaveTextContent(/^Open · expires in/);
    expect(usedRow).toHaveTextContent("Used by Phone");
    expect(usedRow).toHaveTextContent("Refused 1 later attempt to use this code");
    expect(within(usedRow!).queryByRole("button", { name: "Cancel…" })).not.toBeInTheDocument();

    fireEvent.click(within(openRow!).getByRole("button", { name: "Cancel…" }));
    fireEvent.click(within(openRow!).getByRole("button", { name: "Keep" }));
    expect(sync.replicatedSyncCancelJoinCode).not.toHaveBeenCalled();
    fireEvent.click(within(openRow!).getByRole("button", { name: "Cancel…" }));
    expect(within(openRow!).getByRole("group", { name: "Cancel join code confirmation" })).toHaveTextContent(/keys change/);
    fireEvent.click(within(openRow!).getByRole("button", { name: "Cancel code" }));
    await waitFor(() => expect(sync.replicatedSyncCancelJoinCode).toHaveBeenCalledWith("c-open"));
  });

  it("shows nothing when there are no codes", () => {
    render(<Harness>{(operation) => <OutstandingJoinCodes codes={[]} operation={operation} />}</Harness>);
    expect(screen.queryByRole("list", { name: "Join codes" })).not.toBeInTheDocument();
  });

  it("says who joined, offers revoking them, and dismisses notices", async () => {
    vi.mocked(sync.replicatedSyncRotateEpoch).mockResolvedValue(undefined);
    vi.mocked(sync.replicatedSyncDismissJoinCodeNotice).mockResolvedValue(undefined);
    const notices: sync.JoinCodeNotice[] = [
      { redemptionCid: "r-joined", kind: "joined", deviceId: "device-phone", deviceName: "Phone", inviterDeviceId: "d1", inviterName: "Desk", at: "" },
      { redemptionCid: "r-refused", kind: "rejectedAttempt", deviceId: "device-x", deviceName: "Unknown laptop", inviterDeviceId: null, inviterName: null, at: "" },
    ];
    render(<Harness>{(operation) => <JoinCodeNotices notices={notices} operation={operation} />}</Harness>);
    const [joined, refused] = within(screen.getByRole("list", { name: "Join code notices" })).getAllByRole("listitem");
    expect(joined).toHaveTextContent("Phone joined with a join code from Desk.");
    expect(refused).toHaveTextContent("A device named “Unknown laptop” tried to use a join code that was already used, expired, or cancelled.");
    expect(within(refused!).queryByRole("button", { name: "Not you? Revoke…" })).not.toBeInTheDocument();

    fireEvent.click(within(joined!).getByRole("button", { name: "Not you? Revoke…" }));
    fireEvent.click(within(joined!).getByRole("button", { name: "Revoke device" }));
    await waitFor(() => expect(sync.replicatedSyncRotateEpoch).toHaveBeenCalledWith("device-phone"));

    fireEvent.click(within(refused!).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(sync.replicatedSyncDismissJoinCodeNotice).toHaveBeenCalledWith("r-refused"));
  });
});
