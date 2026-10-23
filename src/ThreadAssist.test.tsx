import { openUrl } from "@tauri-apps/plugin-opener";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import { SLOTS_PER_DAY } from "./scheduling";
import { briefScope, meetingPlace, participantName, ThreadAssist, type ThreadAssistProps } from "./ThreadAssist";
import type { ActionProposal, ThreadDetail } from "./domain";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const detail: ThreadDetail = {
  thread: {
    id: "thread-1",
    providerThreadId: "provider-1",
    subject: "Website setup",
    snippet: "Please set up the website by Friday.",
    participants: ["client@example.com"],
    lastMessageAt: "2026-09-19T10:00:00Z",
    lastReceivedAt: "2026-09-19T10:00:00Z",
    unread: false,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: "you@example.com",
    summary: null,
    summaryGeneratedAt: null,
    summaryRevision: null,
    hasAttachments: false,
  },
  messages: [{
    id: "message-1",
    threadId: "thread-1",
    sender: "client@example.com",
    recipients: ["you@example.com"],
    sentAt: "2026-09-19T10:00:00Z",
    bodyHtml: "<p>Please set up the website by Friday.</p>",
    bodyText: "Please set up the website by Friday.",
    unread: false,
    unsubscribe: null,
    attachments: [],
  }],
};

const summarized: ThreadDetail = {
  ...detail,
  thread: { ...detail.thread, summary: "- The client needs the website.\n- Due Friday.", summaryGeneratedAt: "2026-09-19T11:00:00Z" },
};

const taskProposal: ActionProposal = {
  type: "task",
  kind: "action",
  title: "Set up the website",
  notes: "Complete the requested setup.",
  dueKind: "date",
  dueValue: "2026-09-25",
  timeZone: "America/New_York",
  repeatIntervalDays: null,
  confidence: 0.86,
  evidence: { sourceMessageId: "message-1", excerpt: "Please set up the website by Friday." },
};

function props(overrides: Partial<Omit<ThreadAssistProps, "summary" | "suggestions" | "scheduling">> & {
  summary?: Partial<ThreadAssistProps["summary"]>;
  suggestions?: Partial<ThreadAssistProps["suggestions"]>;
  scheduling?: Partial<ThreadAssistProps["scheduling"]>;
} = {}): ThreadAssistProps {
  const { summary, suggestions, scheduling, ...rest } = overrides;
  return {
    detail,
    providerChosen: true,
    loading: false,
    error: null,
    providerName: "Anthropic",
    onRun: vi.fn(),
    onOpenSettings: vi.fn(),
    ...rest,
    summary: { enabled: true, available: true, pending: false, ...summary },
    suggestions: {
      enabled: true,
      available: true,
      requested: false,
      proposals: [],
      hiddenCount: 0,
      onDiscard: vi.fn(),
      onReview: vi.fn(),
      ...suggestions,
    },
    scheduling: {
      calendarConnected: false,
      preferences: { timeZone: "America/New_York", workingWindows: [], defaultDurationMinutes: 30, slotIncrementMinutes: 30, driveTimeBufferMinutes: 0 },
      onAddToCalendar: vi.fn(),
      onReplyWithTimes: vi.fn(),
      onConfirmTime: vi.fn(),
      onOpenCalendarSettings: vi.fn(),
      ...scheduling,
    },
  };
}

describe("meetingPlace", () => {
  it("prefers the calendar copy's meeting link and keeps the place's own words", () => {
    expect(meetingPlace("Microsoft Teams", "https://meet.example.com/abc")).toEqual({ place: "Microsoft Teams", joinUrl: "https://meet.example.com/abc" });
    expect(meetingPlace("Room 4", null)).toEqual({ place: "Room 4", joinUrl: null });
    expect(meetingPlace("224 Fairoaks St.", null)).toEqual({ place: "224 Fairoaks St.", joinUrl: null });
    expect(meetingPlace(null, null)).toEqual({ place: "", joinUrl: null });
  });

  it("takes a web address written in the location as the link, without its trailing punctuation", () => {
    expect(meetingPlace("Video call: https://video.example.org/j/123.", null)).toEqual({ place: "Video call", joinUrl: "https://video.example.org/j/123" });
    expect(meetingPlace("https://video.example.org/j/123", undefined)).toEqual({ place: "", joinUrl: "https://video.example.org/j/123" });
  });

  it("never links anything but a web address", () => {
    expect(meetingPlace("Room 4", "javascript:alert(1)")).toEqual({ place: "Room 4", joinUrl: null });
    expect(meetingPlace("file:///etc/passwd", "ftp://files.example.com")).toEqual({ place: "file:///etc/passwd", joinUrl: null });
  });
});

describe("participantName", () => {
  it("reverses a directory-style name and leaves everything else as written", () => {
    expect(participantName("Dadeboe, Joshua")).toBe("Joshua Dadeboe");
    expect(participantName("Rivera, Ana María")).toBe("Ana María Rivera");
    expect(participantName("Joshua Dadeboe")).toBe("Joshua Dadeboe");
    expect(participantName("Smith, Jr.")).toBe("Smith, Jr.");
    expect(participantName("Acme, Inc")).toBe("Acme, Inc");
    expect(participantName("Van Dyke, Dick")).toBe("Van Dyke, Dick");
    expect(participantName("a, b, c")).toBe("a, b, c");
    expect(participantName("Finance team,")).toBe("Finance team,");
  });
});

describe("ThreadAssist", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("collapses from its heading, keeps its actions, and remembers the choice", () => {
    const onRun = vi.fn();
    const { unmount } = render(<ThreadAssist {...props({ onRun, detail: summarized, suggestions: { requested: true } })} />);
    const toggle = screen.getByRole("button", { name: "Brief" });
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("The client needs the website.")).not.toBeVisible();
    // Copy and Refresh still work while collapsed.
    fireEvent.click(screen.getByRole("button", { name: "Refresh brief" }));
    expect(onRun).toHaveBeenCalledWith(true);
    expect(screen.getByRole("button", { name: "Copy brief" })).toBeInTheDocument();
    unmount();

    render(<ThreadAssist {...props({ detail: summarized, suggestions: { requested: true } })} />);
    expect(screen.getByRole("button", { name: "Brief" })).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(screen.getByRole("button", { name: "Brief" }));
    expect(screen.getByText("The client needs the website.")).toBeVisible();
  });

  it("says in words what was shared and with whom, and links to the activity log", () => {
    const onOpenSettings = vi.fn();
    const { rerender } = render(<ThreadAssist {...props({ onOpenSettings, detail: summarized, suggestions: { requested: true } })} />);
    expect(screen.getByText(/Shared 1 message with Anthropic\./)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "AI Activity" }));
    expect(onOpenSettings).toHaveBeenCalled();
    rerender(<ThreadAssist {...props({ detail: summarized, providerName: "your custom endpoint", suggestions: { requested: true } })} />);
    expect(screen.getByText(/Shared 1 message with your custom endpoint\./)).toBeInTheDocument();
    // Nothing is claimed while the brief is still being read.
    rerender(<ThreadAssist {...props({ detail: summarized, loading: true, suggestions: { requested: true } })} />);
    expect(screen.queryByText(/^Shared /)).not.toBeInTheDocument();
  });

  it("says when a brief covers only the latest messages and reads the recap of earlier ones last", () => {
    const message = (id: string, sentAt: string) => ({ ...detail.messages[0], id, sentAt });
    const thread: ThreadDetail = {
      thread: {
        ...summarized.thread,
        lastMessageAt: "2026-09-21T10:00:00Z",
        summary: "- Earlier: The client asked for the website by Friday.\n- They now want it Monday instead.",
        summaryRevision: "2026-09-21T10:00:00Z",
        summaryFocusFrom: "2026-09-21T10:00:00Z",
      },
      messages: [message("m1", "2026-09-19T10:00:00Z"), message("m2", "2026-09-20T10:00:00Z"), message("m3", "2026-09-21T10:00:00Z")],
    };
    const { rerender } = render(<ThreadAssist {...props({ detail: thread, suggestions: { requested: true } })} />);
    expect(screen.getByText("From the latest message")).toBeInTheDocument();
    const items = screen.getAllByRole("listitem");
    expect(items.map((item) => item.textContent)).toEqual(["They now want it Monday instead.", "Earlier: The client asked for the website by Friday."]);
    expect(items[1]).toHaveClass("thread-assist-summary-earlier");
    expect(screen.getByText(/Shared 3 messages with Anthropic\./)).toBeInTheDocument();

    rerender(<ThreadAssist {...props({ detail: { ...thread, thread: { ...thread.thread, summaryFocusFrom: "2026-09-20T10:00:00Z" } }, suggestions: { requested: true } })} />);
    expect(screen.getByText("From the 2 latest messages")).toBeInTheDocument();
    // A brief of the whole conversation, or one saved before focus was recorded, needs no note.
    rerender(<ThreadAssist {...props({ detail: { ...thread, thread: { ...thread.thread, summaryFocusFrom: null } }, suggestions: { requested: true } })} />);
    expect(screen.queryByText(/^From the /)).not.toBeInTheDocument();
  });

  it("counts only messages the brief was written from, and at most what a brief sends", () => {
    const many: ThreadDetail = {
      thread: { ...summarized.thread, lastMessageAt: "2026-09-30T00:00:00Z", summaryRevision: "2026-09-25T00:00:00Z", summaryFocusFrom: "2026-09-24T00:00:00Z" },
      messages: Array.from({ length: 25 }, (_, index) => ({ ...detail.messages[0], id: `m${index}`, sentAt: `2026-09-${String(index + 6).padStart(2, "0")}T00:00:00Z` })),
    };
    // Sept 6 through 25 were covered (20 messages); Sept 24 and 25 were new.
    expect(briefScope(many)).toEqual({ newCount: 2, coveredCount: 20, sentCount: 15 });
  });

  it("offers one Get Brief action before anything has run and refreshes everything afterwards", () => {
    const onRun = vi.fn();
    const { rerender } = render(<ThreadAssist {...props({ onRun })} />);
    // Before anything has run, the section is just its button, beside the panel's other AI actions.
    const launcher = screen.getByRole("region", { name: "Brief" });
    expect(launcher).toHaveClass("context-ai-launcher");
    expect(within(launcher).queryByRole("heading")).not.toBeInTheDocument();
    const getBrief = within(launcher).getByRole("button", { name: "Get Brief" });
    expect(getBrief.querySelector("svg")).not.toBeNull();
    expect(getBrief).toHaveAttribute("aria-keyshortcuts", "i");
    const briefTooltip = within(getBrief.parentElement as HTMLElement).getByRole("tooltip");
    expect(within(briefTooltip).getByText("Get Brief")).toBeInTheDocument();
    expect(within(briefTooltip).getByText("i").tagName).toBe("KBD");
    fireEvent.click(getBrief);
    expect(onRun).toHaveBeenLastCalledWith(false);
    expect(screen.queryByRole("button", { name: "Refresh brief" })).not.toBeInTheDocument();

    rerender(<ThreadAssist {...props({ onRun, detail: summarized, suggestions: { requested: true } })} />);
    // With a brief, the section opens under a heading that collapses it, like
    // the panel's other sections; the AI mark stays on actions.
    const heading = screen.getByRole("heading", { name: "Brief" });
    expect(within(heading).getByRole("button", { name: "Brief" })).toHaveAttribute("aria-expanded", "true");
    expect(heading.querySelector(".lucide-sparkles")).toBeNull();
    expect(screen.getByText("The client needs the website.")).toBeInTheDocument();
    expect(screen.getByText("Due Friday.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Refresh brief" }));
    expect(onRun).toHaveBeenLastCalledWith(true);
  });

  it("shows one run button before anything has run, then keeps actions in the section header", () => {
    const { rerender, container } = render(<ThreadAssist {...props()} />);
    expect(container.querySelector(".context-section-header")).toBeNull();
    expect(container.querySelectorAll(".thread-assist-run")).toHaveLength(1);

    // A brief without suggestions yet offers them from the header.
    rerender(<ThreadAssist {...props({ detail: summarized })} />);
    const header = container.querySelector(".context-section-header")!;
    expect(within(header as HTMLElement).getByRole("button", { name: "Get Suggestions" })).toBeInTheDocument();

    rerender(<ThreadAssist {...props({ detail: summarized, suggestions: { requested: true } })} />);
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    expect(within(container.querySelector(".context-section-header") as HTMLElement).getByRole("button", { name: "Refresh brief" })).toBeInTheDocument();

    rerender(<ThreadAssist {...props({ loading: true })} />);
    expect(within(container.querySelector(".context-section-header") as HTMLElement).getByRole("status")).toHaveTextContent("Reading the conversation…");
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
  });

  it("asks only for the missing suggestions when a summary was saved earlier", () => {
    const onRun = vi.fn();
    render(<ThreadAssist {...props({ onRun, detail: summarized })} />);
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    const getSuggestions = screen.getByRole("button", { name: "Get Suggestions" });
    expect(getSuggestions).not.toHaveAttribute("aria-keyshortcuts");
    expect(within(getSuggestions.parentElement as HTMLElement).getByRole("tooltip").querySelector("kbd")).toBeNull();
    fireEvent.click(getSuggestions);
    expect(onRun).toHaveBeenCalledWith(false);
  });

  it("flags a summary that predates the newest message", () => {
    render(<ThreadAssist {...props({ detail: { ...summarized, thread: { ...summarized.thread, lastMessageAt: "2026-09-20T09:00:00Z" } } })} />);
    expect(screen.getByText("New messages since this brief.")).toBeInTheDocument();
  });

  it("flags mail that arrived while the brief was being written, even though it predates the save", () => {
    // Written from the thread as of 10:00, saved at 11:00; a message sent at
    // 10:30 arrived during the provider call.
    const thread = { ...summarized.thread, summaryRevision: "2026-09-19T10:00:00Z", summaryGeneratedAt: "2026-09-19T11:00:00Z", lastMessageAt: "2026-09-19T10:30:00Z" };
    render(<ThreadAssist {...props({ detail: { ...summarized, thread } })} />);
    expect(screen.getByText("New messages since this brief.")).toBeInTheDocument();
  });

  it("titles the section Suggestions when summaries are off", () => {
    const { rerender } = render(<ThreadAssist {...props({ detail: summarized, summary: { enabled: false, available: false } })} />);
    const launcher = screen.getByRole("region", { name: "Suggestions" });
    expect(screen.queryByText("The client needs the website.")).not.toBeInTheDocument();
    expect(within(launcher).getByRole("button", { name: "Get Suggestions" })).toBeInTheDocument();
    rerender(<ThreadAssist {...props({ detail: summarized, summary: { enabled: false, available: false }, suggestions: { requested: true } })} />);
    expect(screen.getByRole("heading", { name: "Suggestions" })).toBeInTheDocument();
  });

  it("shows a non-retryable error verbatim without Try Again or technical details", () => {
    const onRun = vi.fn();
    const message = "AI briefs are disabled for this account";
    const { container } = render(<ThreadAssist {...props({ onRun, error: message })} />);
    expect(screen.getByRole("alert")).toHaveTextContent(message);
    expect(screen.queryByRole("button", { name: "Try Again" })).not.toBeInTheDocument();
    expect(screen.queryByText("Technical details")).not.toBeInTheDocument();
    expect(container.querySelector(".action-analysis-error-details")).toBeNull();
    expect(onRun).not.toHaveBeenCalled();
  });

  it("explains missing provider credentials separately from the feature flags", () => {
    const onOpenSettings = vi.fn();
    render(<ThreadAssist {...props({ onOpenSettings, summary: { available: false }, suggestions: { available: false } })} />);
    expect(screen.getByText("Set up an AI provider and API key in AI settings to get a brief of this conversation.")).toBeInTheDocument();
    expect(screen.queryByText("AI briefs and suggestions are off.")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "AI Settings" }));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);

    // Switched off on purpose with a provider chosen: the section disappears instead of reminding.
    const { container } = render(<ThreadAssist {...props({ onOpenSettings, summary: { enabled: false, available: false }, suggestions: { enabled: false, available: false } })} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("is the single place that suggests setting up AI when no provider is chosen", () => {
    const onOpenSettings = vi.fn();
    render(<ThreadAssist {...props({ providerChosen: false, onOpenSettings, summary: { enabled: false, available: false }, suggestions: { enabled: false, available: false } })} />);
    const section = screen.getByRole("region", { name: "Brief" });
    expect(section).toHaveTextContent("Set up AI to get a brief of each conversation, suggested next steps, and answers to your questions about it.");
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    // Opening settings sends nothing to a provider, so it carries no AI mark.
    const setUp = within(section).getByRole("button", { name: "Set Up AI" });
    expect(setUp.querySelector("svg")).toBeNull();
    fireEvent.click(setUp);
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it("previews typed proposals with evidence and review actions", () => {
    const onReview = vi.fn();
    const onDiscard = vi.fn();
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [taskProposal], onReview, onDiscard } })} />);

    expect(screen.getByRole("heading", { name: "Suggested" })).toBeInTheDocument();
    expect(screen.getByText("Set up the website")).toBeInTheDocument();
    fireEvent.click(screen.getByText("From the email"));
    expect(screen.getByText("Please set up the website by Friday.")).toBeInTheDocument();
    expect(screen.getByText(/^client@example\.com · /)).toBeInTheDocument();
    expect(screen.queryByText(/confidence/i)).not.toBeInTheDocument();
    expect(screen.queryByText("Check details")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Review & Add Task" }));
    expect(onReview).toHaveBeenCalledWith(0, taskProposal, "accept");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(onReview).toHaveBeenLastCalledWith(0, taskProposal, "edit");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onDiscard).toHaveBeenCalledWith(0);
    // What was sent is said in words; the request itself is in Settings → AI.
    expect(screen.queryByText("What was shared with AI")).not.toBeInTheDocument();
    expect(document.querySelector("pre")).toBeNull();
  });

  it.each([
    { id: "invite", filename: "invite.ics", mimeType: "application/octet-stream", size: 100 },
    { id: "invite", filename: "", mimeType: "TEXT/CALENDAR; method=REQUEST", size: 100 },
  ])("never shows an AI meeting from an invitation, while preserving task actions", (attachment) => {
    const invitationDetail = { ...summarized, messages: [{ ...detail.messages[0], attachments: [attachment] }] };
    const meeting: ActionProposal = {
      type: "meeting", intent: "attend", title: "Duplicate invitation", participants: [], location: null,
      rawTimeLanguage: "Tuesday at 1pm", normalizedStart: null, normalizedEnd: null,
      searchRangeStart: null, searchRangeEnd: null, durationMinutes: 30, timeZone: null, confidence: 1,
      evidence: taskProposal.evidence,
    };
    const onReview = vi.fn();
    const onDiscard = vi.fn();
    const { rerender } = render(<ThreadAssist {...props({ detail: invitationDetail, suggestions: { requested: true, proposals: [meeting, taskProposal], onReview, onDiscard } })} />);
    expect(screen.queryByText("Duplicate invitation")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Review & Add Task" }));
    expect(onReview).toHaveBeenCalledWith(1, taskProposal, "accept");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onDiscard).toHaveBeenCalledWith(1);
    rerender(<ThreadAssist {...props({ detail: invitationDetail, suggestions: { requested: true, proposals: [meeting] } })} />);
    expect(screen.queryByRole("heading", { name: "Suggested" })).not.toBeInTheDocument();
    expect(screen.queryByText("Nothing to schedule or follow up on.")).not.toBeInTheDocument();
  });

  it("flags uncertain suggestions for a closer look and schedules an unzoned meeting in the user's time zone", async () => {
    const meeting: ActionProposal = {
      type: "meeting",
      intent: "schedule",
      title: "Website kickoff",
      participants: [],
      location: null,
      rawTimeLanguage: "sometime next week",
      normalizedStart: null,
      normalizedEnd: null,
      searchRangeStart: null,
      searchRangeEnd: null,
      durationMinutes: 30,
      timeZone: null,
      confidence: 0.4,
      evidence: { sourceMessageId: "message-unknown", excerpt: "Let's meet next week." },
    };
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    const onReview = vi.fn();
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [meeting], onReview }, scheduling: { calendarConnected: true } })} />);

    // The flag says where to look: it opens the editor.
    fireEvent.click(screen.getByRole("button", { name: "Check details" }));
    expect(onReview).toHaveBeenCalledWith(0, meeting, "edit");
    // The email's wording of the time reads as a quote, not a description.
    expect(screen.getByText("sometime next week")).toHaveClass("proposal-time-language");
    expect(screen.queryByText(/%/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Find Times" })).not.toBeInTheDocument();
    const schedule = screen.getByRole("group", { name: "Schedule" });
    expect(within(schedule).getByText("Times in Eastern Time (the email didn’t say)")).toBeInTheDocument();
    await waitFor(() => expect(find).toHaveBeenCalledWith(expect.objectContaining({ maxPerDay: SLOTS_PER_DAY })));
    expect(await within(schedule).findByText("No open times in your working hours for this range.")).toBeInTheDocument();
    fireEvent.click(screen.getByText("From the email"));
    expect(screen.queryByText(/message-unknown/)).not.toBeInTheDocument();
  });

  it("schedules meetings only, and hands Add to Calendar the card's own suggestion", async () => {
    const meeting: ActionProposal = {
      type: "meeting", intent: "schedule", title: "Budget review", participants: ["jane@example.com"], location: null,
      rawTimeLanguage: "Thursday at 3", normalizedStart: new Date(Date.now() + 26 * 3_600_000).toISOString(), normalizedEnd: new Date(Date.now() + 26.5 * 3_600_000).toISOString(),
      searchRangeStart: null, searchRangeEnd: null, durationMinutes: 30, timeZone: "America/New_York", confidence: 0.9,
      evidence: { sourceMessageId: "message-1", excerpt: "Please set up the website by Friday." },
    };
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    const onAddToCalendar = vi.fn();
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [taskProposal, meeting] }, scheduling: { calendarConnected: true, onAddToCalendar } })} />);

    expect(screen.getAllByRole("group", { name: "Schedule" })).toHaveLength(1);
    fireEvent.click(await screen.findByRole("button", { name: "Add to Calendar" }));
    expect(onAddToCalendar).toHaveBeenCalledWith(1, meeting, { kind: "specific", start: new Date(meeting.normalizedStart!).toISOString(), end: new Date(meeting.normalizedEnd!).toISOString() });
  });

  it("reports withheld suggestions instead of claiming there was nothing to do", () => {
    const { rerender } = render(<ThreadAssist {...props({ suggestions: { requested: true, hiddenCount: 2 } })} />);
    expect(screen.getByText("2 suggestions couldn’t be matched to the email, so they were hidden.")).toBeInTheDocument();
    expect(screen.queryByText("Nothing to schedule or follow up on.")).not.toBeInTheDocument();

    rerender(<ThreadAssist {...props({ suggestions: { requested: true, hiddenCount: 1 } })} />);
    expect(screen.getByText("1 suggestion couldn’t be matched to the email, so it was hidden.")).toBeInTheDocument();

    rerender(<ThreadAssist {...props({ suggestions: { requested: true } })} />);
    expect(screen.getByText("Nothing to schedule or follow up on.")).toBeInTheDocument();
  });

  it("reports suggestions past the limit separately from withheld ones", () => {
    const { rerender } = render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [taskProposal], extraCount: 3 } })} />);
    expect(screen.getByText("3 more suggestions weren’t shown. Ask about one by name to add it.")).toBeInTheDocument();
    expect(screen.queryByText(/couldn’t be matched/)).not.toBeInTheDocument();
    rerender(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [taskProposal], extraCount: 1 } })} />);
    expect(screen.getByText("1 more suggestion wasn’t shown. Ask about it by name to add it.")).toBeInTheDocument();
    rerender(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [taskProposal], extraCount: 0 } })} />);
    expect(screen.queryByText(/more suggestion/)).not.toBeInTheDocument();
  });

  it("labels announced events and keeps each card's calendar check when an earlier card is discarded", async () => {
    const check = vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    const event: ActionProposal = {
      type: "meeting", intent: "attend", title: "Bonfire", participants: [], location: "224 Fairoaks St",
      rawTimeLanguage: "Saturday, starting around 6:45 PM", normalizedStart: new Date(Date.now() + 3 * 86_400_000).toISOString(),
      normalizedEnd: null, eventDate: null, searchRangeStart: null, searchRangeEnd: null, durationMinutes: 120,
      timeZone: "America/New_York", confidence: 0.8, evidence: { sourceMessageId: "message-1", excerpt: "Please set up the website" },
    };
    const scheduling = { calendarConnected: true };
    const { rerender } = render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [taskProposal, event] }, scheduling })} />);
    expect(screen.getByText("Event")).toBeInTheDocument();
    expect(await screen.findByText("You’re free")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reply “That Works”" })).not.toBeInTheDocument();

    rerender(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [event] }, scheduling })} />);
    expect(screen.getByText("You’re free")).toBeInTheDocument();
    expect(screen.queryByText("Checking your calendar…")).not.toBeInTheDocument();
    expect(check).toHaveBeenCalledTimes(1);
  });

  it("recognizes a suggested event already on the calendar by its title and opens it there", async () => {
    clearScheduleCache();
    const start = new Date(Date.now() + 4 * 86_400_000).toISOString();
    const end = new Date(Date.parse(start) + 2 * 3_600_000).toISOString();
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [{ start, end }], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    const added = { id: "added", accountId: "you@example.com", title: "Entrepreneur Day 2026", allDay: false, start, end };
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [added], errors: [] });
    const event: ActionProposal = {
      type: "meeting", intent: "attend", title: "Entrepreneur Day 2026", participants: [], location: "1435 Bedford Ave",
      rawTimeLanguage: "November 17 at 4:00 PM", normalizedStart: start, normalizedEnd: end, eventDate: null,
      searchRangeStart: null, searchRangeEnd: null, durationMinutes: 120,
      timeZone: "America/New_York", confidence: 0.9, evidence: { sourceMessageId: "message-1", excerpt: "You're invited to Entrepreneur Day 2026" },
    };
    const onOpenCalendarEvent = vi.fn();
    const onReview = vi.fn();
    const onDiscard = vi.fn();
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [event], onReview, onDiscard }, scheduling: { calendarConnected: true, onOpenCalendarEvent } })} />);
    expect(await screen.findByText("Already on your calendar")).toBeInTheDocument();
    const open = screen.getByRole("button", { name: "Open in Calendar" });
    fireEvent.click(open);
    expect(onOpenCalendarEvent).toHaveBeenCalledWith(added);

    // The scheduler's line says when; the email's wording of it would repeat it.
    await waitFor(() => expect(screen.queryByText("November 17 at 4:00 PM")).not.toBeInTheDocument());
    // One row of actions: Edit waits in ⋯ with Add Again, and Dismiss sits beside Open in Calendar.
    const card = open.closest("article") as HTMLElement;
    expect(card.querySelectorAll(".proposal-actions")).toHaveLength(0);
    expect(within(card).queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    fireEvent.click(within(card).getByRole("button", { name: "More actions" }));
    fireEvent.click(within(card).getByRole("menuitem", { name: "Edit" }));
    expect(onReview).toHaveBeenCalledWith(0, event, "edit");
    fireEvent.click(within(card).getByRole("button", { name: "Dismiss" }));
    expect(onDiscard).toHaveBeenCalledWith(0);
  });

  it("offers to join an online meeting from the link on its calendar copy", async () => {
    clearScheduleCache();
    const start = new Date(Date.now() + 4 * 86_400_000).toISOString();
    const end = new Date(Date.parse(start) + 3_600_000).toISOString();
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [{ start, end }], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [{ id: "council", accountId: "you@example.com", title: "Finance Council", allDay: false, start, end, conferenceUrl: "https://meet.example.com/council" }],
      errors: [],
    });
    const event: ActionProposal = {
      type: "meeting", intent: "attend", title: "Finance Council", participants: [], location: "Microsoft Teams",
      rawTimeLanguage: "next week", normalizedStart: start, normalizedEnd: end, eventDate: null,
      searchRangeStart: null, searchRangeEnd: null, durationMinutes: 60,
      timeZone: "America/New_York", confidence: 0.9, evidence: { sourceMessageId: "message-1", excerpt: "Finance Council on Teams" },
    };
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [event] }, scheduling: { calendarConnected: true } })} />);
    // Before the calendar copy is found, a place with no address is only a place.
    expect(screen.queryByRole("link", { name: "Join" })).not.toBeInTheDocument();
    const join = await screen.findByRole("link", { name: "Join" });
    expect(join).toHaveAttribute("title", "https://meet.example.com/council");
    expect(join.closest(".proposal-detail")).toHaveTextContent("Microsoft Teams · Join");
    fireEvent.click(join);
    expect(openUrl).toHaveBeenCalledWith("https://meet.example.com/council");
  });

  it("offers to join from a web address written as the location", () => {
    const meeting: ActionProposal = {
      type: "meeting", intent: "attend", title: "Office hours", participants: [], location: "https://video.example.org/j/123",
      rawTimeLanguage: "Thursday", normalizedStart: null, normalizedEnd: null, eventDate: null,
      searchRangeStart: null, searchRangeEnd: null, durationMinutes: 30, timeZone: null, confidence: 0.9,
      evidence: { sourceMessageId: "message-1", excerpt: "Office hours Thursday" },
    };
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [meeting] } })} />);
    expect(screen.queryByText("https://video.example.org/j/123")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Join video meeting" }));
    expect(openUrl).toHaveBeenCalledWith("https://video.example.org/j/123");
  });

  it("shows a meeting's place and people with icons, names in reading order", () => {
    const meeting: ActionProposal = {
      type: "meeting", intent: "attend", title: "Finance Council", participants: ["Dadeboe, Joshua", "Rivera, Ana", "Smith, Jr."],
      location: "Microsoft Teams", rawTimeLanguage: "December 9th", normalizedStart: null, normalizedEnd: null, eventDate: null,
      searchRangeStart: null, searchRangeEnd: null, durationMinutes: 60, timeZone: null, confidence: 0.9,
      evidence: { sourceMessageId: "message-1", excerpt: "Finance Council on December 9th" },
    };
    render(<ThreadAssist {...props({ suggestions: { requested: true, proposals: [meeting] } })} />);
    expect(screen.getByText("December 9th")).toBeInTheDocument();
    expect(screen.getByText("Microsoft Teams").closest(".proposal-detail")?.querySelector("svg")).not.toBeNull();
    const people = screen.getByText(/Joshua Dadeboe/);
    expect(people).toHaveTextContent(/^Joshua Dadeboe, Ana Rivera,? and Smith, Jr\.$/);
    expect(people.closest(".proposal-detail")?.querySelector("svg")).not.toBeNull();
    // Before the meeting is on the calendar, Edit and Dismiss wait in the header's ⋯, with the email excerpt last.
    const card = people.closest("article") as HTMLElement;
    expect(within(card).queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(card.querySelector(".proposal-actions")).toBeNull();
    fireEvent.click(within(card).getByRole("button", { name: "More actions" }));
    expect(within(card).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Edit", "Dismiss"]);
    expect(card.lastElementChild).toHaveClass("proposal-evidence");
    // The disclosure uses the app's chevron rather than the browser's triangle.
    expect(card.querySelector(".proposal-evidence > summary > svg.proposal-evidence-chevron")).not.toBeNull();
  });

  it("explains unreadable provider output in plain language and keeps the details available", () => {
    const onRun = vi.fn();
    render(<ThreadAssist {...props({ onRun, error: "The AI provider returned malformed thread brief JSON" })} />);

    expect(screen.getByRole("alert")).toHaveTextContent("The AI's response couldn't be read. Try again, or choose a different model in AI settings.");
    expect(screen.getByText("The AI provider returned malformed thread brief JSON")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Get Brief" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    expect(onRun).toHaveBeenCalledWith(false);
  });

  it("offers no copy action before a brief exists", () => {
    render(<ThreadAssist {...props()} />);
    expect(screen.queryByRole("button", { name: "Copy brief" })).not.toBeInTheDocument();
  });

  it("copies the brief to the clipboard and confirms it until the brief changes", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const bulleted: ThreadDetail = {
      ...summarized,
      thread: { ...summarized.thread, summary: "- The client needs the website.\n• Due Friday." },
    };
    const { rerender } = render(<ThreadAssist {...props({ detail: bulleted })} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy brief" }));

    expect(writeText).toHaveBeenCalledWith("- The client needs the website.\n- Due Friday.");
    expect(await screen.findByRole("button", { name: "Copied brief" })).toBeInTheDocument();

    rerender(<ThreadAssist {...props({ detail: summarized })} />);
    expect(screen.getByRole("button", { name: "Copy brief" })).toBeInTheDocument();
  });

  it("reports when the brief cannot be copied", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("clipboard unavailable"));
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<ThreadAssist {...props({ detail: summarized })} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy brief" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Could not copy brief");
    expect(screen.getByRole("button", { name: "Copy brief" })).toBeInTheDocument();
  });

  it("stays read-only: no text entry or pickers in the AI section", () => {
    render(<ThreadAssist {...props({ detail: summarized, suggestions: { requested: true, proposals: [taskProposal] } })} />);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("adjusts a meeting suggestion's times with chips only, keeping the AI section free of text entry", async () => {
    const meeting: ActionProposal = {
      type: "meeting", intent: "schedule", title: "Website kickoff", participants: [], location: null,
      rawTimeLanguage: "next week", normalizedStart: null, normalizedEnd: null,
      searchRangeStart: null, searchRangeEnd: null, durationMinutes: 30, timeZone: null, confidence: 0.9,
      evidence: { sourceMessageId: "message-1", excerpt: "Let's meet next week." },
    };
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    const findTimes = vi.spyOn(mailClient, "findTimes");
    render(<ThreadAssist {...props({ detail: summarized, suggestions: { requested: true, proposals: [taskProposal, meeting] }, scheduling: { calendarConnected: true } })} />);

    fireEvent.click(await screen.findByRole("button", { name: "Afternoons" }));
    await waitFor(() => expect(find).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("button", { name: "Afternoons" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(findTimes).not.toHaveBeenCalled();
  });
});
