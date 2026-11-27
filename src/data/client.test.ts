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

  it("passes the reviewed reply profile to the native command", async () => {
    const context = { subject: "Hello", messages: [] };
    const profile = { aboutYou: "Designer", reviewInstructions: "Be concise" };
    await client.generateReply(context, "Decline", "openai", "model", null, "default", profile);
    expect(sent("ai_generate_reply")).toEqual({ control: { requestId: expect.any(String), price: null }, context, instruction: "Decline", provider: "openai", model: "model", endpoint: null, reasoning: "default", profile });
  });

  it("scopes a mailbox refresh to its account while keeping global refresh available", async () => {
    await client.sync({ accountId: "work@example.com" });
    expect(invoke).toHaveBeenLastCalledWith("sync_account", { accountId: "work@example.com", catchUp: false });
    await client.sync();
    expect(invoke).toHaveBeenLastCalledWith("sync_account", { accountId: undefined, catchUp: false });
  });

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
    expect(sent("ai_summarize_thread")).toEqual({ control: { requestId: expect.any(String), price: null }, threadId: "thread-1", provider: "fireworks", model: "fast", endpoint: null, reasoning: "off" });

    const context = { subject: "Hello", messages: [] };
    await client.generateReply(context, "", "fireworks", "fast", null, "off");
    expect(sent("ai_generate_reply")).toEqual({ control: { requestId: expect.any(String), price: null }, context, instruction: "", provider: "fireworks", model: "fast", endpoint: null, reasoning: "off" });

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

  it("reads and saves device-local recording settings through their own commands", async () => {
    await client.recordingSettings();
    expect(invoke).toHaveBeenLastCalledWith("recording_settings", undefined);
    const settings = { recordMeetings: true, transcriptionModel: "accurate" as const, transcribeDuringMeetings: false, keepAudio: "days30" as const, offerToRecord: true, remindToTell: false, summarizeWithAi: false, includeMyNotes: false, attachTranscript: false, consentAcknowledged: true };
    await client.setRecordingSettings(settings);
    expect(invoke).toHaveBeenLastCalledWith("set_recording_settings", { settings });
  });

  it("calls each meeting recording command with the native contract's names and arguments", async () => {
    const eventRef = { version: 1 as const, kind: "event" as const, id: "evt", account: "me@example.com", calendar: "work" };
    const calls: [() => Promise<unknown>, string, Record<string, unknown> | undefined][] = [
      [() => client.recordingCapabilities(), "recording_capabilities", undefined],
      [() => client.recordingStatus(), "recording_status", undefined],
      [() => client.startRecording({ eventRef, title: "Weekly Sync" }), "recording_start", { eventRef, title: "Weekly Sync" }],
      [() => client.startRecording({ eventRef: null, title: "Meeting at 3:04 PM" }), "recording_start", { eventRef: null, title: "Meeting at 3:04 PM" }],
      [() => client.pauseRecording(), "recording_pause", undefined],
      [() => client.resumeRecording(), "recording_resume", undefined],
      [() => client.stopRecording(), "recording_stop", undefined],
      [() => client.listRecordings(eventRef), "list_recordings", { eventRef }],
      [() => client.deleteRecording("r1"), "delete_recording", { id: "r1" }],
      [() => client.deleteRecordingAudio("r1"), "delete_recording_audio", { id: "r1" }],
      [() => client.recordingsUsage(), "recordings_usage", undefined],
      [() => client.deleteAllRecordingAudio(), "delete_all_recording_audio", undefined],
      [() => client.deleteAllRecordings(), "delete_all_recordings", undefined],
      [() => client.requestRecordingPermissions(), "request_recording_permissions", undefined],
      [() => client.confirmRecordingQuit(), "recording_confirm_quit", undefined],
    ];
    for (const [call, command, args] of calls) {
      await call();
      expect(invoke).toHaveBeenLastCalledWith(command, args);
    }
  });

  it("reads meeting preparation context with an optional mail scope", async () => {
    const reference = { version: 1 as const, kind: "event" as const, id: "evt", account: "me@example.com", calendar: "work" };
    await client.meetingContext(reference);
    expect(invoke).toHaveBeenLastCalledWith("meeting_context", { reference, mailScope: null });
    await client.meetingContext(reference, ["other@example.com"]);
    expect(invoke).toHaveBeenLastCalledWith("meeting_context", { reference, mailScope: ["other@example.com"] });
    await client.meetingReadiness([reference]);
    expect(invoke).toHaveBeenLastCalledWith("meeting_readiness", { references: [reference], mailScope: null });
    const entry = { event: reference, time: "3:00pm", attendees: [], name: "Sync", linkNote: false };
    await client.reserveMeetingPrep("2026-11-16", "v1", entry);
    expect(invoke).toHaveBeenLastCalledWith("reserve_meeting_prep", { date: "2026-11-16", expected: "v1", entry });
    const accepted = { date: "2026-11-16", meeting: "Sync", items: [{ kind: "task" as const, text: "Send" }] };
    await client.saveMeetingFollowThrough(reference, { noteId: "n" }, "v1", "op", accepted);
    expect(invoke).toHaveBeenLastCalledWith("save_meeting_follow_through", { reference, destination: { noteId: "n" }, expectedRevision: "v1", operationId: "op", accepted });
    await client.undoMeetingFollowThrough("op");
    expect(invoke).toHaveBeenLastCalledWith("undo_meeting_follow_through", { operationId: "op" });
    const handoff = { handoffId: "h", account: "me@example.com", mode: "new" as const, sourceId: null, to: "a@example.org", cc: "", subject: "S", body: "B" };
    await client.draftHandoff(handoff);
    expect(invoke).toHaveBeenLastCalledWith("correspondence_request", { request: { op: "handoff", ...handoff } });
    await client.draftHandoffStatus("h");
    expect(invoke).toHaveBeenLastCalledWith("correspondence_request", { request: { op: "handoffStatus", handoffId: "h" } });
    await client.dailyPlan("2026-11-16");
    // The plan's day starts at local midnight, so check-ins count from then.
    expect(invoke).toHaveBeenLastCalledWith("daily_plan", { date: "2026-11-16", dayStart: new Date(2026, 10, 16).toISOString() });
    // A suggestion after sending reads the user's local day the same way.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 10, 16, 15, 30));
    try { await client.suggestTaskForOutbox("send-1"); } finally { vi.useRealTimers(); }
    expect(invoke).toHaveBeenLastCalledWith("suggest_task_for_outbox", { outboxId: "send-1", date: "2026-11-16", dayStart: new Date(2026, 10, 16).toISOString() });
    await client.linkTaskToOutbox("send-1", "t1");
    expect(invoke).toHaveBeenLastCalledWith("link_task_to_outbox", { outboxId: "send-1", taskId: "t1" });
    const additions = [{ kind: "newTask" as const, title: "Draft memo" }];
    await client.addToDailyPlan("2026-11-16", null, additions);
    expect(invoke).toHaveBeenLastCalledWith("add_to_daily_plan", { date: "2026-11-16", expected: null, additions });
    await client.removeFromDailyPlan("2026-11-16", "v2", "key");
    expect(invoke).toHaveBeenLastCalledWith("remove_from_daily_plan", { date: "2026-11-16", expected: "v2", key: "key" });
    const changes = { items: [{ kind: "add" as const, addition: additions[0], carried: "2026-11-13" }], remove: ["task:t1"], notToday: [] };
    await client.saveDailyPlan("2026-11-16", "v3", "op-1", changes);
    expect(invoke).toHaveBeenLastCalledWith("save_daily_plan", { date: "2026-11-16", expected: "v3", operationId: "op-1", changes });
    await client.undoDailyPlanSave("2026-11-16", "op-1");
    expect(invoke).toHaveBeenLastCalledWith("undo_daily_plan_save", { date: "2026-11-16", operationId: "op-1" });
    await client.planSources("2026-11-16", "starredMail");
    expect(invoke).toHaveBeenLastCalledWith("plan_sources", { date: "2026-11-16", source: "starredMail", accountId: null, cursor: null });
    await client.planSources("2026-11-16", "tasks", "me@example.com", "50");
    expect(invoke).toHaveBeenLastCalledWith("plan_sources", { date: "2026-11-16", source: "tasks", accountId: "me@example.com", cursor: "50" });
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


describe("native inbox-assistants contract", () => {
  it("routes inbox assistant reads, configuration, runs, and proposal resolutions to registered native commands", async () => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue({});
    const config = {
      agentId: "stale_reads" as const,
      enabled: true,
      ageDays: 7,
      maxThreadsPerRun: 15,
      includeStarred: false,
      accountId: null,
      scheduleHours: 24,
    };
    const aiConfig = { provider: "openai" as const, model: "gpt-4o", fastModel: "gpt-4o-mini", endpoint: null, reasoning: "default" as const };
    await client.getInboxAgentsSnapshot("you@example.com");
    await client.setInboxAgentRunnerEnabled(false);
    await client.updateInboxAgentConfig(config);
    await client.runInboxAgent("stale_reads", aiConfig, "you@example.com");
    await client.runEnabledInboxAgents(aiConfig, "you@example.com");
    await client.resolveInboxAgentProposal("prop-1", "accepted");
    await client.syncInboxAgentAiConfig(aiConfig);
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["get_inbox_agents_snapshot", { accountId: "you@example.com" }],
      ["set_inbox_agent_runner_enabled", { enabled: false }],
      ["update_inbox_agent_config", { config }],
      ["run_inbox_agent", { agentId: "stale_reads", aiConfig, accountId: "you@example.com" }],
      ["run_enabled_inbox_agents", { aiConfig, accountId: "you@example.com" }],
      ["resolve_inbox_agent_proposal", { id: "prop-1", stateValue: "accepted" }],
      ["sync_inbox_agent_ai_config", { aiConfig }],
    ]);
  });
});
