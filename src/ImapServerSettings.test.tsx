import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ImapServerSettings } from "./ImapServerSettings";
import { AccountsSettings } from "./AccountsSettings";
import { mailClient } from "./data/client";
import type { Account, ImapConnectionSettings } from "./domain";

const settings: ImapConnectionSettings = {
  imapHost: "127.0.0.1", imapPort: 1143, imapSecurity: "start_tls", imapUsername: "me@proton.me",
  smtpHost: "127.0.0.1", smtpPort: 1025, smtpSecurity: "start_tls", smtpUsername: "me@proton.me",
  imapCertificatePinned: true, smtpCertificatePinned: true,
};

const account: Account = {
  email: "me@proton.me",
  displayName: null,
  color: "#112233",
  status: "connected",
  provider: "imap",
  labelModel: "imapLabelFolders",
  sortOrder: 0,
  connectedAt: "2026-10-08T12:00:00Z",
  lastSyncedAt: null,
  onThisDevice: true,
};

beforeEach(() => {
  vi.spyOn(mailClient, "getImapConnectionSettings").mockResolvedValue(settings);
  vi.spyOn(mailClient, "checkImapCertificateChanges").mockResolvedValue({ imap: null, smtp: null });
});

const rotated = {
  certificate: { subject: "CN=127.0.0.1", issuer: "CN=127.0.0.1", sha256Fingerprint: "CC:DD" },
  trustedByPlatform: false,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function openEditor() {
  const edit = await screen.findByRole("button", { name: "Edit server settings…" });
  await act(async () => { fireEvent.click(edit); });
  return screen.getByRole("button", { name: "Test and Save" }).closest("form")!;
}

describe("ImapServerSettings", () => {
  it("summarizes both servers without showing a password", async () => {
    render(<ImapServerSettings account="me@proton.me" needsReconnect={false} />);
    const summary = await screen.findByText("Incoming (IMAP)");
    const list = summary.closest("dl")!;
    expect(list).toHaveTextContent("127.0.0.1:1143 · STARTTLS · me@proton.me");
    expect(list).toHaveTextContent("127.0.0.1:1025 · STARTTLS · me@proton.me");
    expect(screen.queryByLabelText("Password")).not.toBeInTheDocument();
  });

  it("saves edited servers, keeps the stored password when left blank, and keeps existing pins", async () => {
    const connected = { ...account };
    const update = vi.spyOn(mailClient, "updateImapConnection").mockResolvedValue(connected);
    const onConnected = vi.fn();
    render(<ImapServerSettings account="me@proton.me" needsReconnect={false} onConnected={onConnected} />);
    const form = await openEditor();
    expect(within(form).getByLabelText("Password")).toHaveAccessibleDescription("Leave blank to keep your current password.");
    const outgoing = within(form).getByRole("group", { name: "Outgoing Mail (SMTP)" });
    expect(outgoing).toHaveTextContent("Using the certificate you trusted for this server.");
    fireEvent.change(within(outgoing).getByLabelText("Port"), { target: { value: "1026" } });
    await act(async () => { fireEvent.submit(form); });
    await waitFor(() => expect(update).toHaveBeenCalledWith({
      email: "me@proton.me",
      imapHost: "127.0.0.1", imapPort: 1143, imapSecurity: "start_tls", imapUsername: "me@proton.me",
      smtpHost: "127.0.0.1", smtpPort: 1026, smtpSecurity: "start_tls", smtpUsername: "me@proton.me",
      password: null,
      imapPinnedFingerprint: null,
      smtpPinnedFingerprint: null,
    }));
    expect(onConnected).toHaveBeenCalledWith(connected);
    expect(await screen.findByText("Server settings saved. Syncing mail…")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Test and Save" })).not.toBeInTheDocument();
  });

  it("keeps the editor and the edits when the connection test fails", async () => {
    vi.spyOn(mailClient, "updateImapConnection").mockRejectedValue(new Error("The server rejected the username or password."));
    render(<ImapServerSettings account="me@proton.me" needsReconnect={false} />);
    const form = await openEditor();
    fireEvent.change(within(form).getByLabelText("Password"), { target: { value: "wrong" } });
    await act(async () => { fireEvent.submit(form); });
    expect(await screen.findByRole("alert")).toHaveTextContent("rejected the username or password");
    expect(within(form).getByLabelText("Password")).toHaveValue("wrong");
  });

  it("requires trusting a newly probed self-signed certificate before sending the password", async () => {
    vi.spyOn(mailClient, "probeImapCertificate").mockResolvedValue({
      certificate: { subject: "CN=127.0.0.1", issuer: "CN=127.0.0.1", sha256Fingerprint: "AA:BB" },
      trustedByPlatform: false,
    });
    const update = vi.spyOn(mailClient, "updateImapConnection").mockResolvedValue(account);
    render(<ImapServerSettings account="me@proton.me" needsReconnect={false} />);
    const form = await openEditor();
    await act(async () => { fireEvent.click(within(form).getByRole("button", { name: "Check IMAP Certificate" })); });
    const save = within(form).getByRole("button", { name: "Test and Save" });
    expect(save).toBeDisabled();
    await act(async () => { fireEvent.click(within(form).getByRole("button", { name: /Trust This IMAP Certificate/ })); });
    expect(save).toBeEnabled();
    await act(async () => { fireEvent.submit(form); });
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ imapPinnedFingerprint: "AA:BB", smtpPinnedFingerprint: null })));
  });

  it("shows a changed certificate for review instead of asking for the password", async () => {
    vi.spyOn(mailClient, "checkImapCertificateChanges").mockResolvedValue({ imap: rotated, smtp: null });
    const update = vi.spyOn(mailClient, "updateImapConnection").mockResolvedValue(account);
    render(<ImapServerSettings account="me@proton.me" needsReconnect />);
    expect(await screen.findByText(/incoming \(IMAP\) server's security certificate has changed/)).toBeInTheDocument();
    expect(screen.queryByText(/couldn't sign in to this account/)).not.toBeInTheDocument();
    const form = screen.getByRole("button", { name: "Test and Save" }).closest("form")!;
    const incoming = within(form).getByRole("group", { name: "Incoming Mail (IMAP)" });
    // The old pin is no longer described as in use for the changed server...
    expect(incoming).not.toHaveTextContent("Using the certificate you trusted for this server.");
    // ...while the unchanged SMTP server still is.
    expect(within(form).getByRole("group", { name: "Outgoing Mail (SMTP)" }))
      .toHaveTextContent("Using the certificate you trusted for this server.");
    expect(within(form).getByLabelText("IMAP Certificate")).toHaveTextContent("CC:DD");
    // Nothing is sent until the new certificate is trusted deliberately.
    const save = within(form).getByRole("button", { name: "Test and Save" });
    expect(save).toBeDisabled();
    await act(async () => { fireEvent.click(within(form).getByRole("button", { name: /Trust This IMAP Certificate/ })); });
    expect(save).toBeEnabled();
    await act(async () => { fireEvent.submit(form); });
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ imapPinnedFingerprint: "CC:DD", smtpPinnedFingerprint: null })));
  });

  it("names both servers when both certificates changed", async () => {
    vi.spyOn(mailClient, "checkImapCertificateChanges").mockResolvedValue({ imap: rotated, smtp: rotated });
    render(<ImapServerSettings account="me@proton.me" needsReconnect={false} />);
    await openEditor();
    expect(await screen.findByText(/incoming \(IMAP\) and outgoing \(SMTP\) servers' security certificate/)).toBeInTheDocument();
  });

  it("keeps the password explanation when the certificate check cannot reach the server", async () => {
    vi.spyOn(mailClient, "checkImapCertificateChanges").mockRejectedValue(new Error("unreachable"));
    render(<ImapServerSettings account="me@proton.me" needsReconnect />);
    expect(await screen.findByText(/couldn't sign in to this account/)).toBeInTheDocument();
    expect(screen.queryByText(/security certificate has changed/)).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("opens the editor with an explanation when the account needs reconnecting", async () => {
    render(<ImapServerSettings account="me@proton.me" needsReconnect />);
    expect(await screen.findByText(/couldn't sign in to this account/)).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
  });
});

describe("IMAP reconnect from the account card", () => {
  it("opens the Server editor and focuses the password instead of starting a Google sign-in", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ identities: [{ address: account.email }], serverSavesSentOverride: false, sentCopyDetected: null });
    const onReconnect = vi.fn();
    render(<AccountsSettings
      authStatus={null}
      accounts={[{ ...account, status: "needs_reauth" }]}
      onAdd={vi.fn()} onRemove={vi.fn()} onRemoveEverywhere={vi.fn()} onReconnect={onReconnect}
      onSetDisplayName={vi.fn()} onSetNickname={vi.fn()} onSetColor={vi.fn()} onReorder={vi.fn()}
    />);
    const password = await screen.findByLabelText("Password");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Reconnect" })); });
    await waitFor(() => expect(password).toHaveFocus());
    expect(onReconnect).not.toHaveBeenCalled();
    expect(screen.queryByText("Waiting for Google…")).not.toBeInTheDocument();
  });
});
