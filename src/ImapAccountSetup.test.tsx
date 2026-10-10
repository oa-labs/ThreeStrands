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
const incoming = () => within(screen.getByRole("group", { name: "Incoming Mail (IMAP)" }));
const outgoing = () => within(screen.getByRole("group", { name: "Outgoing Mail (SMTP)" }));
function fillManualSettings() {
  fireEvent.change(screen.getByLabelText("Email Address"), { target: { value: "me@example.com" } });
  fireEvent.change(incoming().getByLabelText("Host"), { target: { value: "incoming.example.com" } });
  fireEvent.change(outgoing().getByLabelText("Host"), { target: { value: "outgoing.example.com" } });
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "secret" } });
}
async function check(kind: "IMAP" | "SMTP") {
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: `Check ${kind} Certificate` })); });
}
function trust(kind: "IMAP" | "SMTP") {
  fireEvent.click(screen.getByRole("button", { name: `Trust This ${kind} Certificate for This Server` }));
}
async function save() {
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Test and Save" })); });
}
async function confirmMapping() {
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Confirm Mapping" })); });
}
async function skipMapping() {
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Skip for Now" })); });
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

  it("uses aligned settings fields with separate username help and intentional initial focus", () => {
    const { container } = render(<ImapAccountSetup onConnected={vi.fn()} />);
    expect(screen.getByLabelText("Email Address")).toHaveFocus();
    expect(incoming().getByLabelText("Username")).toHaveAccessibleDescription("Defaults to your email address.");
    expect(outgoing().getByLabelText("Username")).toHaveAccessibleDescription("Defaults to your IMAP username.");
    for (const control of container.querySelectorAll("input, select")) {
      expect(control.closest("label")).toHaveClass("settings-field", "settings-field-row");
    }
    expect(mailClient.testAndSaveImapAccount).not.toHaveBeenCalled();
  });

  it("opens manual IMAP setup from accounts settings and refreshes saved accounts while preserving Gmail sign-in", async () => {
    const onAdd = vi.fn().mockResolvedValue(undefined);
    const refresh = vi.fn().mockResolvedValue(undefined);
    render(<AccountsSettings authStatus={null} accounts={[]} onAdd={onAdd} onImapConnected={refresh}
      onRemove={vi.fn()} onRemoveEverywhere={vi.fn()} onReconnect={vi.fn()}
      onSetDisplayName={vi.fn()} onSetColor={vi.fn()} onReorder={vi.fn()} />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Add Account" })); });
    expect(onAdd).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Gmail" })); });
    expect(onAdd).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Add Account" }));
    fireEvent.click(screen.getByRole("button", { name: "IMAP" }));
    expect(screen.getByRole("heading", { name: "Add an IMAP Account" })).toBeInTheDocument();
    fillManualSettings();
    await save();
    expect(mailClient.discoverImapSettings).not.toHaveBeenCalled();
    // New contract (Slice 3): a successful test-and-save does NOT finish setup
    // immediately — it discovers mailboxes and presents the mapping review.
    // The saved-account refresh fires only once the user confirms or skips.
    expect(mailClient.discoverImapMailboxes).toHaveBeenCalledWith("me@example.com");
    expect(screen.getByRole("heading", { name: "Map Your Mailboxes" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Map Your Mailboxes" })).toHaveFocus();
    expect(refresh).not.toHaveBeenCalled();
    await skipMapping();
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("heading", { name: "Add an IMAP Account" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Map Your Mailboxes" })).not.toBeInTheDocument();
    expect(onAdd).toHaveBeenCalledTimes(1);
  });

  it("requires separate trust decisions and sends distinct pins for IMAP and SMTP", async () => {
    const onConnected = vi.fn();
    render(<ImapAccountSetup onConnected={onConnected} />);
    fillManualSettings();
    await check("IMAP");
    expect(screen.getByRole("button", { name: "Test and Save" })).toBeDisabled();
    trust("IMAP");
    await check("SMTP");
    expect(mailClient.probeImapCertificate).toHaveBeenCalledWith("incoming.example.com", 143, "start_tls");
    expect(mailClient.probeSmtpCertificate).toHaveBeenCalledWith("outgoing.example.com", 587, "start_tls");
    expect(screen.getByRole("button", { name: "Test and Save" })).toBeDisabled();
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

  it("defaults to creating Archive when discovery finds no Archive Mailbox", async () => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await save();
    expect(screen.getByLabelText("Create a New Archive Mailbox")).toBeChecked();
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
    expect(screen.getByLabelText("Create a New Archive Mailbox")).not.toBeChecked();
    expect(screen.getByRole("combobox", { name: "Archive Mailbox" })).toHaveValue("Storage");
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
    fireEvent.click(screen.getByLabelText("Create a New Archive Mailbox"));
    expect(screen.getByRole("button", { name: "Confirm Mapping" })).toBeDisabled();
    fireEvent.change(screen.getByRole("combobox", { name: "Archive Mailbox" }), { target: { value: "Storage" } });
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
    const container = screen.getByRole("combobox", { name: "Label Container Mailbox" });
    expect(within(container).getByRole("option", { name: "Labels" })).toBeInTheDocument();
    expect(within(container).getByRole("option", { name: "Work" })).toBeInTheDocument();
    expect(within(screen.getByRole("combobox", { name: "Sent Mailbox" }))
      .queryByRole("option", { name: "Labels" })).not.toBeInTheDocument();
    fireEvent.change(container, { target: { value: "Labels" } });
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith(expect.objectContaining({ labelContainer: "Labels" }));
  });

  it.each(["keywords", "none"])("hides label-container selection in %s mode", async (mode) => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    fireEvent.change(screen.getByLabelText("Label Storage"), { target: { value: mode } });
    await save();
    expect(screen.queryByRole("combobox", { name: "Label Container Mailbox" })).not.toBeInTheDocument();
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
    fireEvent.change(screen.getByLabelText("Container Mailbox"), { target: { value: " Labels " } });
    await save();
    expect(mailClient.testAndSaveImapAccount).toHaveBeenCalledWith(expect.objectContaining({ labelContainer: " Labels " }));
    fireEvent.change(screen.getByRole("combobox", { name: "Junk / Spam Mailbox" }), { target: { value: " Junk" } });
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith({
      email: account.email, archive: " Archive ", createArchive: null,
      mailboxOverrides: { sent: "Sent ", junk: " Junk" }, labelContainer: " Labels ",
    });
  });

  it("preserves a manually entered container and lets the user clear it after discovery", async () => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    fireEvent.change(screen.getByLabelText("Container Mailbox"), { target: { value: "Custom" } });
    await save();
    const container = screen.getByRole("combobox", { name: "Label Container Mailbox" });
    expect(container).toHaveValue("Custom");
    fireEvent.change(container, { target: { value: "" } });
    await confirmMapping();
    expect(mailClient.commitImapMailboxMapping).toHaveBeenCalledWith(expect.objectContaining({ labelContainer: null }));
  });

  it("preserves the exact name requested for Archive creation", async () => {
    render(<ImapAccountSetup onConnected={vi.fn()} />);
    fillManualSettings();
    await save();
    fireEvent.change(screen.getByLabelText("New Archive Mailbox Name"), { target: { value: " Archive " } });
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
    expect(screen.getByRole("button", { name: "Test and Save" })).toBeDisabled();
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
    expect(screen.queryByRole("button", { name: /Trust This SMTP/ })).not.toBeInTheDocument();
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
    expect(screen.queryByRole("heading", { name: `Review the ${kind} Server's Certificate` })).not.toBeInTheDocument();
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
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Find Settings Automatically" })); });
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
    fireEvent.click(screen.getByRole("button", { name: "Check IMAP Certificate" }));
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
    expect(screen.getByLabelText("Email Address")).toHaveValue("me@example.com");
    expect(incoming().getByLabelText("Host")).toHaveValue("incoming.example.com");
    expect(outgoing().getByLabelText("Host")).toHaveValue("outgoing.example.com");
    expect(screen.getByLabelText("Password")).toHaveValue("secret");
    expect(onConnected).not.toHaveBeenCalled();
  });
});
