import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearScheduleCache } from "./calendarScheduleCache";
import { clearAiApiKey, DEFAULT_AI_FEATURES, saveAiFeatures, saveAiProvider, setAiApiKey, type AiFeatureFlags } from "./aiSettings";
import { proactiveDwellMs } from "./proactiveBrief";
import { App, formatMailTimestamp, messagesWithQueuedReplies } from "./App";
import { mailClient } from "./data/client";
import type { OutboxItem } from "./correspondence";
import type { ContactProfile, ContactTimelineItem } from "./domain";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("keeps one read-only context panel beside the conversation; Shift+A reveals suggestions there and workspace shortcuts still switch views", async () => {
  render(<App />);

  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  const panel = screen.getByRole("complementary", { name: "Conversation context" });
  expect(screen.queryByRole("button", { name: "Close contact pane" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Actions (Shift+A)" })).not.toBeInTheDocument();

  fireEvent.keyDown(window, { key: "A", shiftKey: true });
  expect(screen.getByRole("complementary", { name: "Conversation context" })).toBe(panel);
  expect(screen.queryByRole("complementary", { name: "Actions" })).not.toBeInTheDocument();
  expect(within(panel).getByRole("region", { name: "Brief" })).toHaveTextContent("AI briefs and suggestions are off.");
  expect(within(panel).queryByRole("textbox")).not.toBeInTheDocument();
  expect(within(panel).queryByRole("combobox")).not.toBeInTheDocument();

  fireEvent.keyDown(window, { key: "3" });
  expect(screen.getByRole("region", { name: "Tasks" })).toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Inbox" })).not.toBeInTheDocument();
  expect(screen.queryByRole("region", { name: "Conversation" })).not.toBeInTheDocument();
  expect(screen.queryByRole("complementary", { name: "Conversation context" })).not.toBeInTheDocument();
});

it("shows a conversation participant on the next meeting and opens its details in the calendar", async () => {
  clearScheduleCache();
  vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
    { email: "calendar@example.com", connectedAt: "2026-09-18T00:00:00Z", status: "connected" },
  ]);
  const start = new Date(Date.now() + 26 * 60 * 60 * 1000);
  vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
    events: [
      { id: "kickoff", accountId: "calendar@example.com", title: "Onboarding kickoff", allDay: false,
        start: start.toISOString(), end: new Date(start.getTime() + 30 * 60 * 1000).toISOString(), attendees: ["hello@threestrands.local"] },
      { id: "other", accountId: "calendar@example.com", title: "Unrelated sync", allDay: false,
        start: start.toISOString(), end: new Date(start.getTime() + 30 * 60 * 1000).toISOString(), attendees: ["someone@example.com"] },
    ],
    errors: [],
  });
  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  const panel = screen.getByRole("complementary", { name: "Conversation context" });

  const meetings = await within(panel).findByRole("region", { name: "Upcoming meetings" });
  expect(within(meetings).queryByText("Unrelated sync")).not.toBeInTheDocument();
  expect(within(meetings).getByRole("button", { name: /Onboarding kickoff/ })).toHaveTextContent("with ThreeStrands");
  fireEvent.click(within(meetings).getByRole("button", { name: /Onboarding kickoff/ }));

  expect(await screen.findByRole("region", { name: "Calendar week" })).toBeInTheDocument();
  expect(await screen.findByRole("dialog", { name: "Onboarding kickoff details" })).toBeInTheDocument();
  expect(screen.queryByRole("complementary", { name: "Conversation context" })).not.toBeInTheDocument();
});

describe("conversation brief", () => {
  const briefResult = {
    summary: { summary: "- Welcome to the app.\n- Nothing is due.", generatedAt: "2099-01-01T00:00:00Z" },
    analysis: { proposals: [], hiddenCount: 0 },
  };

  async function enableAi(features: Partial<AiFeatureFlags>) {
    saveAiProvider("openai");
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, ...features });
    await setAiApiKey("test-key");
  }

  afterEach(async () => {
    saveAiProvider("none");
    saveAiFeatures(DEFAULT_AI_FEATURES);
    await clearAiApiKey();
  });

  it("gets the summary and suggestions in one request, then only calls again on refresh", async () => {
    await enableAi({ summarize: true, actionExtraction: true });
    const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
    const summarizeThread = vi.spyOn(mailClient, "summarizeThread");
    const analyzeThread = vi.spyOn(mailClient, "analyzeThread");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const panel = screen.getByRole("complementary", { name: "Conversation context" });

    fireEvent.click(await within(panel).findByRole("button", { name: "Get Brief" }));

    expect(await within(panel).findByText("Welcome to the app.")).toBeInTheDocument();
    expect(within(panel).getByText("Nothing to schedule or follow up on.")).toBeInTheDocument();
    expect(briefThread).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(window, { key: "A", shiftKey: true });
    fireEvent.keyDown(window, { key: "i" });
    await waitFor(() => expect(within(panel).queryByRole("status")).not.toBeInTheDocument());
    expect(briefThread).toHaveBeenCalledTimes(1);
    expect(summarizeThread).not.toHaveBeenCalled();
    expect(analyzeThread).not.toHaveBeenCalled();

    fireEvent.click(within(panel).getByRole("button", { name: "Refresh brief" }));
    await waitFor(() => expect(briefThread).toHaveBeenCalledTimes(2));
  });

  it("fetches the whole brief in one request when Shift+A finds nothing yet", async () => {
    await enableAi({ summarize: true, actionExtraction: true });
    const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
    const analyzeThread = vi.spyOn(mailClient, "analyzeThread");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const panel = screen.getByRole("complementary", { name: "Conversation context" });
    await within(panel).findByRole("button", { name: "Get Brief" });

    fireEvent.keyDown(window, { key: "A", shiftKey: true });

    await waitFor(() => expect(briefThread).toHaveBeenCalledTimes(1));
    expect(analyzeThread).not.toHaveBeenCalled();
    expect(await within(panel).findByText("Welcome to the app.")).toBeInTheDocument();
  });

  it("uses the single-purpose request when only one AI feature is on", async () => {
    await enableAi({ summarize: false, actionExtraction: true });
    const briefThread = vi.spyOn(mailClient, "briefThread");
    const analyzeThread = vi.spyOn(mailClient, "analyzeThread").mockResolvedValue({ proposals: [], hiddenCount: 1 });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const panel = screen.getByRole("complementary", { name: "Conversation context" });

    fireEvent.click(await within(panel).findByRole("button", { name: "Get Suggestions" }));

    expect(await within(panel).findByText("1 suggestion couldn’t be matched to the email, so it was hidden.")).toBeInTheDocument();
    expect(analyzeThread).toHaveBeenCalledTimes(1);
    expect(briefThread).not.toHaveBeenCalled();
  });

  describe("scheduling meetings from the panel", () => {
    const hour = 3_600_000;
    // Whole minutes, as the event dialog's date-time fields hold them.
    const minute = 60_000;
    const inHours = (hours: number) => new Date(Math.floor((Date.now() + hours * hour) / minute) * minute).toISOString();
    const meetingAt = (hoursFromNow: number) => ({
      type: "meeting" as const, intent: "schedule", title: "Budget review", participants: ["jane@example.com"], location: null,
      rawTimeLanguage: "Thursday at 3", normalizedStart: inHours(hoursFromNow),
      normalizedEnd: inHours(hoursFromNow + 0.5), searchRangeStart: null, searchRangeEnd: null,
      durationMinutes: 30, timeZone: "America/New_York", confidence: 0.9,
      evidence: { sourceMessageId: "welcome-message", excerpt: "command palette" },
    });

    async function openWithMeeting(meeting: ReturnType<typeof meetingAt>) {
      clearScheduleCache();
      await enableAi({ actionExtraction: true, threadChat: true });
      vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([{ email: "calendar@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" }]);
      vi.spyOn(mailClient, "listCalendarOptions").mockResolvedValue([{ id: "primary", accountId: "calendar@example.com", name: "Work", primary: true, selected: true, writable: true }]);
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
      vi.spyOn(mailClient, "analyzeThread").mockResolvedValue({ proposals: [meeting], hiddenCount: 0 });
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const panel = screen.getByRole("complementary", { name: "Conversation context" });
      fireEvent.click(await within(panel).findByRole("button", { name: "Get Suggestions" }));
      const schedule = await within(panel).findByRole("group", { name: "Schedule" });
      return { panel, schedule };
    }

    it("checks the proposed time, then adds it to the calendar through the prefilled event dialog", async () => {
      const meeting = meetingAt(26);
      const check = vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue({ id: "created", accountId: "calendar@example.com", title: "Budget review", start: meeting.normalizedStart, end: meeting.normalizedEnd, allDay: false });
      const { panel, schedule } = await openWithMeeting(meeting);

      expect(await within(schedule).findByText("You’re free")).toBeInTheDocument();
      expect(check).toHaveBeenCalledWith(expect.objectContaining({ start: meeting.normalizedStart, end: meeting.normalizedEnd }));
      fireEvent.click(within(schedule).getByRole("button", { name: "Add to Calendar" }));

      const dialog = await screen.findByRole("dialog", { name: "New event" });
      expect(within(dialog).getByLabelText("Title")).toHaveValue("Budget review");
      expect(within(dialog).getByLabelText("Invite people")).toHaveValue("jane@example.com");
      expect(within(dialog).getByLabelText("Description")).toHaveValue("Scheduled from “Welcome to ThreeStrands”.\n\n“command palette”");
      expect(create).not.toHaveBeenCalled();
      await waitFor(() => expect(within(dialog).getByLabelText("Calendar")).toHaveValue("calendar@example.com\nprimary"));
      fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));

      await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
        accountId: "calendar@example.com", calendarId: "primary", title: "Budget review", attendees: ["jane@example.com"],
        start: new Date(meeting.normalizedStart).toISOString(),
      })));
      expect(await screen.findByText("Added to calendar")).toBeInTheDocument();
      await waitFor(() => expect(within(panel).queryByText("Budget review")).not.toBeInTheDocument());
    });

    it("replies that a free time works, and opens more times on the meeting's day", async () => {
      const meeting = meetingAt(50);
      vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
      const { schedule } = await openWithMeeting(meeting);

      fireEvent.click(await within(schedule).findByRole("button", { name: "More Times" }));
      const sidebar = await screen.findByRole("complementary", { name: "Calendar schedule" });
      expect(within(sidebar).getByRole("heading", { level: 2 })).toHaveTextContent(
        new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" }).format(new Date(meeting.normalizedStart)),
      );

      fireEvent.click(within(schedule).getByRole("button", { name: "Reply “That Works”" }));
      const editor = await screen.findByRole("textbox", { name: "Message Body" });
      await waitFor(() => expect(editor).toHaveTextContent("That time works for me:"));
    });

    it("shows open times from the calendar when a chat answer asks for them", async () => {
      clearScheduleCache();
      await enableAi({ threadChat: true });
      vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([{ email: "calendar@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" }]);
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
      const rangeStart = new Date(Date.now() + hour).toISOString();
      const rangeEnd = new Date(Date.now() + 5 * 24 * hour).toISOString();
      vi.spyOn(mailClient, "threadChat").mockResolvedValue({
        answer: "Here are open times from your calendar.", analysis: { proposals: [], hiddenCount: 0 }, replyDraft: null, sources: [], searched: [],
        attachments: [],
        availability: { rangeStart, rangeEnd, durationMinutes: 45 },
      });
      const slot = { start: new Date(Date.now() + 20 * hour).toISOString(), end: new Date(Date.now() + 20.75 * hour).toISOString(), status: "verified" as const };
      const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [slot], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const panel = screen.getByRole("complementary", { name: "Conversation context" });

      fireEvent.keyDown(window, { key: "q" });
      const input = await within(panel).findByRole("textbox", { name: "Ask about this conversation" });
      fireEvent.change(input, { target: { value: "When am I free next week?" } });
      fireEvent.keyDown(input, { key: "Enter" });

      const times = await within(panel).findByRole("group", { name: "Open times" });
      expect(within(times).getAllByRole("button")).toHaveLength(1);
      expect(find).toHaveBeenCalledWith(expect.objectContaining({ rangeStart, rangeEnd, maxPerDay: 1, preferences: expect.objectContaining({ defaultDurationMinutes: 45 }) }));
    });
  });

  describe("thread chat", () => {
    const chatProposal = {
      type: "task" as const, kind: "action" as const, title: "Try the command palette", notes: null,
      dueKind: "none" as const, dueValue: null, timeZone: null, repeatIntervalDays: null, confidence: 0.9,
      evidence: { sourceMessageId: "welcome-message", excerpt: "command palette" },
    };

    it("opens with q, keeps typed letters out of shortcuts, and returns to read mode on Escape", async () => {
      await enableAi({ threadChat: true });
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const panel = screen.getByRole("complementary", { name: "Conversation context" });
      expect(within(panel).queryByRole("textbox")).not.toBeInTheDocument();

      fireEvent.keyDown(window, { key: "q" });
      const input = await within(panel).findByRole("textbox", { name: "Ask about this conversation" });
      await waitFor(() => expect(input).toHaveFocus());
      fireEvent.keyDown(input, { key: "j" });
      fireEvent.keyDown(input, { key: "e" });
      expect(screen.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();

      fireEvent.keyDown(input, { key: "Escape" });
      expect(within(panel).queryByRole("textbox")).not.toBeInTheDocument();
      fireEvent.keyDown(window, { key: "j" });
      expect(await screen.findByRole("heading", { name: "Phase 1: read and triage" })).toBeInTheDocument();
    });

    it("sends the question with earlier turns, files its tasks under Suggested, and opens a drafted reply for review", async () => {
      await enableAi({ threadChat: true, actionExtraction: true });
      const threadChat = vi.spyOn(mailClient, "threadChat")
        .mockResolvedValueOnce({ answer: "It introduces the shortcuts.", analysis: { proposals: [chatProposal], hiddenCount: 0 }, replyDraft: null, sources: [], searched: [], attachments: [], availability: null })
        .mockResolvedValueOnce({ answer: "Here is a reply.", analysis: { proposals: [], hiddenCount: 0 }, replyDraft: "Thanks for the tour!", sources: [], searched: [], attachments: [], availability: null });
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const panel = screen.getByRole("complementary", { name: "Conversation context" });

      fireEvent.keyDown(window, { key: "q" });
      const input = await within(panel).findByRole("textbox", { name: "Ask about this conversation" });
      fireEvent.change(input, { target: { value: "What is this about?" } });
      fireEvent.click(within(panel).getByRole("checkbox", { name: "Search all mail" }));
      fireEvent.keyDown(input, { key: "Enter" });

      expect(await within(panel).findByText("It introduces the shortcuts.")).toBeInTheDocument();
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({
        threadId: "welcome", question: "What is this about?", history: [], searchMailbox: true, includeProposals: true,
      }), "openai", expect.any(String), null);
      const suggestions = within(panel).getByRole("region", { name: "Suggestions" });
      expect(within(suggestions).getByText("Try the command palette")).toBeInTheDocument();
      expect(within(suggestions).getByRole("button", { name: "Get Suggestions" })).toBeInTheDocument();

      fireEvent.change(input, { target: { value: "Draft a reply" } });
      fireEvent.keyDown(input, { key: "Enter" });
      expect(await within(panel).findByText("Thanks for the tour!")).toBeInTheDocument();
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({
        searchMailbox: false,
        history: [{ role: "user", content: "What is this about?" }, { role: "assistant", content: "It introduces the shortcuts." }],
      }), "openai", expect.any(String), null);

      fireEvent.click(within(panel).getByRole("button", { name: "Use as Reply" }));
      const editor = await screen.findByRole("textbox", { name: "Message Body" });
      await waitFor(() => expect(editor).toHaveTextContent("Thanks for the tour!"));
    });

    it("leaves proposals out of the request when Suggestions is off and offers the question again after a failure", async () => {
      await enableAi({ threadChat: true });
      const threadChat = vi.spyOn(mailClient, "threadChat")
        .mockRejectedValueOnce(new Error("error sending request for url"))
        .mockResolvedValueOnce({ answer: "Answered.", analysis: { proposals: [], hiddenCount: 0 }, replyDraft: null, sources: [], searched: [], attachments: [], availability: null });
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const panel = screen.getByRole("complementary", { name: "Conversation context" });

      fireEvent.keyDown(window, { key: "q" });
      const input = await within(panel).findByRole("textbox", { name: "Ask about this conversation" });
      fireEvent.change(input, { target: { value: "Anything due?" } });
      fireEvent.keyDown(input, { key: "Enter" });

      expect(await within(panel).findByText("Couldn't reach the AI provider. Check your connection and try again.")).toBeInTheDocument();
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({ includeProposals: false }), "openai", expect.any(String), null);
      expect(within(panel).queryByText("Anything due?")).not.toBeInTheDocument();
      fireEvent.click(within(panel).getByRole("button", { name: "Try Again" }));
      expect(await within(panel).findByText("Answered.")).toBeInTheDocument();
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({ question: "Anything due?", history: [] }), "openai", expect.any(String), null);
    });
    it("shares an attachment chosen with @ and keeps it shared for follow-up questions", async () => {
      await enableAi({ threadChat: true });
      const threadChat = vi.spyOn(mailClient, "threadChat").mockResolvedValue({
        answer: "It lists the shortcuts.", analysis: { proposals: [], hiddenCount: 0 }, replyDraft: null, sources: [], searched: [],
        attachments: [{ messageId: "welcome-message", attachmentId: "demo-guide", filename: "threestrands-shortcuts.txt", truncated: false }],
        availability: null,
      });
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const panel = screen.getByRole("complementary", { name: "Conversation context" });

      fireEvent.keyDown(window, { key: "q" });
      const input = await within(panel).findByRole("textbox", { name: "Ask about this conversation" });
      fireEvent.change(input, { target: { value: "Summarize @short" } });
      fireEvent.click(within(within(panel).getByRole("listbox", { name: "Attachments" })).getByRole("option", { name: /threestrands-shortcuts\.txt/ }));
      fireEvent.keyDown(input, { key: "Enter" });

      expect(await within(panel).findByText("It lists the shortcuts.")).toBeInTheDocument();
      expect(within(panel).getByText("Shared threestrands-shortcuts.txt")).toBeInTheDocument();
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({
        question: "Summarize @threestrands-shortcuts.txt",
        attachments: [{ messageId: "welcome-message", attachmentId: "demo-guide" }],
      }), "openai", expect.any(String), null);

      fireEvent.change(input, { target: { value: "Which one archives?" } });
      fireEvent.keyDown(input, { key: "Enter" });
      await waitFor(() => expect(threadChat).toHaveBeenCalledTimes(2));
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({
        question: "Which one archives?",
        attachments: [{ messageId: "welcome-message", attachmentId: "demo-guide" }],
      }), "openai", expect.any(String), null);
    });

    it("shares no attachments unless one is chosen", async () => {
      await enableAi({ threadChat: true });
      const threadChat = vi.spyOn(mailClient, "threadChat").mockResolvedValue({
        answer: "Answered.", analysis: { proposals: [], hiddenCount: 0 }, replyDraft: null, sources: [], searched: [], attachments: [], availability: null,
      });
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const panel = screen.getByRole("complementary", { name: "Conversation context" });
      fireEvent.keyDown(window, { key: "q" });
      const input = await within(panel).findByRole("textbox", { name: "Ask about this conversation" });
      fireEvent.change(input, { target: { value: "What does threestrands-shortcuts.txt say?" } });
      fireEvent.keyDown(input, { key: "Enter" });
      await waitFor(() => expect(threadChat).toHaveBeenCalled());
      expect(threadChat).toHaveBeenLastCalledWith(expect.objectContaining({ attachments: [] }), "openai", expect.any(String), null);
    });
  });

  describe("proactive suggestions", () => {
    const dwell = proactiveDwellMs(2);

    async function openRoadmap() {
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      fireEvent.keyDown(window, { key: "j" });
      await screen.findByRole("heading", { name: "Phase 1: read and triage" });
    }

    afterEach(() => { vi.useRealTimers(); });

    it("prepares the brief once after the reader stays on a qualifying conversation", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      await enableAi({ summarize: true, actionExtraction: true, proactiveBriefs: true });
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

      // The welcome message offers unsubscribe, so it is treated as a mailing list.
      await vi.advanceTimersByTimeAsync(dwell + 1_000);
      expect(briefThread).not.toHaveBeenCalled();

      fireEvent.keyDown(window, { key: "j" });
      await screen.findByRole("heading", { name: "Phase 1: read and triage" });
      await vi.advanceTimersByTimeAsync(dwell - 1_000);
      expect(briefThread).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1_500);
      await waitFor(() => expect(briefThread).toHaveBeenCalledTimes(1));
      expect(briefThread.mock.calls[0][0]).toBe("roadmap");
      expect(await within(screen.getByRole("complementary", { name: "Conversation context" })).findByText("Welcome to the app.")).toBeInTheDocument();

      await vi.advanceTimersByTimeAsync(dwell * 3);
      expect(briefThread).toHaveBeenCalledTimes(1);
    });

    it("does nothing for a conversation left before the dwell elapses", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      await enableAi({ summarize: true, actionExtraction: true, proactiveBriefs: true });
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      await openRoadmap();
      await vi.advanceTimersByTimeAsync(dwell - 1_000);
      fireEvent.keyDown(window, { key: "k" });
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      await vi.advanceTimersByTimeAsync(dwell * 2);
      expect(briefThread).not.toHaveBeenCalled();
    });

    it("waits for a sender the user has emailed when that filter is on", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      await enableAi({ summarize: true, actionExtraction: true, proactiveBriefs: true, proactiveKnownSendersOnly: true });
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      vi.spyOn(mailClient, "listContactProfiles").mockResolvedValue([]);
      await openRoadmap();
      await vi.advanceTimersByTimeAsync(dwell * 2);
      expect(briefThread).not.toHaveBeenCalled();
    });

    it("stays on request only while proactive suggestions are off", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      await enableAi({ summarize: true, actionExtraction: true });
      const briefThread = vi.spyOn(mailClient, "briefThread").mockResolvedValue(briefResult);
      await openRoadmap();
      await vi.advanceTimersByTimeAsync(dwell * 2);
      expect(briefThread).not.toHaveBeenCalled();
    });
  });

  it("keeps a failed brief retryable in the panel", async () => {
    await enableAi({ summarize: true, actionExtraction: true });
    const briefThread = vi.spyOn(mailClient, "briefThread")
      .mockRejectedValueOnce(new Error("The AI provider returned malformed thread brief JSON"))
      .mockResolvedValueOnce(briefResult);
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const panel = screen.getByRole("complementary", { name: "Conversation context" });

    fireEvent.click(await within(panel).findByRole("button", { name: "Get Brief" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent("The AI's response couldn't be read.");
    fireEvent.click(within(panel).getByRole("button", { name: "Try Again" }));

    expect(await within(panel).findByText("Welcome to the app.")).toBeInTheDocument();
    expect(briefThread).toHaveBeenCalledTimes(2);
  });
});

it("opens archived contact timeline email in All Mail and keeps it selected after refresh", async () => {
  localStorage.removeItem("threestrands.settings.selectedMailboxByAccount");
  await mailClient.mutateThread({ kind: "archive", threadId: "roadmap", value: true });
  const contact: ContactProfile = {
    id: "contact:team@example.com", displayName: "Product Team", role: null, company: null,
    location: null, bio: null, notes: null, links: [], photoData: null, favorite: false,
    addresses: ["team@example.com"], sentCount: 1, receivedCount: 1, lastInteractedAt: null,
  };
  const timelineItem: ContactTimelineItem = {
    threadId: "roadmap", accountId: "demo@example.com", contactEmail: "team@example.com", subject: "Phase 1: read and triage",
    snippet: "The first vertical slice includes local search and optimistic actions.",
    sentAt: "2026-03-05T14:15:00Z", labels: [],
  };
  vi.spyOn(mailClient, "listContactProfiles").mockResolvedValue([contact]);
  vi.spyOn(mailClient, "getContactProfile").mockResolvedValue(contact);
  vi.spyOn(mailClient, "contactTimeline").mockResolvedValue([timelineItem]);
  const listAllMail = mailClient.listAllMailPage.bind(mailClient);
  vi.spyOn(mailClient, "listAllMailPage").mockImplementation(async (accountId, offset, limit) => {
    const page = await listAllMail(accountId, offset, limit);
    return { ...page, threads: page.threads.filter((thread) => thread.id !== "roadmap") };
  });

  try {
    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });
    fireEvent.click(screen.getByRole("button", { name: "Contacts" }));
    await screen.findByDisplayValue("Product Team");
    fireEvent.click(screen.getByRole("button", { name: /Phase 1: read and triage/ }));

    expect(await screen.findByRole("listbox", { name: "All Mail" })).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Phase 1: read and triage" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Refresh mail" }));

    expect(await screen.findByRole("heading", { name: "Phase 1: read and triage" })).toBeInTheDocument();
    expect(screen.getByRole("listbox", { name: "All Mail" })).toBeInTheDocument();
  } finally {
    await mailClient.mutateThread({ kind: "archive", threadId: "roadmap", value: false });
    localStorage.removeItem("threestrands.settings.selectedMailboxByAccount");
  }
});

it("formats today's mail with the time of day and older mail with the date", () => {
  const now = new Date(2026, 8, 14, 18, 0);
  const today = new Date(2026, 8, 14, 9, 5);
  const older = new Date(2026, 8, 13, 23, 55);

  expect(formatMailTimestamp(today.toISOString(), now)).toBe(new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
  }).format(today));
  expect(formatMailTimestamp(older.toISOString(), now)).toBe(new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "2-digit",
  }).format(older));
});

it("puts 'and' before the final message recipient", async () => {
  const originalGetThread = mailClient.getThread.bind(mailClient);
  vi.spyOn(mailClient, "getThread").mockImplementation(async (id) => {
    const detail = await originalGetThread(id);
    return id === "welcome" ? {
      ...detail,
      messages: detail.messages.map((message) => ({
        ...message,
        recipients: [
          "Joel Reed <joel@example.com>",
          '"Bates',
          'Daniel R" <daniel@example.com>',
          "Cara Cenfetelli <cara@example.com>",
        ],
      })),
    } : detail;
  });

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

  const recipientLine = document.querySelector(".message-recipients");
  expect(recipientLine?.querySelectorAll(".address-name")).toHaveLength(3);
  expect(Array.from(recipientLine?.querySelectorAll(".address-name") ?? [], ({ textContent }) => textContent))
    .toEqual(["Joel Reed", "Bates, Daniel R", "Cara Cenfetelli"]);
  expect(Array.from(recipientLine?.children ?? []).map((recipient) =>
    Array.from(recipient.childNodes).find((node) => node.nodeType === Node.TEXT_NODE)?.textContent ?? "",
  )).toEqual(["", ", ", ", and "]);
});

it("refreshes the open conversation when its inbox row receives a sent reply", async () => {
  const originalList = mailClient.listThreadsPage.bind(mailClient);
  const originalGetThread = mailClient.getThread.bind(mailClient);
  const originalListAccounts = mailClient.listAccounts.bind(mailClient);
  const syncStatus = await mailClient.syncStatus();
  let replyDelivered = false;

  vi.spyOn(mailClient, "sync").mockImplementation(async () => {
    replyDelivered = true;
    return syncStatus;
  });
  vi.spyOn(mailClient, "listAccounts").mockImplementation(async () =>
    (await originalListAccounts()).map((account) => account.email === "demo@example.com"
      ? { ...account, displayName: "Joel Reed" }
      : account),
  );
  vi.spyOn(mailClient, "listThreadsPage").mockImplementation(async (...args) => {
    const page = await originalList(...args);
    if (!replyDelivered) return page;
    return {
      ...page,
      threads: page.threads.map((thread) => thread.id === "welcome" ? {
        ...thread,
        snippet: "Sent reply body",
        lastMessageAt: "2026-03-05T17:30:00Z",
      } : thread),
    };
  });
  vi.spyOn(mailClient, "getThread").mockImplementation(async (id) => {
    const detail = await originalGetThread(id);
    if (!replyDelivered || id !== "welcome") return detail;
    return {
      ...detail,
      thread: {
        ...detail.thread,
        snippet: "Sent reply body",
        lastMessageAt: "2026-03-05T17:30:00Z",
      },
      messages: [...detail.messages, {
        id: "sent-reply",
        threadId: "welcome",
        sender: "<demo@example.com>",
        recipients: ["hello@threestrands.local"],
        sentAt: "2026-03-05T17:30:00Z",
        bodyHtml: "<p>Sent reply body</p>",
        bodyText: "Sent reply body",
        unread: false,
        unsubscribe: null,
        attachments: [],
      }],
    };
  });

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  fireEvent.click(screen.getByRole("button", { name: "Refresh mail" }));

  await waitFor(() => {
    const bodies = screen.getAllByTestId("message-body") as HTMLIFrameElement[];
    expect(bodies.some((body) => body.srcdoc.includes("Sent reply body"))).toBe(true);
  });
  expect(screen.getByText("Joel Reed", { selector: ".address-name" })).toBeInTheDocument();
});

it("keeps an unsaved reply mounted when a refresh changes its conversation placement", async () => {
  localStorage.removeItem("threestrands.demoCorrespondence");
  const originalList = mailClient.listThreadsPage.bind(mailClient);
  const originalGetThread = mailClient.getThread.bind(mailClient);
  const syncStatus = await mailClient.syncStatus();
  let refreshed = false;

  vi.spyOn(mailClient, "sync").mockImplementation(async () => {
    refreshed = true;
    return syncStatus;
  });
  vi.spyOn(mailClient, "listThreadsPage").mockImplementation(async (...args) => {
    const page = await originalList(...args);
    if (!refreshed) return page;
    return {
      ...page,
      threads: page.threads.map((thread) => thread.id === "welcome" ? {
        ...thread,
        snippet: "Conversation refreshed",
        lastMessageAt: "2026-03-05T18:00:00Z",
      } : thread),
    };
  });
  vi.spyOn(mailClient, "getThread").mockImplementation(async (id) => {
    const detail = await originalGetThread(id);
    if (!refreshed || id !== "welcome") return detail;
    return {
      ...detail,
      thread: {
        ...detail.thread,
        snippet: "Conversation refreshed",
        lastMessageAt: "2026-03-05T18:00:00Z",
      },
      // A cache reconciliation can replace the source-message rows while the
      // open local draft remains valid. That may change where the reply is
      // displayed, but must never recreate its browser-owned editor DOM.
      messages: detail.messages.map((message) => ({
        ...message,
        id: `refreshed-${message.id}`,
      })),
    };
  });

  try {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Reply" }));

    const editor = await screen.findByRole("textbox", { name: "Message Body" });
    editor.insertAdjacentHTML("afterbegin", "<p>Do not lose this long reply.</p>");
    fireEvent.input(editor);

    fireEvent.click(screen.getByRole("button", { name: "Refresh mail" }));

    await waitFor(() => expect(screen.queryByRole("heading", { name: "Welcome to ThreeStrands" })).not.toBeInTheDocument());
    const refreshedEditor = screen.getByRole("textbox", { name: "Message Body" });
    expect(refreshedEditor).toBe(editor);
    expect(refreshedEditor).toHaveTextContent("Do not lose this long reply.");
  } finally {
    localStorage.removeItem("threestrands.demoCorrespondence");
  }
});

it("reloads the local inbox after a refresh even when one account sync fails", async () => {
  const originalList = mailClient.listThreadsPage.bind(mailClient);
  const originalStatus = mailClient.syncStatus.bind(mailClient);
  let listCalls = 0;

  vi.spyOn(mailClient, "listThreadsPage").mockImplementation(async (...args) => {
    listCalls += 1;
    return originalList(...args);
  });
  vi.spyOn(mailClient, "sync").mockRejectedValue(new Error("second@example.com failed"));
  vi.spyOn(mailClient, "syncStatus").mockImplementation(originalStatus);

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  const callsBeforeRefresh = listCalls;

    fireEvent.click(screen.getByRole("button", { name: "Refresh mail" }));

  await waitFor(() => expect(listCalls).toBeGreaterThan(callsBeforeRefresh));
});

it("reloads the inbox after reconnecting an imported account", async () => {
  const originalAccounts = mailClient.listAccounts.bind(mailClient);
  const originalList = mailClient.listThreadsPage.bind(mailClient);
  let needsReconnect = true;
  let listCalls = 0;

  vi.spyOn(mailClient, "listAccounts").mockImplementation(async () =>
    (await originalAccounts()).map((account) => ({
      ...account,
      status: needsReconnect ? "needs_reauth" as const : "connected" as const,
    })),
  );
  vi.spyOn(mailClient, "reconnectAccount").mockImplementation(async (email) => {
    needsReconnect = false;
    return (await originalAccounts()).find((account) => account.email === email)!;
  });
  vi.spyOn(mailClient, "listThreadsPage").mockImplementation(async (...args) => {
    listCalls += 1;
    return originalList(...args);
  });

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  expect((await screen.findByRole("radio", { name: /demo@example\.com.*Needs reconnect/ })).querySelector(".account-reconnect-badge")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  fireEvent.click(within(dialog).getByRole("button", { name: "Mail Accounts" }));
  const callsBeforeReconnect = listCalls;

  fireEvent.click(await within(dialog).findByRole("button", { name: "Reconnect" }));

  await waitFor(() => expect(mailClient.reconnectAccount).toHaveBeenCalled());
  await waitFor(() => expect(listCalls).toBeGreaterThan(callsBeforeReconnect));
  expect(within(dialog).getByText("Connected")).toBeInTheDocument();
  expect(screen.queryByRole("radio", { name: /Needs reconnect/ })).not.toBeInTheDocument();
});

it("reloads the inbox and reports the error when reconnecting an account fails", async () => {
  const originalAccounts = mailClient.listAccounts.bind(mailClient);
  const originalList = mailClient.listThreadsPage.bind(mailClient);
  let listCalls = 0;

  vi.spyOn(mailClient, "listAccounts").mockImplementation(async () =>
    (await originalAccounts()).map((account) => ({ ...account, status: "needs_reauth" as const })),
  );
  vi.spyOn(mailClient, "reconnectAccount").mockRejectedValue(new Error("Google sign-in was cancelled"));
  vi.spyOn(mailClient, "listThreadsPage").mockImplementation(async (...args) => {
    listCalls += 1;
    return originalList(...args);
  });

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  fireEvent.click(within(dialog).getByRole("button", { name: "Mail Accounts" }));
  const callsBeforeReconnect = listCalls;

  fireEvent.click(await within(dialog).findByRole("button", { name: "Reconnect" }));

  expect(await within(dialog).findByText("Google sign-in was cancelled")).toBeInTheDocument();
  await waitFor(() => expect(listCalls).toBeGreaterThan(callsBeforeReconnect));
});

it("shows a queued reply immediately and replaces it with the provider copy", async () => {
  const detail = await mailClient.getThread("welcome");
  const source = detail.messages.at(-1)!;
  const queued: OutboxItem = {
    id: "queued-reply",
    state: "undo_pending",
    deadline: Date.now() + 10_000,
    error: null,
    draft: {
      id: "reply-draft",
      revision: 2,
      account: "demo@example.com",
      mode: "reply",
      sourceId: source.id,
      threadId: detail.thread.providerThreadId,
      replyId: "source@example.com",
      references: [],
      to: '"Doe, Jane" <jane@example.com>, brian@example.com',
      cc: "team@example.com",
      bcc: "",
      subject: detail.thread.subject,
      body: "Immediate reply",
      bodyHtml: "<p>Immediate reply</p>",
      attachments: [{
        id: "inline-1",
        name: "image.png",
        mime: "image/png",
        size: 4,
        ready: true,
        messageId: null,
        providerId: null,
        inline: true,
        contentId: "inline-1@threestrands.local",
      }],
      updatedAt: Date.now(),
    },
  };

  const optimistic = messagesWithQueuedReplies(detail, [queued]);
  expect(optimistic).toHaveLength(detail.messages.length + 1);
  expect(optimistic.at(-1)).toMatchObject({
    id: "outbox-queued-reply",
    bodyText: "Immediate reply",
    recipients: ['"Doe, Jane" <jane@example.com>', "brian@example.com", "team@example.com"],
    attachments: [{
      id: "inline-1",
      filename: "image.png",
      mimeType: "image/png",
      inline: true,
      contentId: "inline-1@threestrands.local",
    }],
  });

  const providerDetail = {
    ...detail,
    messages: [...detail.messages, {
      ...optimistic.at(-1)!,
      id: "gmail-sent-reply",
      sentAt: new Date().toISOString(),
    }],
  };
  expect(messagesWithQueuedReplies(providerDetail, [{ ...queued, state: "sent", providerId: "gmail-sent-reply" }]))
    .toHaveLength(providerDetail.messages.length);
  expect(messagesWithQueuedReplies(detail, [{ ...queued, state: "canceled" }]))
    .toHaveLength(detail.messages.length);
});

it("resolves a queued reply's inline image from its outbox draft", async () => {
  const detail = await mailClient.getThread("welcome");
  const source = detail.messages.at(-1)!;
  const queued: OutboxItem = {
    id: "queued-inline-reply",
    state: "undo_pending",
    deadline: Date.now() + 10_000,
    error: null,
    draft: {
      id: "inline-reply-draft",
      revision: 2,
      account: "demo@example.com",
      mode: "reply",
      sourceId: source.id,
      threadId: detail.thread.providerThreadId,
      replyId: "source@example.com",
      references: [],
      to: "hello@threestrands.local",
      cc: "",
      bcc: "",
      subject: detail.thread.subject,
      body: "Screenshot",
      bodyHtml: '<p>Screenshot</p><img src="cid:inline-1@threestrands.local" alt="image.png">',
      attachments: [{
        id: "inline-1",
        name: "image.png",
        mime: "image/png",
        size: 4,
        ready: true,
        messageId: null,
        providerId: null,
        inline: true,
        contentId: "inline-1@threestrands.local",
      }],
      updatedAt: Date.now(),
    },
  };
  vi.spyOn(mailClient, "listOutbox").mockResolvedValue([queued]);
  vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
  const readInline = vi.spyOn(mailClient, "readInlineImage")
    .mockResolvedValue("data:image/png;base64,iVBORw==");

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

  await waitFor(() => expect(readInline).toHaveBeenCalledWith("inline-reply-draft", "inline-1"));
  // Resolved images are patched into the loaded frame rather than rewritten
  // into srcdoc. jsdom does not load srcdoc, so mirror it and fire load.
  const queuedFrame = (screen.getAllByTestId("message-body") as HTMLIFrameElement[])
    .find((body) => body.srcdoc.includes("cid:inline-1@threestrands.local"))!;
  queuedFrame.contentDocument!.body.innerHTML = new DOMParser()
    .parseFromString(queuedFrame.srcdoc, "text/html").body.innerHTML;
  fireEvent.load(queuedFrame);
  await waitFor(() => {
    expect(queuedFrame.contentDocument!.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,iVBORw==");
  });
});

it("renders a reply in the open conversation as soon as Send queues it", async () => {
  localStorage.removeItem("threestrands.demoCorrespondence");
  try {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Reply" }));

    const editor = await screen.findByRole("textbox", { name: "Message Body" });
    editor.innerHTML = "<p>Visible without waiting for delivery</p>";
    fireEvent.input(editor);
    fireEvent.click(screen.getByRole("button", { name: /Send/ }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reply Message" })).not.toBeInTheDocument());
    await waitFor(() => {
      const bodies = screen.getAllByTestId("message-body") as HTMLIFrameElement[];
      expect(bodies.some((body) => body.srcdoc.includes("Visible without waiting for delivery"))).toBe(true);
    });
    expect(screen.getByRole("status")).toHaveTextContent("Sending in");
  } finally {
    localStorage.removeItem("threestrands.demoCorrespondence");
  }
});

it("sends and marks the open conversation done with Mod+Shift+Enter", async () => {
  localStorage.removeItem("threestrands.demoCorrespondence");
  const mutateThreads = vi.spyOn(mailClient, "mutateThreads").mockResolvedValue();
  try {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Reply" }));

    const editor = await screen.findByRole("textbox", { name: "Message Body" });
    editor.innerHTML = "<p>Send this and mark the conversation done</p>";
    fireEvent.input(editor);
    fireEvent.keyDown(editor, { key: "Enter", metaKey: true, shiftKey: true });

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reply Message" })).not.toBeInTheDocument());
    await waitFor(() => expect(mutateThreads).toHaveBeenCalledWith([
      { kind: "archive", threadId: "welcome", value: true },
    ]));
  } finally {
    localStorage.removeItem("threestrands.demoCorrespondence");
  }
});
