import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ImapAccountSetup } from "./ImapAccountSetup";
import { AccountsSettings } from "./AccountsSettings";
import { mailClient } from "./data/client";
import type { Account, ImapCertificateProbe, ImapMailboxMapping } from "./domain";

vi.mock("./data/client", () => ({ mailClient: {
  discoverImapSettings: vi.fn(), probeImapCertificate: vi.fn(),
  probeSmtpCertificate: vi.fn(), testAndSaveImapAccount: vi.fn(),
  discoverImapMailboxes: vi.fn(), commitImapMailboxMapping: vi.fn(),
} }));

const account: Account = { email: "me@example.com", provider: "imap", displayName: null,
  color: "#112233", status: "connected", sortOrder: 0, connectedAt: "2026-10-10", lastSyncedAt: null };
const probe = (fingerprint: string, trusted = false): ImapCertificateProbe => ({
  certificate: { subject: "test server", issuer: "test issuer", sha256Fingerprint: fingerprint },
  trustedByPlatform: trusted,
});
const incoming = () => within(screen.getByRole("group", { name: "Incoming mail (IMAP)" }));
const outgoing = () => within(screen.getByRole("group", { name: "Outgoing mail (SMTP)" }));
function fillManualSettings() {
  fireEvent.change(screen.getByLabelText("Email address"), { target: { value: "me@example.com" } });
  fireEvent.change(incoming().getByLabelText("Host"), { target: { value: "incoming.example.com" } });
  fireEvent.change(outgoing().getByLabelText("Host"), { target: { value: "outgoing.example.com" } });
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "secret" } });
}
async function check(kind: "IMAP" | "SMTP") {
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: `Check ${kind} certificate` })); });
}
function trust(kind: "IMAP" | "SMTP") {
  fireEvent.click(screen.getByRole("button", { name: `Trust this ${kind} certificate for this server` }));
}
async function save() {
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Test and save" })); });
}
async function confirmMapping() {
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Confirm mapping" })); });
}
async function skipMapping() {
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Skip for now" })); });
}

describe("IMAP account setup", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(mailClient.discoverImapSettings).mockResolvedValue(null);
    vi.mocked(mailClient.probeImapCertificate).mockResolvedValue(probe("incoming-pin"));
    vi.mocked(mailClient.probeSmtpCertificate).mockResolvedValue(probe("outgoing-pin"));
    vi.mocked(mailClient.testAndSaveImapAccount).mockResolvedValue(account);
    vi.mocked(mailClient.discoverImapMailboxes).mockResolvedValue({ proposals: [], selectable: [], labelContainers: [] });
    vi.mocked(mailClient.commitImapMailboxMapping).mockResolvedValue(undefined);
  });
  afterEach(cleanup);

  it("opens manual IMAP setup from accounts settings and refreshes saved accounts while preserving Gmail sign-in", async () => {
    const onAdd = vi.fn().mockResolvedValue(undefined);
    const refresh = vi.fn().mockResolvedValue(undefined);
    render(<AccountsSettings authStatus={null} accounts={[]} onAdd={onAdd} onImapConnected={refresh}
      onRemove={vi.fn()} onRemoveEverywhere={vi.fn()} onReconnect={vi.fn()}
      onSetDisplayName={vi.fn()} onSetColor={vi.fn()} onReorder={vi.fn()} />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Add Account" })); });
    expect(onAdd).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Add IMAP account" }));
    expect(screen.getByRole("heading", { name: "Add an IMAP account" })).toBeInTheDocument();
    fillManualSettings();
    await save();
    expect(mailClient.discoverImapSettings).not.toHaveBeenCalled();
    // New contract (Slice 3): a successful test-and-save does NOT finish setup
    // immediately — it discovers mailboxes and presents the mapping review.
    // The saved-account refresh fires only once the user confirms or skips.
    expect(mailClient.discoverImapMailboxes).toHaveBeenCalledWith("me@example.com");
    expect(screen.getByRole("heading", { name: "Map your mailboxes" })).toBeInTheDocument();
    expect(refresh).not.toHaveBeenCalled();
    await skipMapping();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("heading", { name: "Add an IMAP account" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Map your mailboxes" })).not.toBeInTheDocument();
    expect(onAdd).toHaveBeenCalledTimes(1);
  });

  it("requires separate trust decisions and sends distinct pins for IMAP and SMTP", async () => {
    const onConnected = vi.fn();
    render(<ImapAccountSetup onConnected={onConnected} />);
    fillManualSettings();
    await check("IMAP");
    expect(screen.getByRole("button", { name: "Test and save" })).toBeDisabled();
    trust("IMAP");
    await check("SMTP");
    expect(mailClient.probeImapCertificate).toHaveBeenCalledWith("incoming.example.com", 143, "start_tls");
    expect(mailClient.probeSmtpCertificate).toHaveBeenCalledWith("outgoing.example.com", 587, "start_tls");
    expect(screen.getByRole("button", { name: "Test and save" })).toBeDisabled();
    expect(mailClient.testAndSaveImapAccount).not.toHaveBeenCalled();
    trust("SMTP");
    await save();
    expect(mailClient.testAndSaveImapAccount).toHaveBeenCalledWith(expect.objectContaining({
      imapPinnedFingerprint: "incoming-pin", smtpPinnedFingerprint: "outgoing-pin",
      imapUsername: "me@example.com", smtpUsername: "me@example.com", imapPassword: "secret",
    }));
    // New contract (Slice 3): the account is saved but onConnected is deferred
    // until the user confirms the mailbox mapping.
    expect(onConnected).not.toHaveBeenCalled();
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalled();
    expect(onConnected).toHaveBeenCalledWith(account);
  });

  it("defaults to creating Archive when discovery finds no Archive mailbox", async () => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await save();
    expect(screen.getByLabelText("Create a new Archive mailbox")).toBeChecked();
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith({
      email: account.email, archive: null, createArchive: "Archive",
      mailboxOverrides: {}, labelContainer: null,
    });
  });

  it.each(["special_use", "name_match"] as const)("keeps an existing %s Archive proposal", async (source) => {
    vi.mocked(mailClient.discoverImapMailboxes).mockResolvedValue({
      proposals: [{ role: "archive", mailbox: "Storage", source }],
      selectable: [{ name: "Storage", delimiter: "/", specialUse: null }],
      labelContainers: [],
    });
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await save();
    expect(screen.getByLabelText("Create a new Archive mailbox")).not.toBeChecked();
    expect(screen.getByRole("combobox", { name: "Archive mailbox" })).toHaveValue("Storage");
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith(expect.objectContaining({
      archive: "Storage", createArchive: null,
    }));
  });

  it("requires an Archive choice when the user turns off creation", async () => {
    vi.mocked(mailClient.discoverImapMailboxes).mockResolvedValue({
      proposals: [], selectable: [{ name: "Storage", delimiter: "/", specialUse: null }], labelContainers: [],
    });
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await save();
    fireEvent.click(screen.getByLabelText("Create a new Archive mailbox"));
    expect(screen.getByRole("button", { name: "Confirm mapping" })).toBeDisabled();
    fireEvent.change(screen.getByRole("combobox", { name: "Archive mailbox" }), { target: { value: "Storage" } });
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith(expect.objectContaining({
      archive: "Storage", createArchive: null,
    }));
  });

  it("offers non-selectable label containers separately from system mailboxes", async () => {
    vi.mocked(mailClient.discoverImapMailboxes).mockResolvedValue({
      proposals: [], selectable: [{ name: "Work", delimiter: "/", specialUse: null }],
      labelContainers: [
        { name: "Labels", delimiter: "/", specialUse: null },
        { name: "Work", delimiter: "/", specialUse: null },
      ],
    });
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await save();
    const container = screen.getByRole("combobox", { name: "Label container mailbox" });
    expect(within(container).getByRole("option", { name: "Labels" })).toBeInTheDocument();
    expect(within(container).getByRole("option", { name: "Work" })).toBeInTheDocument();
    expect(within(screen.getByRole("combobox", { name: "Sent mailbox" }))
      .queryByRole("option", { name: "Labels" })).not.toBeInTheDocument();
    fireEvent.change(container, { target: { value: "Labels" } });
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith(expect.objectContaining({ labelContainer: "Labels" }));
  });

  it.each(["keywords", "none"])("hides label-container selection in %s mode", async (mode) => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    fireEvent.change(screen.getByLabelText("How this account stores labels"), { target: { value: mode } });
    await save();
    expect(screen.queryByRole("combobox", { name: "Label container mailbox" })).not.toBeInTheDocument();
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith(expect.objectContaining({ labelContainer: null }));
  });

  it("preserves selected mailbox identifiers including surrounding whitespace", async () => {
    const mapping: ImapMailboxMapping = {
      proposals: [
        { role: "archive", mailbox: " Archive ", source: "special_use" },
        { role: "sent", mailbox: "Sent ", source: "special_use" },
      ],
      selectable: [" Archive ", "Sent ", " Junk"].map((name) => ({ name, delimiter: "/", specialUse: null })),
      labelContainers: [{ name: " Labels ", delimiter: "/", specialUse: null }],
    };
    vi.mocked(mailClient.discoverImapMailboxes).mockResolvedValue(mapping);
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    fireEvent.change(screen.getByLabelText("Container mailbox (e.g. Labels)"), { target: { value: " Labels " } });
    await save();
    expect(mailClient.testAndSaveImapAccount).toHaveBeenCalledWith(expect.objectContaining({ labelContainer: " Labels " }));
    fireEvent.change(screen.getByRole("combobox", { name: "Junk / Spam mailbox" }), { target: { value: " Junk" } });
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith({
      email: account.email, archive: " Archive ", createArchive: null,
      mailboxOverrides: { sent: "Sent ", junk: " Junk" }, labelContainer: " Labels ",
    });
  });

  it("preserves a manually entered container and lets the user clear it after discovery", async () => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    fireEvent.change(screen.getByLabelText("Container mailbox (e.g. Labels)"), { target: { value: "Custom" } });
    await save();
    const container = screen.getByRole("combobox", { name: "Label container mailbox" });
    expect(container).toHaveValue("Custom");
    fireEvent.change(container, { target: { value: "" } });
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith(expect.objectContaining({ labelContainer: null }));
  });

  it("preserves the exact name requested for Archive creation", async () => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await save();
    fireEvent.change(screen.getByLabelText("New Archive mailbox name"), { target: { value: " Archive " } });
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith(expect.objectContaining({ createArchive: " Archive " }));
  });

  it("lets the user finish without creating anything when discovery fails and mapping is skipped", async () => {
    vi.mocked(mailClient.discoverImapMailboxes).mockRejectedValue("Unavailable");
    const onConnected = vi.fn();
    render(<ImapAccountSetup onConnected={onConnected} />);
    fillManualSettings();
    await save();
    expect(screen.getByRole("alert")).toHaveTextContent("mailbox discovery failed");
    await skipMapping();
    expect(mailClient.commitImapMailboxMapping).not.toHaveBeenCalled();
    expect(onConnected).toHaveBeenCalledWith(account);
  });

  it("requires a fresh trust decision when a new probe presents a changed certificate", async () => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await check("IMAP"); trust("IMAP");
    vi.mocked(mailClient.probeImapCertificate).mockResolvedValue(probe("changed-pin"));
    await check("IMAP");
    expect(screen.getByRole("button", { name: "Test and save" })).toBeDisabled();
    expect(screen.getByText("changed-pin")).toBeInTheDocument();
    trust("IMAP");
    await save();
    expect(mailClient.testAndSaveImapAccount).toHaveBeenCalledWith(expect.objectContaining({
      imapPinnedFingerprint: "changed-pin", smtpPinnedFingerprint: null,
    }));
  });

  it("leaves CA-trusted SMTP unpinned when IMAP requires a pin", async () => {
    vi.mocked(mailClient.probeSmtpCertificate).mockResolvedValue(probe("public-ca", true));
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await check("IMAP"); trust("IMAP");
    await check("SMTP");
    expect(screen.queryByRole("button", { name: /Trust this SMTP/ })).not.toBeInTheDocument();
    await save();
    expect(mailClient.testAndSaveImapAccount).toHaveBeenCalledWith(expect.objectContaining({
      imapPinnedFingerprint: "incoming-pin", smtpPinnedFingerprint: null,
    }));
  });

  it.each([
    ["IMAP", "Host", "changed.example.com"], ["IMAP", "Port", "1993"], ["IMAP", "Security", "implicit_tls"],
    ["SMTP", "Host", "changed.example.com"], ["SMTP", "Port", "2465"], ["SMTP", "Security", "implicit_tls"],
  ] as const)("invalidates the %s pin when %s changes", async (kind, label, value) => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await check("IMAP"); trust("IMAP");
    await check("SMTP"); trust("SMTP");
    fireEvent.change((kind === "IMAP" ? incoming() : outgoing()).getByLabelText(label), { target: { value } });
    expect(screen.queryByRole("heading", { name: `Review the ${kind} server's certificate` })).not.toBeInTheDocument();
    await save();
    expect(mailClient.testAndSaveImapAccount).toHaveBeenCalledWith(expect.objectContaining({
      imapPinnedFingerprint: kind === "IMAP" ? null : "incoming-pin",
      smtpPinnedFingerprint: kind === "SMTP" ? null : "outgoing-pin",
    }));
  });

  it("uses both discovered usernames and allows editing the discovered settings", async () => {
    vi.mocked(mailClient.discoverImapSettings).mockResolvedValue({ source: "isp-autoconfig",
      imap: { host: "imap.example.com", port: 993, security: "implicit_tls", username: "incoming-user" },
      smtp: { host: "smtp.example.com", port: 465, security: "implicit_tls", username: "outgoing-user" },
    });
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Find settings automatically" })); });
    expect(incoming().getByLabelText("Host")).toHaveValue("imap.example.com");
    expect(outgoing().getByLabelText("Port")).toHaveValue(465);
    fireEvent.change(outgoing().getByLabelText("Host"), { target: { value: "manual.example.com" } });
    await save();
    expect(mailClient.testAndSaveImapAccount).toHaveBeenCalledWith(expect.objectContaining({
      imapUsername: "incoming-user", smtpUsername: "outgoing-user", smtpHost: "manual.example.com",
    }));
  });

  it("disables endpoint editing while a certificate probe is pending", async () => {
    let resolve!: (value: ImapCertificateProbe) => void;
    vi.mocked(mailClient.probeImapCertificate).mockReturnValue(new Promise((done) => { resolve = done; }));
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    fireEvent.click(screen.getByRole("button", { name: "Check IMAP certificate" }));
    expect(incoming().getByLabelText("Host")).toBeDisabled();
    expect(outgoing().getByLabelText("Security")).toBeDisabled();
    await act(async () => { resolve(probe("incoming-pin")); });
    expect(incoming().getByLabelText("Host")).toBeEnabled();
  });

  it("shows a failed test without reporting a saved account", async () => {
    vi.mocked(mailClient.testAndSaveImapAccount).mockRejectedValue(new Error("Wrong password"));
    const onConnected = vi.fn();
    render(<ImapAccountSetup onConnected={onConnected} />);
    fillManualSettings();
    await save();
    expect(screen.getByRole("alert")).toHaveTextContent("Wrong password");
    expect(onConnected).not.toHaveBeenCalled();
  });
});
