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

  it("opens native single-note import without accepting a filesystem path", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(null);
    expect(await client.importNoteFile()).toBeNull();
    expect(invoke).toHaveBeenCalledWith("import_note_file", undefined);
  });

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


describe("native delegated-work contract", () => {
  it("routes local reads, explicit controls, and clarification to registered native commands", async () => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue({});
    const request = { accountId: "you@example.com", objective: "Fixture", workflow: "fixture_research_v1" as const };
    await client.agentRunnerEnabled();
    await client.setAgentRunnerEnabled(true);
    await client.createAgentTask(request);
    await client.getAgentTask("agent-1");
    await client.listAgentTasks("you@example.com");
    await client.controlAgentTask("agent-1", "pause");
    await client.answerAgentQuestion("agent-1", "question-1", "Sales");
    await client.deleteAgentTask("agent-1");
    await client.interactAgentTask("agent-1", "Focus on sales", "revise");
    await client.linkAgentTask("agent-1", null);
    await client.markAgentTaskRead("agent-1", 2, 4, 1);
    const config = { enabled:true, searchEnabled:true, verified:false };
    const research = { task: {...request, workflow:"web_research_v1" as const}, subjects:["Example Show"], consent:true };
    await client.agentResearchSettings();
    await client.setAgentResearchSettings(config);
    await client.setAgentResearchKey("search", "test-secret");
    await client.checkAgentResearchConnection();
    await client.createAgentResearchTask(research);
    await client.openAgentResearchSource("agent-1", "source-1");
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["agent_runner_enabled", undefined],
      ["set_agent_runner_enabled", { enabled: true }],
      ["create_agent_task", { request }],
      ["get_agent_task", { id: "agent-1" }],
      ["list_agent_tasks", { accountId: "you@example.com", includeArchived: false }],
      ["control_agent_task", { id: "agent-1", action: "pause" }],
      ["answer_agent_question", { id: "agent-1", questionId: "question-1", answer: "Sales" }],
      ["delete_agent_task", { id: "agent-1" }],
      ["interact_agent_task", { id: "agent-1", message: "Focus on sales", mode: "revise" }],
      ["link_agent_task", { id: "agent-1", linkedTaskId: null }],
      ["mark_agent_task_read", { id: "agent-1", generation: 2, turnCount: 4, artifactCount: 1 }],
      ["agent_research_settings", undefined],
      ["set_agent_research_settings", { config }],
      ["set_agent_research_key", { provider:"search", key:"test-secret" }],
      ["check_agent_research_connection", undefined],
      ["create_agent_research_task", { request:research }],
      ["open_agent_research_source", { id:"agent-1", sourceId:"source-1" }],
    ]);
  });
});
