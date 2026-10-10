import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ImapAccountSetup } from "./ImapAccountSetup";
import { AccountsSettings } from "./AccountsSettings";
import { mailClient } from "./data/client";
import type { Account, ImapCertificateProbe } from "./domain";

vi.mock("./data/client", () => ({ mailClient: {
  discoverImapSettings: vi.fn(), probeImapCertificate: vi.fn(),
  probeSmtpCertificate: vi.fn(), testAndSaveImapAccount: vi.fn(),
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

describe("IMAP account setup", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(mailClient.discoverImapSettings).mockResolvedValue(null);
    vi.mocked(mailClient.probeImapCertificate).mockResolvedValue(probe("incoming-pin"));
    vi.mocked(mailClient.probeSmtpCertificate).mockResolvedValue(probe("outgoing-pin"));
    vi.mocked(mailClient.testAndSaveImapAccount).mockResolvedValue(account);
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
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("heading", { name: "Add an IMAP account" })).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Account saved. IMAP mail sync is not available yet.");
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
