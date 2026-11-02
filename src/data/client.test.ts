import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import type { MailClient } from "./client";

let client: MailClient;

beforeAll(async () => {
  // The native client is chosen when the module loads.
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  client = (await import("./client")).mailClient;
});

describe("native AI request payloads", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue({});
  });

  const sent = (command: string) => vi.mocked(invoke).mock.calls.find(([name]) => name === command)?.[1];

  it("routes diagnostic reads and local diagnostic controls to the native commands", async () => {
    await client.aiDiagnostics();
    await client.setAiDiagnosticCapture(true);
    await client.setAiDiagnosticCapture(false);
    await client.clearAiDiagnostics();
    await client.openLogsFolder();
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["ai_diagnostics", undefined],
      ["set_ai_diagnostic_capture", { enabled: true }],
      ["set_ai_diagnostic_capture", { enabled: false }],
      ["clear_ai_diagnostics", undefined],
      ["open_logs_folder", undefined],
    ]);
  });

  it("forwards the reasoning switch for fast-tier work", async () => {
    await client.summarizeThread("thread-1", "fireworks", "fast", null, "off");
    expect(sent("ai_summarize_thread")).toEqual({ threadId: "thread-1", provider: "fireworks", model: "fast", endpoint: null, reasoning: "off" });

    const context = { subject: "Hello", messages: [] };
    await client.generateReply(context, "", "fireworks", "fast", null, "off");
    expect(sent("ai_generate_reply")).toEqual({ context, instruction: "", provider: "fireworks", model: "fast", endpoint: null, reasoning: "off" });

    await client.enrichContact("contact-1", "fireworks", "fast", null, ["role"], false, undefined, "off");
    expect(sent("ai_enrich_contact")).toMatchObject({ id: "contact-1", model: "fast", emptyFields: ["role"], reasoning: "off" });
  });

  it("asks for the model's default reasoning when the caller does not say", async () => {
    await client.summarizeThread("thread-1", "fireworks", "model", null);
    await client.generateReply({ subject: "Hello", messages: [] }, "", "fireworks", "model", null);
    await client.enrichContact("contact-1", "fireworks", "model", null, ["role"]);
    for (const command of ["ai_summarize_thread", "ai_generate_reply", "ai_enrich_contact"]) {
      expect(sent(command)).toMatchObject({ reasoning: "default" });
    }
  });
});

describe("native account requests", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue({});
  });

  it("passes optional contextual ranking separately from legacy autocomplete and group mutations", async () => {
    await client.listContactSuggestions("me@example.com", "bob", 8);
    expect(invoke).toHaveBeenLastCalledWith("list_contact_suggestions", { accountId: "me@example.com", query: "bob", limit: 8 });
    const context = { seeds: ["alice@example.com"], excluded: ["alice@example.com"] };
    await client.listContactSuggestions("me@example.com", "bob", 8, context);
    expect(invoke).toHaveBeenLastCalledWith("list_contact_suggestions", { accountId: "me@example.com", query: "bob", limit: 8, ...context });
    await client.rankContactGroupCandidates("team", ["alice"], ["new@example.com"]);
    expect(invoke).toHaveBeenLastCalledWith("rank_contact_group_candidates", { groupId: "team", selectedIds: ["alice"], emails: ["new@example.com"] });
  });

  it("checks meeting guests through the event's calendar account", async () => {
    const request = { eventId: "meeting", accountId: "work@example.com", calendarId: "team", attendees: ["guest@example.com"],
      rangeStart: "2099-09-21T00:00:00Z", rangeEnd: "2099-09-28T00:00:00Z",
      preferences: { timeZone: "UTC", workingWindows: [], defaultDurationMinutes: 30, slotIncrementMinutes: 15, driveTimeBufferMinutes: 0 } };
    await client.meetingAvailability(request);
    expect(invoke).toHaveBeenCalledWith("meeting_availability", { request });
  });

  it("reads Gmail signatures and saves the reviewed import through distinct commands", async () => {
    await client.listGmailSignatures("me@example.com");
    const signature = { id: "gmail", name: "Gmail", html: "<p>Alex</p>", text: "Alex" };
    await client.importGmailSignature("me@example.com", signature, true);
    expect(invoke).toHaveBeenCalledWith("list_gmail_signatures", { account: "me@example.com" });
    expect(invoke).toHaveBeenCalledWith("import_gmail_signature", { account: "me@example.com", signature, useAsDefault: true });
  });

  it("probes each mail endpoint through its own certificate command", async () => {
    await client.probeImapCertificate("incoming.example.com", 993, "implicit_tls");
    await client.probeSmtpCertificate("outgoing.example.com", 587, "start_tls");
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["probe_imap_certificate", { host: "incoming.example.com", port: 993, security: "implicit_tls" }],
      ["probe_smtp_certificate", { host: "outgoing.example.com", port: 587, security: "start_tls" }],
    ]);
  });

  it("forwards independent IMAP and SMTP pins on test and save", async () => {
    const request = {
      email: "me@example.com", imapHost: "incoming.example.com", imapPort: 993,
      imapSecurity: "implicit_tls" as const, imapUsername: "incoming", imapPassword: "secret",
      smtpHost: "outgoing.example.com", smtpPort: 587, smtpSecurity: "start_tls" as const,
      smtpUsername: "outgoing", smtpPassword: null, imapPinnedFingerprint: "incoming-pin",
      smtpPinnedFingerprint: "outgoing-pin", labelStorage: "folders" as const, labelContainer: "Labels",
    };
    await client.testAndSaveImapAccount(request);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("test_and_save_imap_account", { request });
  });

  it("names the mail provider a new account signs in through", async () => {
    await client.addAccount("gmail");
    expect(vi.mocked(invoke).mock.calls.find(([name]) => name === "add_account")?.[1]).toEqual({ provider: "gmail" });
  });

  it("activates the saved IMAP account through its setup completion command", async () => {
    await client.finishImapAccountSetup("me@example.com");
    expect(invoke).toHaveBeenCalledWith("finish_imap_account_setup", { email: "me@example.com" });
  });
});
